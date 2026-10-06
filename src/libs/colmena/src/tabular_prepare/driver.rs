//! The driver that prepares one large CSV: claim the registry row, convert the
//! source into Parquet parts on the host's storage, write the manifest LAST and
//! complete the row with it. Dark behind `COLMENA_LARGE_TABULAR`.
//!
//! Order matters. The manifest is the only thing that names the parts, so it is
//! put after every part is stored and the row is completed only after the
//! manifest is stored: a reader that finds a ready row finds every part it
//! lists. The key of every object is recorded before its put (the keys of the
//! parts by [`ConvertControl`], the manifest's here), so the registry always
//! tracks the union of everything any attempt may have written, including what
//! a failed put left. A failure is recorded with `fail_with_blobs` and the
//! tracked objects are then deleted; nothing but the failure reason is
//! user-visible, and it never carries a cell or a storage key.

use crate::storage::domain::OutputStorageRepository;
use crate::tabular_prepare::convert::{convert_csv_table_with, ConvertControl, ConvertedTable};
use crate::tabular_prepare::csv::Encoding;
use crate::tabular_prepare::manifest::{
    unique_table_names, ConversionReport, Manifest, TableInfo, MANIFEST_PATH, MAX_REPORTED_DEMOTED,
};
use crate::tabular_prepare::part_sink::PartSink;
use crate::tabular_prepare::ports::PrepareRequest;
use crate::tabular_prepare::prepare::{PrepareStartError, StorageCsvSource, StoragePartSink};
use crate::tabular_prepare::registry::{
    lease_for, ClaimRequest, PreparationRegistry, ReadyInfo, RegistryError, TerminalOutcome,
    FORMAT_VERSION,
};
use crate::tabular_prepare::writer::WriterConfig;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;

/// Time a preparation may take before it stops itself.
pub const PREP_TIMEOUT: Duration = Duration::from_secs(300);

/// The reasons a preparation can be recorded as failed. Stable, lowercase
/// snake_case; the host maps them to what the user sees. Each is written
/// together with a fixed detail sentence that carries no cell and no key.
pub mod reason {
    /// The time budget ([`super::PREP_TIMEOUT`]) ran out.
    pub const TIME: &str = "time";
    /// The storage could not be read or written (source read, a part or the
    /// manifest put), or answered with a key outside the prepared layout.
    pub const STORAGE: &str = "storage";
    /// The file cannot be read as a CSV: empty, UTF-16 or binary, an unclosed
    /// quote, a record over the limit, a row longer than the header, or more
    /// columns than the reader accepts.
    pub const UNREADABLE_FILE: &str = "unreadable_file";
    /// The table list does not fit the registry row (more than 64 KiB).
    pub const TABLE_TOO_LARGE: &str = "table_too_large";
    /// Anything else: a defect, never the file's fault.
    pub const INTERNAL: &str = "internal";
}

/// Source of "now". Read once per registry write.
pub type Clock = dyn Fn() -> DateTime<Utc> + Send + Sync;

/// What the driver needs.
pub struct PrepareEnv {
    pub registry: Arc<dyn PreparationRegistry>,
    pub storage: Arc<dyn OutputStorageRepository>,
    pub clock: Arc<Clock>,
    pub budget: Duration,
    pub writer: WriterConfig,
}

impl PrepareEnv {
    pub fn new(
        registry: Arc<dyn PreparationRegistry>,
        storage: Arc<dyn OutputStorageRepository>,
    ) -> Self {
        Self {
            registry,
            storage,
            clock: Arc::new(Utc::now),
            budget: PREP_TIMEOUT,
            writer: WriterConfig::default(),
        }
    }

    pub fn with_clock(mut self, clock: Arc<Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }

    pub fn with_writer(mut self, writer: WriterConfig) -> Self {
        self.writer = writer;
        self
    }
}

/// A table that is ready: what the registry row and the manifest record, and
/// what the conversion reported.
#[derive(Debug)]
pub struct PreparedTable {
    pub manifest: Manifest,
    /// Full storage key of the manifest.
    pub manifest_key: String,
    /// Every object any attempt may have written, parts and manifest, as full
    /// storage keys.
    pub blob_keys: Vec<String>,
    /// The part of `blob_keys` the manifest does not reference (left by an
    /// aborted run): safe to delete.
    pub stale_keys: Vec<String>,
    pub prepared_bytes: u64,
    /// Everything the conversion reported (rows, parts, encoding, replacements,
    /// the UTF-8 counts, blank and padded rows, demoted columns, restarts).
    pub converted: ConvertedTable,
}

/// A failure that was recorded in the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareFailure {
    pub code: &'static str,
    pub detail: String,
}

#[derive(Debug)]
pub enum PrepareOutcome {
    Ready(Box<PreparedTable>),
    /// The storage adapter reports no derived root: nothing was written, not
    /// even a registry row.
    Refused(PrepareStartError),
    /// Someone else holds the source, it is final, or it is ready already.
    NotClaimed,
    /// The row is gone or another job owns it: nothing was recorded.
    Cancelled,
    Failed(PrepareFailure),
}

pub fn encoding_name(e: Encoding) -> &'static str {
    match e {
        Encoding::Utf8 => "utf-8",
        Encoding::Windows1252 => "windows-1252",
    }
}

pub fn table_name(filename: &str) -> String {
    let stem = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    unique_table_names(&[stem]).remove(0)
}

pub fn report_of(table: &str, t: &ConvertedTable) -> ConversionReport {
    ConversionReport {
        table: table.to_string(),
        encoding: encoding_name(t.encoding).to_string(),
        replacements: t.replacements,
        utf8_valid_multibyte: t.utf8_valid_multibyte,
        utf8_invalid: t.utf8_invalid,
        blank_rows: t.blank_rows,
        blank_dropped: t.blank_dropped,
        padded_rows: t.padded_rows,
        restarts: t.restarts as u32,
        all_strings: t.all_strings,
        demoted_count: t.demoted.len() as u32,
        demoted: t
            .demoted
            .iter()
            .take(MAX_REPORTED_DEMOTED)
            .cloned()
            .collect(),
    }
}

/// Prepares `req.source_key` as table 0 of its source. Registry errors are
/// returned as they are: if the registry cannot be written nothing can be
/// recorded.
pub async fn prepare_csv(
    env: &PrepareEnv,
    req: &PrepareRequest,
) -> Result<PrepareOutcome, RegistryError> {
    let sink = match StoragePartSink::new(env.storage.clone(), &req.source_key) {
        Ok(sink) => Arc::new(sink),
        Err(e) => return Ok(PrepareOutcome::Refused(e)),
    };
    let owner = format!("prep-{}", uuid::Uuid::new_v4());
    let lease = chrono::Duration::from_std(env.budget).unwrap_or(chrono::Duration::days(1));
    let claim = env
        .registry
        .claim(ClaimRequest {
            source_key: req.source_key.clone(),
            source_bytes: i64::try_from(req.size_bytes).unwrap_or(i64::MAX),
            format_version: FORMAT_VERSION,
            owner: owner.clone(),
            lease: lease_for(lease),
            now: (env.clock)(),
        })
        .await?;
    if claim.is_none() {
        return Ok(PrepareOutcome::NotClaimed);
    }
    let control = ConvertControl::new();
    let source = StorageCsvSource::new(env.storage.clone(), &req.source_key);
    let converted = convert_csv_table_with(
        &source,
        sink.clone() as Arc<dyn PartSink>,
        0,
        env.writer,
        &control,
    )
    .await;
    // Every key any run may have written, as full storage keys.
    let keys_of =
        |paths: Vec<String>| -> Vec<String> { paths.iter().map(|p| sink.key_of(p)).collect() };
    match converted {
        Ok(table) => {
            let name = table_name(&req.filename);
            let manifest = Manifest::new(vec![TableInfo {
                name: name.clone(),
                rows: table.written.rows,
                parts: table.written.parts,
                columns: table.written.columns.clone(),
            }])
            .with_conversion(vec![report_of(&name, &table)]);
            let mut blob_keys = keys_of(table.blob_paths.clone());
            let manifest_key = sink.key_of(MANIFEST_PATH);
            blob_keys.push(manifest_key.clone());
            let stored = async {
                let tables_json = manifest.tables_json().map_err(|_| {
                    (
                        reason::TABLE_TOO_LARGE,
                        "the table list does not fit the registry row; export fewer columns"
                            .to_string(),
                    )
                })?;
                let json = manifest.to_json().map_err(|_| {
                    (
                        reason::INTERNAL,
                        "the manifest could not be written".to_string(),
                    )
                })?;
                sink.put(MANIFEST_PATH, Bytes::from(json))
                    .await
                    .map_err(|_| {
                        (
                            reason::STORAGE,
                            "the manifest could not be stored".to_string(),
                        )
                    })?;
                Ok::<_, (&'static str, String)>(tables_json)
            }
            .await;
            let tables_json = match stored {
                Ok(t) => t,
                Err((code, detail)) => {
                    return fail(env, req, &owner, code, detail, blob_keys).await;
                }
            };
            let mut live: Vec<String> = table.live_paths(0);
            live.push(MANIFEST_PATH.to_string());
            let prepared_bytes = sink.bytes_of(live.iter());
            let outcome = env
                .registry
                .complete(
                    &req.source_key,
                    &owner,
                    ReadyInfo {
                        manifest_key: manifest_key.clone(),
                        blob_keys: blob_keys.clone(),
                        tables_json,
                        prepared_bytes: i64::try_from(prepared_bytes).unwrap_or(i64::MAX),
                    },
                    (env.clock)(),
                )
                .await?;
            match outcome {
                TerminalOutcome::Cancelled => Ok(PrepareOutcome::Cancelled),
                TerminalOutcome::Written => {
                    let stale_keys = keys_of(table.stale_paths.clone());
                    Ok(PrepareOutcome::Ready(Box::new(PreparedTable {
                        manifest,
                        manifest_key,
                        blob_keys,
                        stale_keys,
                        prepared_bytes,
                        converted: table,
                    })))
                }
            }
        }
        Err(failure) => {
            // Every failure is recorded as internal here; the next slice tells
            // the reasons apart.
            let (code, detail) = (reason::INTERNAL, "the conversion failed".to_string());
            let keys = keys_of(failure.blob_paths);
            fail(env, req, &owner, code, detail, keys).await
        }
    }
}

/// Records the failure with every key that may exist, then deletes them.
async fn fail(
    env: &PrepareEnv,
    req: &PrepareRequest,
    owner: &str,
    code: &'static str,
    detail: String,
    keys: Vec<String>,
) -> Result<PrepareOutcome, RegistryError> {
    let outcome = env
        .registry
        .fail_with_blobs(&req.source_key, owner, code, &detail, &keys, (env.clock)())
        .await?;
    if outcome == TerminalOutcome::Cancelled {
        return Ok(PrepareOutcome::Cancelled);
    }
    // Best effort: the keys are tracked, so the cleanup pass removes what this
    // could not.
    if let Err(e) = env.storage.delete_derived(&req.source_key, &keys).await {
        tracing::warn!(
            target: "colmena::tabular_prepare",
            error = %e,
            "could not delete the objects of a failed preparation; the cleanup pass will"
        );
    }
    Ok(PrepareOutcome::Failed(PrepareFailure { code, detail }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::manifest::{ColumnInfo, ColumnType, MAX_NAME_CHARS};
    use crate::tabular_prepare::writer::TableWritten;

    #[test]
    fn the_table_is_named_after_the_file_without_its_extension() {
        assert_eq!(table_name("sales.csv"), "sales");
        assert_eq!(table_name("Q3 report.final.CSV"), "Q3 report.final");
        assert_eq!(table_name("data"), "data");
        // A name that is empty or only whitespace gets the generic one.
        assert_eq!(table_name(""), "sheet1");
        assert_eq!(table_name("  .csv"), "sheet1");
        assert_eq!(table_name(".csv"), ".csv");
        // Control characters are replaced and the length is bounded.
        assert_eq!(table_name("a\u{7}b.csv"), "a_b");
        assert_eq!(
            table_name(&format!("{}.csv", "x".repeat(200)))
                .chars()
                .count(),
            MAX_NAME_CHARS
        );
        // What comes out always passes the manifest's own check.
        for raw in ["a\u{0}b.csv", "\u{202e}x.csv", "../../etc/passwd.csv"] {
            let name = table_name(raw);
            assert!(
                !name.is_empty() && !name.chars().any(char::is_control),
                "{raw}"
            );
        }
    }

    fn converted(demoted: usize) -> ConvertedTable {
        ConvertedTable {
            written: TableWritten {
                rows: 10,
                parts: 1,
                columns: vec![ColumnInfo {
                    name: "a".into(),
                    column_type: ColumnType::String,
                    uncompressed_bytes: 1,
                    in_memory_bytes: 40,
                }],
            },
            restarts: 2,
            demoted: (0..demoted).map(|i| format!("c{i}")).collect(),
            all_strings: demoted > 0,
            encoding: Encoding::Windows1252,
            replacements: 0,
            utf8_valid_multibyte: 3,
            utf8_invalid: 90,
            blank_rows: 1,
            blank_dropped: 2,
            padded_rows: 4,
            blob_paths: Vec::new(),
            stale_paths: Vec::new(),
        }
    }

    #[test]
    fn the_report_carries_every_count_the_conversion_gave() {
        let r = report_of("sales", &converted(2));
        assert_eq!(r.table, "sales");
        assert_eq!(r.encoding, "windows-1252");
        assert_eq!(
            (r.replacements, r.utf8_valid_multibyte, r.utf8_invalid),
            (0, 3, 90)
        );
        assert_eq!((r.blank_rows, r.blank_dropped, r.padded_rows), (1, 2, 4));
        assert_eq!((r.restarts, r.all_strings), (2, true));
        assert_eq!((r.demoted_count, r.demoted.len()), (2, 2));
        let mut utf8 = converted(0);
        utf8.encoding = Encoding::Utf8;
        assert_eq!(report_of("t", &utf8).encoding, "utf-8");
    }

    #[test]
    fn a_wide_table_reports_the_exact_count_and_only_the_first_names() {
        let r = report_of("wide", &converted(5000));
        assert_eq!(r.demoted_count, 5000);
        assert_eq!(r.demoted.len(), MAX_REPORTED_DEMOTED);
        assert_eq!(r.demoted[0], "c0");
        assert_eq!(r.demoted[MAX_REPORTED_DEMOTED - 1], "c31");
        // The manifest accepts what the report produces.
        let manifest = Manifest::new(vec![crate::tabular_prepare::manifest::TableInfo {
            name: "wide".into(),
            rows: 10,
            parts: 1,
            columns: converted(0).written.columns,
        }])
        .with_conversion(vec![r]);
        assert!(manifest.to_json().is_ok());
    }

    #[test]
    fn the_time_budget_is_the_design_value_and_the_reasons_are_stable_snake_case() {
        assert_eq!(PREP_TIMEOUT, Duration::from_secs(300));
        let all = [
            reason::TIME,
            reason::STORAGE,
            reason::UNREADABLE_FILE,
            reason::TABLE_TOO_LARGE,
            reason::INTERNAL,
        ];
        for r in all {
            assert!(
                r.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{r}"
            );
        }
        let mut sorted = all.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len());
        assert_eq!(reason::TIME, "time");
    }
}

/// The driver's cases, written once over any registry: SQLite here, Postgres in
/// the ignored tests of `postgres_registry`.
#[cfg(test)]
pub(crate) mod cases {
    use super::*;
    use crate::tabular_prepare::prepare::fake::{root_of, PlacedStorage};
    use crate::tabular_prepare::registry::PrepareStatus;
    use chrono::TimeZone;

    /// A key no other run of the same database has used.
    pub(crate) fn fresh_source() -> String {
        format!("chat-attachments/u/s/{}.csv", uuid::Uuid::new_v4())
    }

    pub(crate) fn request(source: &str, size: u64) -> PrepareRequest {
        PrepareRequest {
            source_key: source.to_string(),
            mime_type: "text/csv".into(),
            filename: "sales.csv".into(),
            size_bytes: size,
        }
    }

    pub(crate) fn env(
        registry: Arc<dyn PreparationRegistry>,
        storage: Arc<PlacedStorage>,
    ) -> PrepareEnv {
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        PrepareEnv::new(registry, storage)
            .with_clock(Arc::new(move || now))
            .with_writer(WriterConfig {
                max_rows: 2,
                max_bytes: usize::MAX,
            })
    }

    fn five_rows() -> Vec<u8> {
        b"id,name\n1,ann\n2,bob\n3,cy\n4,di\n5,ed\n".to_vec()
    }

    fn part_keys(source: &str, n: usize) -> Vec<String> {
        let root = root_of(source);
        (0..n)
            .map(|i| format!("{root}/t0/part-{i:05}.parquet"))
            .collect()
    }

    fn objects_but_source(storage: &PlacedStorage, source: &str) -> Vec<String> {
        storage.keys().into_iter().filter(|k| k != source).collect()
    }

    pub(crate) async fn a_prepared_table_is_ready_with_the_manifest_stored_last(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let env = env(registry.clone(), storage.clone());
        let PrepareOutcome::Ready(table) = prepare_csv(&env, &request(source, 40)).await.unwrap()
        else {
            panic!("expected a ready table");
        };
        let root = root_of(source);
        let manifest_key = format!("{root}/manifest.json");
        // Layout: exactly the manifest and three parts of two rows (the last of one).
        let mut want = part_keys(source, 3);
        want.push(manifest_key.clone());
        let mut got = objects_but_source(&storage, source);
        got.sort();
        want.sort();
        assert_eq!(got, want);
        // The manifest is the last object stored.
        assert_eq!(storage.order.lock().unwrap().last(), Some(&manifest_key));
        // The row says what the storage holds.
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.status, PrepareStatus::Ready);
        assert_eq!(row.format_version, FORMAT_VERSION);
        assert_eq!(row.manifest_key.as_deref(), Some(manifest_key.as_str()));
        let mut tracked = row.blob_keys.clone();
        tracked.sort();
        assert_eq!(tracked, want);
        assert_eq!(table.manifest.tables[0].rows, 5);
        assert_eq!(table.manifest.tables[0].parts, 3);
        assert_eq!(
            row.tables_json.as_deref(),
            Some(table.manifest.tables_json().unwrap().as_str())
        );
        let stored_manifest = storage.objects.lock().unwrap()[&manifest_key].clone();
        assert_eq!(
            Manifest::from_json(&stored_manifest).unwrap(),
            table.manifest
        );
        let held: usize = objects_but_source(&storage, source)
            .iter()
            .map(|k| storage.objects.lock().unwrap()[k].len())
            .sum();
        assert_eq!(row.prepared_bytes, Some(held as i64));
        assert_eq!(table.prepared_bytes, held as u64);
        assert!(table.stale_keys.is_empty());
    }

    pub(crate) async fn without_a_derived_root_nothing_is_written_not_even_a_row(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        *storage.no_root.lock().unwrap() = true;
        let env = env(registry.clone(), storage.clone());
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        assert!(matches!(
            out,
            PrepareOutcome::Refused(PrepareStartError::NoDerivedRoot)
        ));
        assert!(registry.get(source).await.unwrap().is_none());
        assert_eq!(*storage.stores.lock().unwrap(), 0);
        assert_eq!(*storage.opens.lock().unwrap(), 0);
    }

    pub(crate) async fn the_result_carries_what_the_conversion_reports(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        // Windows-1252, a short row and a blank line.
        let body = b"name,v\ncaf\xE9,1\nshort\n\nzo\xEB,2\n".to_vec();
        let storage = PlacedStorage::with_source(source, body);
        let env = env(registry, storage.clone());
        let PrepareOutcome::Ready(table) = prepare_csv(&env, &request(source, 30)).await.unwrap()
        else {
            panic!("expected a ready table");
        };
        let c = &table.converted;
        assert_eq!(c.encoding, Encoding::Windows1252);
        assert_eq!((c.replacements, c.padded_rows, c.blank_dropped), (0, 1, 1));
        assert!(c.utf8_invalid >= 2 && c.utf8_valid_multibyte == 0);
        let r = &table.manifest.conversion[0];
        assert_eq!(r.table, "sales");
        assert_eq!(r.encoding, "windows-1252");
        assert_eq!(
            (
                r.replacements,
                r.padded_rows,
                r.blank_dropped,
                r.utf8_invalid
            ),
            (0, 1, 1, c.utf8_invalid)
        );
        // The stored manifest carries the same section.
        let key = table.manifest_key.clone();
        let json = storage.objects.lock().unwrap()[&key].clone();
        assert_eq!(
            Manifest::from_json(&json).unwrap().conversion,
            table.manifest.conversion
        );
    }

    pub(crate) async fn a_second_preparation_is_not_claimed_and_stores_nothing(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let env = env(registry, storage.clone());
        assert!(matches!(
            prepare_csv(&env, &request(source, 40)).await.unwrap(),
            PrepareOutcome::Ready(_)
        ));
        let stores = *storage.stores.lock().unwrap();
        assert!(matches!(
            prepare_csv(&env, &request(source, 40)).await.unwrap(),
            PrepareOutcome::NotClaimed
        ));
        assert_eq!(*storage.stores.lock().unwrap(), stores);
    }
}

#[cfg(test)]
mod registry_tests {
    use super::cases::*;
    use crate::tabular_prepare::registry::PreparationRegistry;
    use crate::tabular_prepare::sqlite_registry::SqlitePreparationRegistry;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::sync::Arc;
    use std::time::Duration;

    async fn sqlite() -> (Arc<dyn PreparationRegistry>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(dir.path().join("registry.db"))
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate!("migrations/sqlite")
            .run(&pool)
            .await
            .unwrap();
        (
            Arc::new(SqlitePreparationRegistry::from_pool(Arc::new(pool))),
            dir,
        )
    }

    macro_rules! sqlite_case {
        ($name:ident, $case:ident) => {
            #[tokio::test]
            async fn $name() {
                let (registry, _dir) = sqlite().await;
                $case(registry).await;
            }
        };
    }

    sqlite_case!(
        tabular_prepare_a_prepared_table_is_ready_with_the_manifest_stored_last,
        a_prepared_table_is_ready_with_the_manifest_stored_last
    );
    sqlite_case!(
        tabular_prepare_without_a_derived_root_nothing_is_written_not_even_a_row,
        without_a_derived_root_nothing_is_written_not_even_a_row
    );
    sqlite_case!(
        tabular_prepare_the_result_carries_what_the_conversion_reports,
        the_result_carries_what_the_conversion_reports
    );
    sqlite_case!(
        tabular_prepare_a_second_preparation_is_not_claimed_and_stores_nothing,
        a_second_preparation_is_not_claimed_and_stores_nothing
    );
}
