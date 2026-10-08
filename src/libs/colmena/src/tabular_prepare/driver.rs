//! The driver that prepares one large CSV: claim the registry row, convert the
//! source into Parquet parts on the host's storage, write the manifest LAST and
//! complete the row with it. Dark behind `COLMENA_LARGE_TABULAR`.
//!
//! Order matters. The manifest is the only thing that names the parts, so it is
//! put after every part is stored and the row is completed only after the
//! manifest is stored: a reader that finds a ready row finds every part it
//! lists.
//!
//! What holds about the objects. Before an object is put, its key is listed in the
//! registry row by an owner-guarded write (`track_blobs`): the part keys are
//! deterministic, so one write lists the next sixteen parts and the manifest, not
//! one write per part. So every object a preparation may have written is in the
//! row whatever happens next (a crash, a dropped future, a registry error at the
//! terminal write); a key listed and never written is harmless, deleting is
//! idempotent. A failure reads ownership, deletes the objects while the job still
//! holds the lease and only then records the failure (never a delete after it: the
//! row is claimable at once and a retry writes the same keys). If the registry cannot
//! say whether `complete` was applied nothing is deleted (the row may be ready), the
//! objects stay listed. Nothing but the
//! failure reason is user-visible, and it never carries a cell or a storage key.

use crate::storage::domain::{OutputStorageRepository, StorageError};
use crate::tabular_prepare::convert::{
    convert_csv_table_with, ConvertControl, ConvertError, ConvertedTable, TableError, TableFailure,
};
use crate::tabular_prepare::csv::{CsvError, Encoding};
use crate::tabular_prepare::manifest::{
    parse_part_path, part_path, unique_table_names, ConversionReport, Manifest, ManifestError,
    SkippedSheet, TableInfo, MANIFEST_PATH, MAX_PARTS, MAX_REPORTED_DEMOTED,
};
use crate::tabular_prepare::part_sink::{PartSink, SinkError};
use crate::tabular_prepare::ports::{
    NoopProgress, PrepareConfig, PrepareProgress, PrepareProgressInfo, PrepareRequest,
    PrepareRunner, ProgressState,
};
use crate::tabular_prepare::precheck::ArchiveError;
use crate::tabular_prepare::prepare::{
    PrepareStartError, StorageCsvSource, StoragePartSink, StorageXlsxSource,
};
use crate::tabular_prepare::registry::{
    ClaimRequest, PreparationRegistry, ReadyInfo, RegistryError, TerminalOutcome, FORMAT_VERSION,
};
use crate::tabular_prepare::writer::{WriterConfig, WriterError};
use crate::tabular_prepare::xlsx_convert::convert_xlsx;
use crate::tabular_prepare::xlsx_sheet::{MAX_CELLS, MAX_COLUMNS, MAX_ROWS};
use crate::tabular_prepare::xlsx_spool::{Cap, Invalid, XlsxError, MAX_XLSX_BYTES};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How often a running preparation reports its progress.
pub const PROGRESS_INTERVAL: Duration = Duration::from_secs(2);

/// The bound of one registry or storage step that comes after the budget (a
/// terminal write, a delete, an ownership read) and of the claim. A step that
/// does not answer in this time is given up on, never waited for forever.
pub const TERMINAL_STEP: Duration = Duration::from_secs(10);

/// The most bounded steps an owner takes outside its budget on its longest path:
/// the claim and the first progress report before it, and after it the ownership
/// read, the delete, the ownership read and the delete of a failed cleanup and the
/// failure write (a source found missing whose cleanup fails). Steps of a job that
/// no longer owns its row do not count: it writes nothing that needs the lease.
/// `the_longest_owner_path_ends_before_the_lease_does` runs this path with every
/// step taking its whole bound and fails when the path or the time changes.
pub const MAX_OWNER_STEPS: u32 = 7;

/// Added to the budget to make the lease of a claim: longer than the owner's steps
/// outside the budget, with room. It replaces the registry's 60 s grace, which the
/// longest path could outrun (7 steps of 10 s).
pub const JOB_GRACE: Duration = Duration::from_secs(90);

const _: () = assert!(TERMINAL_STEP.as_secs() * (MAX_OWNER_STEPS as u64) < JOB_GRACE.as_secs());

/// The most bytes of Parquet parts one preparation may store: 1 GiB, the size of the
/// largest source the product accepts (the upload limit). A prepared copy is smaller than
/// its source (a 1 GiB CSV prepared to 297 MB in the spike), so a legitimate source never
/// comes near it; what does is an input that decodes to far more than it weighs, such as
/// a 128 KiB shared string referenced by every cell of a million rows (about a terabyte of
/// text from a small upload). Time (300 s) ends that too, but only after hours of work are
/// paid for in storage; this ends it by what it stores, with its own reason.
pub const MAX_PREPARED_BYTES: u64 = 1024 * 1024 * 1024;

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
    /// The parts stored passed [`super::MAX_PREPARED_BYTES`]: the source decodes to far
    /// more than it weighs.
    pub const OUTPUT_TOO_LARGE: &str = "output_too_large";
    /// A workbook over a size limit (bytes, sheets, rows, columns, cells or shared
    /// text): the user is told to export it as CSV.
    pub const XLSX_TOO_LARGE: &str = "xlsx_too_large";
    /// A workbook whose archive is over a safety limit or inconsistent (a zip bomb,
    /// too many entries, headers that disagree). Written with an underscore: the
    /// ADP status endpoint accepts both `archive-limit` and `archive_limit` and has
    /// to be reconciled with this spelling.
    pub const ARCHIVE_LIMIT: &str = "archive_limit";
}

/// Source of "now". Read once per registry write.
pub type Clock = dyn Fn() -> DateTime<Utc> + Send + Sync;

/// How the driver waits out a duration: `tokio::time::sleep` in production, a
/// test's own trigger in tests, so no test depends on how long anything takes.
pub type Sleeper = dyn Fn(Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync;

/// What the driver needs.
pub struct PrepareEnv {
    pub registry: Arc<dyn PreparationRegistry>,
    pub storage: Arc<dyn OutputStorageRepository>,
    pub clock: Arc<Clock>,
    pub sleeper: Arc<Sleeper>,
    pub progress: Arc<dyn PrepareProgress>,
    pub budget: Duration,
    pub writer: WriterConfig,
    /// The largest workbook prepared, checked against the size the storage reports
    /// before a byte is read (a larger one fails with `xlsx_too_large`; 0 refuses
    /// every workbook).
    pub xlsx_max_bytes: u64,
    /// The most bytes of parts one preparation may store (see [`MAX_PREPARED_BYTES`]).
    pub max_prepared_bytes: u64,
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
            sleeper: Arc::new(|d| Box::pin(tokio::time::sleep(d))),
            progress: Arc::new(NoopProgress),
            budget: PREP_TIMEOUT,
            writer: WriterConfig::default(),
            xlsx_max_bytes: MAX_XLSX_BYTES,
            max_prepared_bytes: MAX_PREPARED_BYTES,
        }
    }

    pub fn with_clock(mut self, clock: Arc<Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_sleeper(mut self, sleeper: Arc<Sleeper>) -> Self {
        self.sleeper = sleeper;
        self
    }

    pub fn with_progress(mut self, progress: Arc<dyn PrepareProgress>) -> Self {
        self.progress = progress;
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

    pub fn with_max_prepared_bytes(mut self, max: u64) -> Self {
        self.max_prepared_bytes = max;
        self
    }

    pub fn with_xlsx_max_bytes(mut self, max: u64) -> Self {
        self.xlsx_max_bytes = max;
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
    /// Everything the conversion reported for each table, in manifest order (rows,
    /// parts, encoding, replacements, the UTF-8 counts, blank and padded rows,
    /// demoted columns, restarts). A CSV has one; a workbook one per sheet.
    pub tables: Vec<ConvertedTable>,
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
    /// The source no longer exists: what was written is removed and the row
    /// released, with no failure kept.
    SourceGone,
    Failed(PrepareFailure),
}

fn detail_of_csv(e: &CsvError) -> String {
    match e {
        CsvError::Empty => "the file is empty".into(),
        CsvError::UnsupportedEncoding(what) => format!("unsupported encoding: {what}"),
        CsvError::RecordTooLong { limit, .. } => format!("a record is longer than {limit} bytes"),
        CsvError::TooManyColumns { limit } => format!("the file has more than {limit} columns"),
        _ => "the file is not valid CSV".into(),
    }
}

/// The reason and the fixed detail of a failed conversion.
fn classify(e: &TableError) -> (&'static str, String) {
    use reason::*;
    match e {
        TableError::Convert(ConvertError::Csv(c)) => match c {
            CsvError::Io(_) => (STORAGE, "the source could not be read from storage".into()),
            CsvError::Cancelled | CsvError::InvalidLimits(_) => {
                (INTERNAL, "the conversion stopped unexpectedly".into())
            }
            other => (UNREADABLE_FILE, detail_of_csv(other)),
        },
        TableError::Convert(ConvertError::SourceUnavailable) => {
            (STORAGE, "the source could not be read from storage".into())
        }
        TableError::Convert(ConvertError::Manifest(ManifestError::ManifestTooLarge { .. }))
        | TableError::Writer(WriterError::Manifest(ManifestError::ManifestTooLarge { .. })) => (
            TABLE_TOO_LARGE,
            "the table list does not fit the registry row; export fewer columns".into(),
        ),
        TableError::Writer(WriterError::Sink(_)) => {
            (STORAGE, "a prepared object could not be stored".into())
        }
        TableError::Xlsx(e) => classify_xlsx(e),
        _ => (INTERNAL, "the conversion failed".into()),
    }
}

/// The reason and the fixed detail of a workbook that could not be read. No sheet
/// name, cell, key or library message goes in: the sentences are chosen by the
/// kind of error alone.
fn classify_xlsx(e: &XlsxError) -> (&'static str, String) {
    use reason::*;
    match e {
        XlsxError::Archive(ArchiveError::NotAnArchive) => {
            (UNREADABLE_FILE, "the file is not an xlsx workbook".into())
        }
        XlsxError::Archive(ArchiveError::Io) | XlsxError::Local => (
            INTERNAL,
            "the workbook could not be handled in local storage".into(),
        ),
        XlsxError::Archive(a) => (
            ARCHIVE_LIMIT,
            match a {
                ArchiveError::Unsupported => {
                    "the workbook's archive uses zip64, encryption or an unsupported method"
                }
                ArchiveError::TooManyEntries => "the workbook's archive has too many entries",
                ArchiveError::CentralDirectoryTooLarge => {
                    "the workbook's archive directory is too large"
                }
                ArchiveError::BadName => {
                    "the workbook's archive has an entry name that is not acceptable"
                }
                ArchiveError::DuplicateName => {
                    "the workbook's archive has two entries with one name"
                }
                ArchiveError::EntryTooLarge => "a part of the workbook expands past its size limit",
                ArchiveError::TotalTooLarge => "the workbook expands past its total size limit",
                ArchiveError::RatioTooLow => {
                    "a part of the workbook is compressed beyond the ratio limit"
                }
                ArchiveError::ImpossibleSizes => {
                    "a part of the workbook declares sizes its compression cannot produce"
                }
                _ => "the workbook's archive headers are inconsistent",
            }
            .into(),
        ),
        // Its own sentence: the cause is the number of sheets and columns together.
        XlsxError::TooLarge(Cap::TableList) => (
            TABLE_TOO_LARGE,
            "the table lists of all the sheets do not fit the registry row; export fewer sheets or columns"
                .into(),
        ),
        XlsxError::TooLarge(Cap::Manifest) => (
            TABLE_TOO_LARGE,
            "the manifest of the workbook (its tables, conversion reports and skipped sheets) is over its size limit; export fewer sheets or columns"
                .into(),
        ),
        XlsxError::TooLarge(cap) => (
            XLSX_TOO_LARGE,
            match cap {
                Cap::Bytes => BYTES_DETAIL.into(),
                Cap::Sheets => {
                    "the workbook has more sheets than the limit; export fewer sheets or use CSV"
                        .into()
                }
                Cap::Rows => format!("a sheet has more than {MAX_ROWS} rows; export it as CSV"),
                Cap::Columns => {
                    format!("a sheet has more than {MAX_COLUMNS} columns; export it as CSV")
                }
                Cap::Cells => {
                    format!("the workbook has more than {MAX_CELLS} cells; export it as CSV")
                }
                Cap::TableList | Cap::Manifest => unreachable!("classified above"),
                Cap::SharedStrings => {
                    "the workbook's text is over the limit; export it as CSV".into()
                }
            },
        ),
        XlsxError::Invalid(i) => (
            UNREADABLE_FILE,
            match i {
                Invalid::TokenTooLong | Invalid::CellTooLong | Invalid::RowTooLong => {
                    "a cell, a row or an element of the workbook is longer than the limit"
                }
                Invalid::BeyondHeader => "a row has a value past the last column of the header",
                Invalid::NoData => "the workbook has no sheet with data",
                _ => "the file is not a valid xlsx workbook",
            }
            .into(),
        ),
        XlsxError::SourceMissing | XlsxError::SourceUnavailable => {
            (STORAGE, "the source could not be read from storage".into())
        }
        XlsxError::Cancelled => (INTERNAL, "the conversion stopped unexpectedly".into()),
    }
}

/// The reason when the parts stored passed their cap: fixed words, no size of the file.
fn output_too_large() -> (&'static str, String) {
    (
        reason::OUTPUT_TOO_LARGE,
        "the prepared tables are larger than the limit; export fewer rows or columns".to_string(),
    )
}

/// The reason when the registry, not the storage, failed while the job listed keys
/// or confirmed its row: a defect or an outage on our side, never the file's.
fn registry_failure() -> (&'static str, String) {
    (
        reason::INTERNAL,
        "the registry could not confirm the preparation's row".to_string(),
    )
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

/// The in-process runner behind `InlineTrigger`: prepares the CSV or the xlsx a
/// request names, by its mime type. With the engine switch off it runs nothing (no
/// registry read, no storage call); any other source is logged and dropped.
pub struct CsvPrepareRunner {
    env: Arc<PrepareEnv>,
    enabled: bool,
}

impl CsvPrepareRunner {
    /// `config.large_tabular` is the engine switch, read once.
    pub fn new(env: Arc<PrepareEnv>, config: &PrepareConfig) -> Self {
        Self {
            env,
            enabled: config.large_tabular,
        }
    }
}

/// A short, stable identifier of a source for logs, derived from its key with
/// SHA-256: it lets one preparation be followed through the logs without the key
/// (which names a user and a file) being written, and it cannot be turned back.
pub(crate) fn opaque_id(source_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(source_key.as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// The mime type of an `.xlsx` workbook.
pub const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// Whether `mime` (parameters such as a charset ignored, case too) is `expected`.
fn is_mime(mime: &str, expected: &str) -> bool {
    mime.split(';')
        .next()
        .is_some_and(|m| m.trim().eq_ignore_ascii_case(expected))
}

#[async_trait]
impl PrepareRunner for CsvPrepareRunner {
    async fn run(&self, req: PrepareRequest) {
        if !self.enabled {
            return;
        }
        let source = opaque_id(&req.source_key);
        let prepared = if is_mime(&req.mime_type, "text/csv") {
            prepare_csv(&self.env, &req).await
        } else if is_mime(&req.mime_type, XLSX_MIME) {
            prepare_xlsx(&self.env, &req).await
        } else {
            tracing::warn!(
                target: "colmena::tabular_prepare",
                source = %source,
                "only CSV and xlsx sources are prepared; the request was dropped"
            );
            return;
        };
        match prepared {
            Ok(PrepareOutcome::Failed(f)) => tracing::warn!(
                target: "colmena::tabular_prepare",
                source = %source,
                reason = f.code,
                "the preparation failed"
            ),
            Ok(PrepareOutcome::Refused(_)) => tracing::error!(
                target: "colmena::tabular_prepare",
                source = %source,
                kind = "refused",
                "the preparation was refused"
            ),
            Ok(_) => {}
            Err(_) => tracing::error!(
                target: "colmena::tabular_prepare",
                source = %source,
                kind = "registry",
                "the preparation could not record its outcome"
            ),
        }
    }
}

/// The sink of a run: before every object it checks, with a read, that the
/// preparation still owns its row, and refuses to write when it does not. The
/// objects are written under deterministic keys, so a job that lost its row must
/// not write over those of whoever owns it now.
struct OwnedSink {
    inner: Arc<StoragePartSink>,
    registry: Arc<dyn PreparationRegistry>,
    clock: Arc<Clock>,
    source_key: String,
    owner: String,
    lost: AtomicBool,
    /// The registry itself failed (not storage) when listing keys or reading the row.
    registry_failed: AtomicBool,
    /// Per table, parts `0..n` are listed in the row already.
    tracked_parts: std::sync::Mutex<std::collections::HashMap<usize, usize>>,
    manifest_tracked: AtomicBool,
    /// Bytes of each part as last stored (a restart overwrites a part: it counts once),
    /// and the most allowed in all.
    stored: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    max_stored: u64,
    /// A part was refused for passing `max_stored`.
    too_big: AtomicBool,
}

/// How many part keys are listed in the row ahead of the part being written. The
/// part keys are deterministic, so they are known long before the parts exist: one
/// registry write lists the next sixteen (up to a gigabyte of output) and the
/// manifest, instead of one write per part. A key listed and never written is
/// harmless, deleting is idempotent; an object written and never listed is what
/// this prevents.
const TRACK_AHEAD: usize = 16;

/// [`TRACK_AHEAD`], for the tests of other modules.
#[cfg(test)]
pub(crate) const TRACK_AHEAD_FOR_TESTS: usize = TRACK_AHEAD;

impl OwnedSink {
    /// Lists in the row, owner-guarded, the keys that writing `path` needs listed,
    /// before it is written. `Ok(false)` means the row is no longer ours.
    async fn track_for(&self, path: &str) -> Result<bool, RegistryError> {
        let mut keys: Vec<String> = Vec::new();
        let mut upto = None;
        if let Some((table, part)) = parse_part_path(path) {
            let done = self
                .tracked_parts
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&table)
                .copied()
                .unwrap_or(0);
            if part >= done {
                let end = (part + TRACK_AHEAD).min(MAX_PARTS);
                keys.extend((part..end).filter_map(|p| part_path(table, p).ok()));
                upto = Some((table, end));
            }
        }
        let with_manifest = !self.manifest_tracked.load(Ordering::SeqCst);
        if with_manifest {
            keys.push(MANIFEST_PATH.to_string());
        }
        if keys.is_empty() {
            return self.registry_owns().await;
        }
        let full: Vec<String> = keys.iter().map(|k| self.inner.key_of(k)).collect();
        let outcome = self
            .registry
            .track_blobs(&self.source_key, &self.owner, &full, (self.clock)())
            .await?;
        if outcome == TerminalOutcome::Cancelled {
            return Ok(false);
        }
        if let Some((table, end)) = upto {
            self.tracked_parts
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(table, end);
        }
        self.manifest_tracked.store(true, Ordering::SeqCst);
        Ok(true)
    }

    async fn registry_owns(&self) -> Result<bool, RegistryError> {
        self.registry
            .still_owned(&self.source_key, &self.owner)
            .await
    }
}

#[async_trait]
impl PartSink for OwnedSink {
    async fn put(&self, path: &str, data: Bytes) -> Result<(), SinkError> {
        // Parts count against the cap; the manifest, which is small, does not.
        if path != MANIFEST_PATH {
            let mut stored = self.stored.lock().unwrap_or_else(|p| p.into_inner());
            stored.insert(path.to_string(), data.len() as u64);
            if stored.values().sum::<u64>() > self.max_stored {
                self.too_big.store(true, Ordering::SeqCst);
                return Err(SinkError(
                    "the prepared parts passed their size limit".into(),
                ));
            }
        }
        match self.track_for(path).await {
            Ok(true) => self.inner.put(path, data).await,
            Ok(false) => {
                self.lost.store(true, Ordering::SeqCst);
                Err(SinkError("the preparation no longer owns its row".into()))
            }
            Err(_) => {
                self.registry_failed.store(true, Ordering::SeqCst);
                Err(SinkError(
                    "the registry could not confirm the preparation's row".into(),
                ))
            }
        }
    }
}

/// Runs one step under [`TERMINAL_STEP`]; `None` when it did not answer in time.
async fn bounded<T>(env: &PrepareEnv, step: impl Future<Output = T>) -> Option<T> {
    tokio::select! {
        out = step => Some(out),
        () = (env.sleeper)(TERMINAL_STEP) => None,
    }
}

/// What a registry step that did not answer in time reports. Fixed text: it
/// carries no key and nothing from the backend.
fn no_answer() -> RegistryError {
    RegistryError::Backend("the registry did not answer in time".into())
}

/// A registry step under its bound.
async fn within<T>(
    env: &PrepareEnv,
    step: impl Future<Output = Result<T, RegistryError>>,
) -> Result<T, RegistryError> {
    bounded(env, step).await.unwrap_or_else(|| Err(no_answer()))
}

/// What a workbook over the byte cap is told (the file is never read).
const BYTES_DETAIL: &str = "the workbook is over the size limit; export it as CSV";

/// The named tables a conversion made, and the sheets it skipped.
type Named = (Vec<(String, ConvertedTable)>, Vec<SkippedSheet>);

/// What a source is converted from; everything else about a preparation (claim,
/// ownership, tracking, budget, terminal steps, failure sentences) is the same.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Csv,
    Xlsx,
}

/// Prepares `req.source_key` as table 0 of its source. Registry errors are
/// returned as they are: if the registry cannot be written nothing can be
/// recorded.
pub async fn prepare_csv(
    env: &PrepareEnv,
    req: &PrepareRequest,
) -> Result<PrepareOutcome, RegistryError> {
    prepare(env, req, Kind::Csv).await
}

/// Prepares the workbook `req.source_key`: one table per sheet that holds a value.
/// A workbook larger than `env.xlsx_max_bytes` fails with `xlsx_too_large` before
/// a byte is read. Otherwise as [`prepare_csv`].
pub async fn prepare_xlsx(
    env: &PrepareEnv,
    req: &PrepareRequest,
) -> Result<PrepareOutcome, RegistryError> {
    prepare(env, req, Kind::Xlsx).await
}

async fn prepare(
    env: &PrepareEnv,
    req: &PrepareRequest,
    kind: Kind,
) -> Result<PrepareOutcome, RegistryError> {
    let outcome = run_prepare(env, req, kind).await;
    let (state, done, total) = match &outcome {
        Ok(PrepareOutcome::Ready(_)) => {
            (ProgressState::Ready, req.size_bytes, Some(req.size_bytes))
        }
        Ok(PrepareOutcome::Failed(_)) => (ProgressState::Failed, 0, None),
        Ok(PrepareOutcome::Cancelled | PrepareOutcome::SourceGone) => {
            (ProgressState::Cancelled, 0, None)
        }
        // Nothing was started, or the registry could not be written.
        _ => return outcome,
    };
    bounded(
        env,
        env.progress
            .report(&req.source_key, PrepareProgressInfo { state, done, total }),
    )
    .await;
    outcome
}

async fn run_prepare(
    env: &PrepareEnv,
    req: &PrepareRequest,
    kind: Kind,
) -> Result<PrepareOutcome, RegistryError> {
    let stored = match StoragePartSink::new(env.storage.clone(), &req.source_key) {
        Ok(sink) => Arc::new(sink),
        Err(e) => return Ok(PrepareOutcome::Refused(e)),
    };
    let owner = format!("prep-{}", uuid::Uuid::new_v4());
    let lease =
        chrono::Duration::from_std(env.budget + JOB_GRACE).unwrap_or(chrono::Duration::days(1));
    let claim = within(
        env,
        env.registry.claim(ClaimRequest {
            source_key: req.source_key.clone(),
            source_bytes: i64::try_from(req.size_bytes).unwrap_or(i64::MAX),
            format_version: FORMAT_VERSION,
            owner: owner.clone(),
            lease,
            now: (env.clock)(),
        }),
    )
    .await?;
    if claim.is_none() {
        return Ok(PrepareOutcome::NotClaimed);
    }
    let sink = Arc::new(OwnedSink {
        inner: stored.clone(),
        registry: env.registry.clone(),
        clock: env.clock.clone(),
        source_key: req.source_key.clone(),
        owner: owner.clone(),
        lost: AtomicBool::new(false),
        registry_failed: AtomicBool::new(false),
        tracked_parts: std::sync::Mutex::new(std::collections::HashMap::new()),
        manifest_tracked: AtomicBool::new(false),
        stored: Default::default(),
        max_stored: env.max_prepared_bytes,
        too_big: AtomicBool::new(false),
    });
    if kind == Kind::Xlsx && req.size_bytes > env.xlsx_max_bytes {
        // Over the cap by the size the host declared: nothing is read.
        let detail = BYTES_DETAIL.to_string();
        return fail(env, req, &owner, reason::XLSX_TOO_LARGE, detail, Vec::new()).await;
    }
    let control = ConvertControl::new();
    let csv_source = StorageCsvSource::new(env.storage.clone(), &req.source_key);
    let xlsx_source =
        StorageXlsxSource::new(env.storage.clone(), &req.source_key, env.xlsx_max_bytes);
    let read = match kind {
        Kind::Csv => csv_source.bytes_read(),
        Kind::Xlsx => xlsx_source.bytes_read(),
    };
    // Under the total until the table is ready: the file is read before its output
    // is written, and a bar must not show complete while that goes on.
    let running = |done: u64| PrepareProgressInfo {
        state: ProgressState::Running,
        done: done.min(req.size_bytes.saturating_sub(1)),
        total: Some(req.size_bytes),
    };
    bounded(env, env.progress.report(&req.source_key, running(0))).await;
    // Progress is reported every [`PROGRESS_INTERVAL`] while the run goes on, to
    // the host's progress port and never to the registry.
    let ticker = async {
        loop {
            (env.sleeper)(PROGRESS_INTERVAL).await;
            let done = read.load(Ordering::Relaxed);
            env.progress.report(&req.source_key, running(done)).await;
        }
    };
    // Every key any run may have written, as full storage keys.
    let keys_of =
        |paths: Vec<String>| -> Vec<String> { paths.iter().map(|p| stored.key_of(p)).collect() };
    // One budget for everything up to the manifest: the conversion and the
    // manifest put. When it ends first the run is dropped, which cancels its
    // reader; the keys were listed in the row before each put.
    let budget = (env.sleeper)(env.budget);
    tokio::pin!(budget);
    let conversion = async {
        let sink = sink.clone() as Arc<dyn PartSink>;
        let named: Result<Named, TableFailure> = match kind {
            Kind::Csv => convert_csv_table_with(&csv_source, sink, 0, env.writer, &control)
                .await
                .map(|t| (vec![(table_name(&req.filename), t)], Vec::new())),
            Kind::Xlsx => convert_xlsx(&xlsx_source, sink, env.writer, &control)
                .await
                .map(|converted| {
                    let tables = converted
                        .table_names
                        .into_iter()
                        .zip(converted.tables)
                        .map(|(name, sheet)| (name, sheet.table))
                        .collect();
                    (tables, converted.skipped)
                }),
        };
        named
    };
    let converted = tokio::select! {
        done = conversion => done,
        never = ticker => {
            let _: std::convert::Infallible = never;
            unreachable!("the progress ticker never ends")
        }
        () = &mut budget => {
            let detail = "the preparation did not finish within its time budget".to_string();
            return fail(env, req, &owner, reason::TIME, detail, keys_of(control.paths())).await;
        }
    };
    match converted {
        Ok((tables, skipped)) => {
            let manifest = Manifest::new(
                tables
                    .iter()
                    .map(|(name, t)| TableInfo {
                        name: name.clone(),
                        rows: t.written.rows,
                        parts: t.written.parts,
                        columns: t.written.columns.clone(),
                    })
                    .collect(),
            )
            .with_conversion(tables.iter().map(|(name, t)| report_of(name, t)).collect())
            .with_skipped(skipped);
            let mut blob_keys = keys_of(
                tables
                    .iter()
                    .flat_map(|(_, t)| t.blob_paths.iter().cloned())
                    .collect(),
            );
            let manifest_key = stored.key_of(MANIFEST_PATH);
            blob_keys.push(manifest_key.clone());
            let manifest_put = async {
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
            };
            let manifest_put = tokio::select! {
                done = manifest_put => done,
                () = &mut budget => Err((
                    reason::TIME,
                    "the preparation did not finish within its time budget".to_string(),
                )),
            };
            let tables_json = match manifest_put {
                Ok(t) => t,
                Err((code, detail)) => {
                    if sink.lost.load(Ordering::SeqCst) {
                        return settle_lost(env, req, &blob_keys).await;
                    }
                    let (code, detail) = if sink.registry_failed.load(Ordering::SeqCst) {
                        registry_failure()
                    } else {
                        (code, detail)
                    };
                    return fail(env, req, &owner, code, detail, blob_keys).await;
                }
            };
            let mut live: Vec<String> = tables
                .iter()
                .enumerate()
                .flat_map(|(idx, (_, t))| t.live_paths(idx))
                .collect();
            live.push(MANIFEST_PATH.to_string());
            let prepared_bytes = stored.bytes_of(live.iter());
            let outcome = within(
                env,
                env.registry.complete(
                    &req.source_key,
                    &owner,
                    ReadyInfo {
                        manifest_key: manifest_key.clone(),
                        blob_keys: blob_keys.clone(),
                        tables_json,
                        prepared_bytes: i64::try_from(prepared_bytes).unwrap_or(i64::MAX),
                    },
                    (env.clock)(),
                ),
            )
            .await?;
            match outcome {
                TerminalOutcome::Cancelled => settle_lost(env, req, &blob_keys).await,
                TerminalOutcome::Written => {
                    let stale_keys = keys_of(
                        tables
                            .iter()
                            .flat_map(|(_, t)| t.stale_paths.iter().cloned())
                            .collect(),
                    );
                    Ok(PrepareOutcome::Ready(Box::new(PreparedTable {
                        manifest,
                        manifest_key,
                        blob_keys,
                        stale_keys,
                        prepared_bytes,
                        tables: tables.into_iter().map(|(_, t)| t).collect(),
                    })))
                }
            }
        }
        Err(failure) => {
            let keys = keys_of(failure.blob_paths.clone());
            if sink.lost.load(Ordering::SeqCst) {
                return settle_lost(env, req, &keys).await;
            }
            if matches!(
                failure.error,
                TableError::Convert(ConvertError::SourceMissing)
                    | TableError::Xlsx(XlsxError::SourceMissing)
            ) {
                return source_gone(env, req, &owner, keys).await;
            }
            let (code, detail) = if sink.registry_failed.load(Ordering::SeqCst) {
                registry_failure()
            } else if sink.too_big.load(Ordering::SeqCst) {
                output_too_large()
            } else {
                classify(&failure.error)
            };
            fail(env, req, &owner, code, detail, keys).await
        }
    }
}

/// The preparation lost its row: it records nothing. If the row is gone (the
/// source was deleted) what it wrote belongs to nobody and is removed; if
/// another job owns the row, the keys are the same deterministic ones and belong
/// to that job now, so they are left alone.
pub(crate) async fn settle_lost(
    env: &PrepareEnv,
    req: &PrepareRequest,
    keys: &[String],
) -> Result<PrepareOutcome, RegistryError> {
    if within(env, env.registry.get(&req.source_key))
        .await?
        .is_none()
    {
        delete_best_effort(env, req, keys, false).await;
    }
    Ok(PrepareOutcome::Cancelled)
}

/// The source does not exist (any more): nothing is worth keeping and no
/// failure is recorded. The objects are deleted first; if that fails the row
/// stays, failed, with the keys, so the cleanup pass can still reach them.
async fn source_gone(
    env: &PrepareEnv,
    req: &PrepareRequest,
    owner: &str,
    keys: Vec<String>,
) -> Result<PrepareOutcome, RegistryError> {
    // Ownership first: after a lease takeover the deterministic keys are the new
    // owner's, and a job that is not the owner deletes and releases nothing.
    if !within(env, env.registry.still_owned(&req.source_key, owner)).await? {
        return settle_lost(env, req, &keys).await;
    }
    if !keys.is_empty()
        && !matches!(
            bounded(env, env.storage.delete_derived(&req.source_key, &keys)).await,
            Some(Ok(()))
        )
    {
        let detail = "the objects of a deleted source could not be removed".to_string();
        return fail(env, req, owner, reason::STORAGE, detail, keys).await;
    }
    Ok(
        if within(env, env.registry.release(&req.source_key, owner)).await? {
            PrepareOutcome::SourceGone
        } else {
            PrepareOutcome::Cancelled
        },
    )
}

/// Deletes what a preparation wrote. The keys are listed in the row, so what
/// this cannot delete the cleanup pass will; the failure is logged as a fixed
/// sentence and a kind, never with the adapter's text or a key.
pub(crate) async fn delete_best_effort(
    env: &PrepareEnv,
    req: &PrepareRequest,
    keys: &[String],
    listed: bool,
) {
    // Never an empty list to the adapter: it may read one as "delete by prefix".
    if keys.is_empty() {
        return;
    }
    let kind = match bounded(env, env.storage.delete_derived(&req.source_key, keys)).await {
        Some(Ok(())) => return,
        Some(Err(e)) => storage_kind(&e),
        None => "no_answer",
    };
    // `listed`: the keys are in the row, so the cleanup pass will remove what this
    // could not. When the row is gone nothing lists them: the default delete stops at
    // the first error, so what is left stays until someone deletes by prefix.
    if listed {
        tracing::warn!(
            target: "colmena::tabular_prepare",
            kind,
            "could not delete prepared objects; the cleanup pass will"
        );
    } else {
        tracing::warn!(
            target: "colmena::tabular_prepare",
            kind,
            "could not delete prepared objects; no row lists them"
        );
    }
}

fn storage_kind(e: &StorageError) -> &'static str {
    match e {
        StorageError::BackendUnavailable(_) => "backend_unavailable",
        StorageError::InvalidInput(_) => "invalid_input",
        StorageError::UploadFailed(_) => "upload_failed",
        StorageError::CallbackFailed { .. } => "callback_failed",
    }
}

/// Deletes the objects of a failed preparation and then records the failure.
///
/// The order is the point. Once the failure is recorded the row is `failed`, its
/// lease cleared and the source claimable at once, and the objects have
/// deterministic keys a retry writes to: a delete after that could remove a part
/// the retry just wrote. So the delete comes first, while this job still holds the
/// lease, and nothing is deleted after the write. Before deleting, ownership is
/// read; a job that is not the owner deletes nothing unless the row is gone.
async fn fail(
    env: &PrepareEnv,
    req: &PrepareRequest,
    owner: &str,
    code: &'static str,
    detail: String,
    keys: Vec<String>,
) -> Result<PrepareOutcome, RegistryError> {
    if !within(env, env.registry.still_owned(&req.source_key, owner)).await? {
        return settle_lost(env, req, &keys).await;
    }
    // Best effort: the keys are listed in the row, so the cleanup pass removes
    // what this could not.
    // With nothing written there is nothing to delete, and an adapter may read an
    // empty list as "delete everything under the prefix".
    if !keys.is_empty() {
        delete_best_effort(env, req, &keys, true).await;
    }
    let outcome = within(
        env,
        env.registry
            .fail_with_blobs(&req.source_key, owner, code, &detail, &keys, (env.clock)()),
    )
    .await?;
    if outcome == TerminalOutcome::Cancelled {
        // The row was lost while the objects were being deleted: they are the
        // new owner's keys now or nobody's; deleting again would be a guess.
        return settle_lost(env, req, &[]).await;
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
    fn every_failure_has_a_reason_and_a_fixed_text_without_content() {
        use crate::tabular_prepare::part_sink::SinkError;
        let csv = |c: CsvError| TableError::Convert(ConvertError::Csv(c));
        let cases: Vec<(TableError, &str, &str)> = vec![
            (
                csv(CsvError::Empty),
                reason::UNREADABLE_FILE,
                "the file is empty",
            ),
            (
                csv(CsvError::UnsupportedEncoding("UTF-16".into())),
                reason::UNREADABLE_FILE,
                "unsupported encoding: UTF-16",
            ),
            (
                csv(CsvError::RecordTooLong {
                    record: 7,
                    limit: 1048576,
                }),
                reason::UNREADABLE_FILE,
                "a record is longer than 1048576 bytes",
            ),
            (
                csv(CsvError::TooManyColumns { limit: 16384 }),
                reason::UNREADABLE_FILE,
                "the file has more than 16384 columns",
            ),
            (
                csv(CsvError::Parse("row 3: secret cell".into())),
                reason::UNREADABLE_FILE,
                "the file is not valid CSV",
            ),
            (
                csv(CsvError::Io("https://x/secret-key".into())),
                reason::STORAGE,
                "the source could not be read from storage",
            ),
            (
                TableError::Convert(ConvertError::SourceUnavailable),
                reason::STORAGE,
                "the source could not be read from storage",
            ),
            (
                TableError::Writer(WriterError::Sink(SinkError("secret-key".into()))),
                reason::STORAGE,
                "a prepared object could not be stored",
            ),
            (
                TableError::Writer(WriterError::Manifest(ManifestError::ManifestTooLarge {
                    bytes: 70_000,
                    cap: 65_536,
                })),
                reason::TABLE_TOO_LARGE,
                "the table list does not fit the registry row; export fewer columns",
            ),
            (
                TableError::Convert(ConvertError::Manifest(ManifestError::ManifestTooLarge {
                    bytes: 70_000,
                    cap: 65_536,
                })),
                reason::TABLE_TOO_LARGE,
                "the table list does not fit the registry row; export fewer columns",
            ),
            (
                TableError::Convert(ConvertError::ReaderPanicked),
                reason::INTERNAL,
                "the conversion failed",
            ),
            (
                TableError::Convert(ConvertError::Cast("secret".into())),
                reason::INTERNAL,
                "the conversion failed",
            ),
            (
                csv(CsvError::Cancelled),
                reason::INTERNAL,
                "the conversion stopped unexpectedly",
            ),
            (
                csv(CsvError::InvalidLimits("secret".into())),
                reason::INTERNAL,
                "the conversion stopped unexpectedly",
            ),
        ];
        for (err, code, detail) in cases {
            let (c, d) = classify(&err);
            assert_eq!((c, d.as_str()), (code, detail), "{err:?}");
            assert!(!d.contains("secret"), "{err:?}");
        }
    }

    #[test]
    fn every_workbook_failure_has_a_reason_and_a_fixed_text_without_content() {
        use crate::tabular_prepare::xlsx_spool::{Cap, Invalid};
        let archive = |a: ArchiveError| TableError::Xlsx(XlsxError::Archive(a));
        let archive_cases = [
            (
                ArchiveError::Unsupported,
                "the workbook's archive uses zip64, encryption or an unsupported method",
            ),
            (
                ArchiveError::TooManyEntries,
                "the workbook's archive has too many entries",
            ),
            (
                ArchiveError::CentralDirectoryTooLarge,
                "the workbook's archive directory is too large",
            ),
            (
                ArchiveError::BadName,
                "the workbook's archive has an entry name that is not acceptable",
            ),
            (
                ArchiveError::DuplicateName,
                "the workbook's archive has two entries with one name",
            ),
            (
                ArchiveError::EntryTooLarge,
                "a part of the workbook expands past its size limit",
            ),
            (
                ArchiveError::TotalTooLarge,
                "the workbook expands past its total size limit",
            ),
            (
                ArchiveError::RatioTooLow,
                "a part of the workbook is compressed beyond the ratio limit",
            ),
            (
                ArchiveError::ImpossibleSizes,
                "a part of the workbook declares sizes its compression cannot produce",
            ),
            (
                ArchiveError::InconsistentHeaders,
                "the workbook's archive headers are inconsistent",
            ),
        ];
        for (a, detail) in archive_cases {
            let (code, d) = classify(&archive(a));
            assert_eq!((code, d.as_str()), (reason::ARCHIVE_LIMIT, detail), "{a:?}");
        }
        let x = |e: XlsxError| classify(&TableError::Xlsx(e));
        let cases: Vec<(XlsxError, &str, String)> = vec![
            (
                XlsxError::Archive(ArchiveError::NotAnArchive),
                reason::UNREADABLE_FILE,
                "the file is not an xlsx workbook".into(),
            ),
            (
                XlsxError::Archive(ArchiveError::Io),
                reason::INTERNAL,
                "the workbook could not be handled in local storage".into(),
            ),
            (
                XlsxError::Local,
                reason::INTERNAL,
                "the workbook could not be handled in local storage".into(),
            ),
            (
                XlsxError::TooLarge(Cap::Bytes),
                reason::XLSX_TOO_LARGE,
                BYTES_DETAIL.into(),
            ),
            (
                XlsxError::TooLarge(Cap::Sheets),
                reason::XLSX_TOO_LARGE,
                "the workbook has more sheets than the limit; export fewer sheets or use CSV"
                    .into(),
            ),
            (
                XlsxError::TooLarge(Cap::Rows),
                reason::XLSX_TOO_LARGE,
                "a sheet has more than 1048576 rows; export it as CSV".into(),
            ),
            (
                XlsxError::TooLarge(Cap::Columns),
                reason::XLSX_TOO_LARGE,
                "a sheet has more than 16384 columns; export it as CSV".into(),
            ),
            (
                XlsxError::TooLarge(Cap::Cells),
                reason::XLSX_TOO_LARGE,
                "the workbook has more than 50000000 cells; export it as CSV".into(),
            ),
            (
                XlsxError::TooLarge(Cap::Manifest),
                reason::TABLE_TOO_LARGE,
                "the manifest of the workbook (its tables, conversion reports and skipped sheets) is over its size limit; export fewer sheets or columns".into(),
            ),
            (
                XlsxError::TooLarge(Cap::TableList),
                reason::TABLE_TOO_LARGE,
                "the table lists of all the sheets do not fit the registry row; export fewer sheets or columns".into(),
            ),
            (
                XlsxError::TooLarge(Cap::SharedStrings),
                reason::XLSX_TOO_LARGE,
                "the workbook's text is over the limit; export it as CSV".into(),
            ),
            (
                XlsxError::Invalid(Invalid::Xml),
                reason::UNREADABLE_FILE,
                "the file is not a valid xlsx workbook".into(),
            ),
            (
                XlsxError::Invalid(Invalid::NoWorkbook),
                reason::UNREADABLE_FILE,
                "the file is not a valid xlsx workbook".into(),
            ),
            (
                XlsxError::Invalid(Invalid::BadCell),
                reason::UNREADABLE_FILE,
                "the file is not a valid xlsx workbook".into(),
            ),
            (
                XlsxError::Invalid(Invalid::CellTooLong),
                reason::UNREADABLE_FILE,
                "a cell, a row or an element of the workbook is longer than the limit".into(),
            ),
            (
                XlsxError::Invalid(Invalid::BeyondHeader),
                reason::UNREADABLE_FILE,
                "a row has a value past the last column of the header".into(),
            ),
            (
                XlsxError::Invalid(Invalid::NoData),
                reason::UNREADABLE_FILE,
                "the workbook has no sheet with data".into(),
            ),
            (
                XlsxError::SourceMissing,
                reason::STORAGE,
                "the source could not be read from storage".into(),
            ),
            (
                XlsxError::SourceUnavailable,
                reason::STORAGE,
                "the source could not be read from storage".into(),
            ),
            (
                XlsxError::Cancelled,
                reason::INTERNAL,
                "the conversion stopped unexpectedly".into(),
            ),
        ];
        for (e, code, detail) in cases {
            let (c, d) = x(e);
            assert_eq!((c, d.as_str()), (code, detail.as_str()), "{e:?}");
        }
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
            reason::XLSX_TOO_LARGE,
            reason::ARCHIVE_LIMIT,
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
        // The two Excel reasons are spelled with an underscore.
        assert_eq!(
            (reason::XLSX_TOO_LARGE, reason::ARCHIVE_LIMIT),
            ("xlsx_too_large", "archive_limit")
        );
    }
}

/// The driver's cases, written once over any registry: SQLite here, Postgres in
/// the ignored tests of `postgres_registry`.
#[cfg(test)]
pub(crate) mod cases {
    use super::*;
    use crate::tabular_prepare::prepare::fake::{root_of, PlacedStorage};
    use crate::tabular_prepare::registry::lease_for;
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

    async fn failed_row(
        registry: &Arc<dyn PreparationRegistry>,
        source: &str,
        outcome: PrepareOutcome,
    ) -> (
        PrepareFailure,
        crate::tabular_prepare::registry::PreparedRow,
    ) {
        let PrepareOutcome::Failed(f) = outcome else {
            panic!("expected a recorded failure, got {outcome:?}");
        };
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.status, PrepareStatus::Failed);
        assert_eq!(row.error_code.as_deref(), Some(f.code));
        assert_eq!(row.error_detail.as_deref(), Some(f.detail.as_str()));
        (f, row)
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
        // Every object is listed; so are the part keys listed ahead of the parts
        // (sixteen) that were never needed, which cleanup deletes as no-ops.
        for k in &want {
            assert!(row.blob_keys.contains(k), "{k} not tracked");
        }
        assert_eq!(row.blob_keys.len(), TRACK_AHEAD + 1);
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
        let c = &table.tables[0];
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

    pub(crate) async fn a_file_that_is_not_a_csv_fails_with_a_fixed_text_and_no_cell(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        for (body, detail) in [
            (&b""[..], "the file is empty"),
            (&b"\xFF\xFEa\0,\0b\0"[..], "unsupported encoding: UTF-16"),
            (
                &b"a,b\n1,a secret cell,3\n"[..],
                "the file is not valid CSV",
            ),
        ] {
            let storage = Arc::new(PlacedStorage::default());
            let env = env(registry.clone(), storage.clone());
            // A fresh key per case keeps the rows apart.
            let source = format!("chat-attachments/u/s/{}.csv", uuid::Uuid::new_v4());
            storage
                .objects
                .lock()
                .unwrap()
                .insert(source.clone(), Bytes::from(body.to_vec()));
            let out = prepare_csv(&env, &request(&source, 10)).await.unwrap();
            let PrepareOutcome::Failed(f) = out else {
                panic!("expected a failure");
            };
            assert_eq!(
                (f.code, f.detail.as_str()),
                (reason::UNREADABLE_FILE, detail)
            );
            let row = registry.get(&source).await.unwrap().unwrap();
            assert_eq!(row.error_code.as_deref(), Some("unreadable_file"));
            assert!(!row.error_detail.unwrap().contains("secret"));
            assert!(row.blob_keys.is_empty());
        }
    }

    pub(crate) async fn a_storage_failure_midway_tracks_every_key_tried_and_deletes_them(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // The first part is stored, the second put fails.
        *storage.fail_stores_from.lock().unwrap() = Some(1);
        let env = env(registry.clone(), storage.clone());
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        let (f, row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::STORAGE);
        assert!(!f.detail.contains("secret") && !f.detail.contains("chat-attachments"));
        // The key of the put that failed is tracked, with the one that worked.
        for k in part_keys(source, 2) {
            assert!(row.blob_keys.contains(&k), "{k} untracked");
        }
        // Everything tracked was deleted; the source is untouched.
        assert_eq!(storage.keys(), vec![source.to_string()]);
        for k in part_keys(source, 2) {
            assert!(storage.deleted.lock().unwrap().contains(&k));
        }
        assert!(!storage
            .deleted
            .lock()
            .unwrap()
            .contains(&source.to_string()));
    }

    pub(crate) async fn a_manifest_that_could_not_be_stored_never_makes_the_row_ready(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // Three parts are stored; the manifest is the fourth put and fails.
        *storage.fail_stores_from.lock().unwrap() = Some(3);
        let env = env(registry.clone(), storage.clone());
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        let (f, row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::STORAGE);
        assert_eq!(row.manifest_key, None);
        let manifest_key = format!("{}/manifest.json", root_of(source));
        assert!(row.blob_keys.contains(&manifest_key));
        assert_eq!(row.blob_keys.len(), TRACK_AHEAD + 1);
        assert_eq!(storage.keys(), vec![source.to_string()]);
    }

    /// A sleeper that records what it was asked for and fires when told.
    pub(crate) struct Gate {
        /// Durations asked for, other than the progress interval.
        pub asked: std::sync::Mutex<Vec<Duration>>,
        /// Ends the time budget.
        pub fire: tokio::sync::Notify,
        /// Ends one progress interval.
        pub tick: tokio::sync::Notify,
        /// Ends the bound of one terminal step.
        pub step: tokio::sync::Notify,
    }

    pub(crate) fn gated(env: PrepareEnv) -> (PrepareEnv, Arc<Gate>) {
        let gate = Arc::new(Gate {
            asked: std::sync::Mutex::new(Vec::new()),
            fire: tokio::sync::Notify::new(),
            tick: tokio::sync::Notify::new(),
            step: tokio::sync::Notify::new(),
        });
        let g = gate.clone();
        let env = env.with_sleeper(Arc::new(move |d| {
            let g = g.clone();
            Box::pin(async move {
                if d == PROGRESS_INTERVAL {
                    g.tick.notified().await;
                } else if d == TERMINAL_STEP {
                    g.step.notified().await;
                } else {
                    g.asked.lock().unwrap().push(d);
                    g.fire.notified().await;
                }
            })
        }));
        (env, gate)
    }

    pub(crate) async fn the_budget_ends_a_stuck_run_with_the_time_reason_and_removes_its_output(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // The first part is stored; the second put never completes.
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        let (env, gate) =
            gated(env(registry.clone(), storage.clone()).with_budget(Duration::from_secs(300)));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.hung.notified().await;
        gate.fire.notify_one();
        let out = run.await.unwrap();
        let (f, row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::TIME);
        // Asked for exactly the budget, once.
        assert_eq!(*gate.asked.lock().unwrap(), vec![Duration::from_secs(300)]);
        // The part that was stored and the one that hung are tracked, then removed.
        for k in part_keys(source, 2) {
            assert!(row.blob_keys.contains(&k), "{k} untracked");
            assert!(storage.deleted.lock().unwrap().contains(&k));
        }
        assert_eq!(storage.keys(), vec![source.to_string()]);
        assert_eq!(row.manifest_key, None);
    }

    pub(crate) async fn a_source_that_never_yields_ends_with_the_time_reason(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        *storage.stall_source.lock().unwrap() = true;
        let (env, gate) = gated(env(registry.clone(), storage.clone()));
        gate.fire.notify_one();
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        let (f, row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::TIME);
        assert_eq!(
            (f.detail.as_str(), row.blob_keys.len()),
            ("the preparation did not finish within its time budget", 0)
        );
        assert_eq!(storage.keys(), vec![source.to_string()]);
    }

    /// Another job takes the row of `source`.
    async fn take_over(registry: &Arc<dyn PreparationRegistry>, source: &str) {
        registry.delete(source).await.unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let claim = ClaimRequest {
            source_key: source.to_string(),
            source_bytes: 1,
            format_version: FORMAT_VERSION,
            owner: "other".into(),
            lease: lease_for(chrono::Duration::seconds(300)),
            now,
        };
        assert!(registry.claim(claim).await.unwrap().is_some());
    }

    /// Runs a preparation whose second put hangs, lets `during` change the
    /// registry while it hangs, then lets it go on.
    async fn run_hung_at<F, Fut>(
        registry: &Arc<dyn PreparationRegistry>,
        storage: &Arc<PlacedStorage>,
        source: &str,
        hang_at: usize,
        during: F,
    ) -> PrepareOutcome
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ()>,
    {
        *storage.hang_stores_from.lock().unwrap() = Some(hang_at);
        let env = env(registry.clone(), storage.clone());
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.hung.notified().await;
        during().await;
        storage.release_hang.notify_one();
        run.await.unwrap()
    }

    pub(crate) async fn a_job_whose_row_was_deleted_stops_before_its_next_part_and_leaves_no_row(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let out = run_hung_at(&registry, &storage, source, 1, || async {
            registry.delete(source).await.unwrap();
        })
        .await;
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        // The second part went through (it was in flight), the third was never tried.
        assert_eq!(*storage.stores.lock().unwrap(), 2);
        // Nothing is recorded, and what it wrote belongs to nobody: removed.
        assert!(registry.get(source).await.unwrap().is_none());
        assert_eq!(storage.keys(), vec![source.to_string()]);
        for k in part_keys(source, 2) {
            assert!(storage.deleted.lock().unwrap().contains(&k));
        }
    }

    pub(crate) async fn a_job_whose_lease_was_taken_stops_and_leaves_the_new_owners_row_and_objects(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let out = run_hung_at(&registry, &storage, source, 1, || async {
            take_over(&registry, source).await;
        })
        .await;
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        assert_eq!(*storage.stores.lock().unwrap(), 2);
        // The new owner's row is untouched: running, not failed, nothing deleted
        // (the keys are the deterministic ones it writes now).
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.status, PrepareStatus::Running);
        assert_eq!(row.lease_owner.as_deref(), Some("other"));
        assert_eq!(row.error_code, None);
        assert!(storage.deleted.lock().unwrap().is_empty());
        assert_eq!(objects_but_source(&storage, source).len(), 2);
    }

    pub(crate) async fn a_completion_that_finds_the_row_gone_is_cancelled_and_never_ready(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // Three parts, then the manifest is the fourth put: the row is deleted
        // while it is being stored, after the ownership check before it passed.
        let out = run_hung_at(&registry, &storage, source, 3, || async {
            registry.delete(source).await.unwrap();
        })
        .await;
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        assert!(registry.get(source).await.unwrap().is_none());
        assert_eq!(storage.keys(), vec![source.to_string()]);
    }

    pub(crate) async fn an_unopenable_source_releases_the_row_without_a_failure(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = Arc::new(PlacedStorage::default());
        let env = env(registry.clone(), storage.clone());
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        assert!(matches!(out, PrepareOutcome::SourceGone), "{out:?}");
        assert!(registry.get(source).await.unwrap().is_none());
        assert_eq!(*storage.stores.lock().unwrap(), 0);
    }

    pub(crate) async fn a_source_deleted_during_a_restart_removes_what_was_written_and_releases_the_row(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let mut first = b"id\n".to_vec();
        for i in 0..10_000 {
            first.extend_from_slice(format!("{i}\n").as_bytes());
        }
        first.extend_from_slice(b"late\n");
        let storage = PlacedStorage::with_source(source, first);
        *storage.remove_source_on_second_open.lock().unwrap() = true;
        let env = env(registry.clone(), storage.clone()).with_writer(WriterConfig {
            max_rows: 2000,
            max_bytes: usize::MAX,
        });
        let out = prepare_csv(&env, &request(source, 60_000)).await.unwrap();
        assert!(matches!(out, PrepareOutcome::SourceGone), "{out:?}");
        assert!(registry.get(source).await.unwrap().is_none());
        // The four parts the first run wrote are gone, and so is the source.
        assert!(storage.keys().is_empty());
        for k in part_keys(source, 4) {
            assert!(storage.deleted.lock().unwrap().contains(&k));
        }
    }

    /// Remembers every report and says when one arrives.
    #[derive(Default)]
    pub(crate) struct Recording {
        pub seen: std::sync::Mutex<Vec<PrepareProgressInfo>>,
        pub arrived: tokio::sync::Notify,
        pub slow: std::sync::Mutex<Option<Arc<crate::tabular_prepare::registry_faults::Slow>>>,
    }

    #[async_trait]
    impl PrepareProgress for Recording {
        async fn report(&self, _key: &str, info: PrepareProgressInfo) {
            if let Some(s) = self.slow.lock().unwrap().clone() {
                s.pass("progress");
            }
            self.seen.lock().unwrap().push(info);
            self.arrived.notify_one();
        }
        async fn read(&self, _key: &str) -> Option<PrepareProgressInfo> {
            self.seen.lock().unwrap().last().cloned()
        }
    }

    /// Runs a preparation held at its second put, ticks the progress interval once
    /// and returns the report that tick produced, checking the registry was not
    /// written for it. A report that never comes fails the test instead of hanging it.
    async fn one_tick(
        registry: &Arc<dyn PreparationRegistry>,
        declared: u64,
    ) -> PrepareProgressInfo {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let progress = Arc::new(Recording::default());
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        let (env, gate) =
            gated(env(registry.clone(), storage.clone()).with_progress(progress.clone()));
        let req = request(source, declared);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.hung.notified().await;
        let before = registry.get(source).await.unwrap().unwrap();
        gate.tick.notify_one();
        // The start report, then the tick's.
        let arrived = async {
            while progress.seen.lock().unwrap().len() < 2 {
                progress.arrived.notified().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(30), arrived)
            .await
            .expect("the tick produced no report");
        let tick = progress.seen.lock().unwrap()[1].clone();
        // The registry was not written for it.
        assert_eq!(registry.get(source).await.unwrap().unwrap(), before);
        storage.release_hang.notify_one();
        assert!(matches!(run.await.unwrap(), PrepareOutcome::Ready(_)));
        tick
    }

    pub(crate) async fn progress_is_reported_every_interval_to_the_port_and_never_written_to_the_registry(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        // The whole file was read (the sample reads it before any part).
        let read = five_rows().len() as u64;
        let tick = one_tick(&registry, 40).await;
        assert_eq!(tick.state, ProgressState::Running);
        assert_eq!((tick.done, tick.total), (read, Some(40)));
        // A declared size smaller than what was read never makes done pass total.
        // Done stays under the total until the preparation is ready, so a bar
        // never shows complete while the output is still being written.
        let tick = one_tick(&registry, read).await;
        assert_eq!((tick.done, tick.total), (read - 1, Some(read)));
        // A declared size smaller than what was read is clamped the same way.
        let tick = one_tick(&registry, 20).await;
        assert_eq!((tick.done, tick.total), (19, Some(20)));
    }

    pub(crate) async fn a_finished_preparation_reports_its_final_state_to_the_port(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let progress = Arc::new(Recording::default());
        // Ready.
        let ready = fresh_source();
        let storage = PlacedStorage::with_source(&ready, five_rows());
        let env1 = env(registry.clone(), storage).with_progress(progress.clone());
        assert!(matches!(
            prepare_csv(&env1, &request(&ready, 40)).await.unwrap(),
            PrepareOutcome::Ready(_)
        ));
        assert_eq!(
            progress.read("").await,
            Some(PrepareProgressInfo {
                state: ProgressState::Ready,
                done: 40,
                total: Some(40)
            })
        );
        // Failed.
        let source = fresh_source();
        let storage = PlacedStorage::with_source(&source, Vec::new());
        let env2 = env(registry.clone(), storage).with_progress(progress.clone());
        assert!(matches!(
            prepare_csv(&env2, &request(&source, 0)).await.unwrap(),
            PrepareOutcome::Failed(_)
        ));
        assert_eq!(
            progress.read("").await.unwrap().state,
            ProgressState::Failed
        );
        // Cancelled: the source does not exist.
        let source = fresh_source();
        let env3 = env(registry.clone(), Arc::new(PlacedStorage::default()))
            .with_progress(progress.clone());
        assert!(matches!(
            prepare_csv(&env3, &request(&source, 40)).await.unwrap(),
            PrepareOutcome::SourceGone
        ));
        assert_eq!(
            progress.read("").await.unwrap().state,
            ProgressState::Cancelled
        );
        // Not claimed: nothing is reported for it.
        let reports = progress.seen.lock().unwrap().len();
        assert!(matches!(
            prepare_csv(&env1, &request(&ready, 40)).await.unwrap(),
            PrepareOutcome::NotClaimed
        ));
        assert_eq!(progress.seen.lock().unwrap().len(), reports);
    }

    pub(crate) async fn the_inline_trigger_runs_a_csv_request_through_the_driver(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::ports::{InlineTrigger, PrepareTrigger};
        let progress = Arc::new(Recording::default());
        for mime in ["text/csv", "Text/CSV; charset=utf-8"] {
            let source = fresh_source();
            let storage = PlacedStorage::with_source(&source, five_rows());
            let env = env(registry.clone(), storage).with_progress(progress.clone());
            let config = PrepareConfig::from_switch(Some("on"));
            let runner = Arc::new(CsvPrepareRunner::new(Arc::new(env), &config));
            let trigger = InlineTrigger::new(runner);
            let mut req = request(&source, 40);
            req.mime_type = mime.to_string();
            let before = progress.seen.lock().unwrap().len();
            trigger.request(req).await.unwrap();
            // The request runs detached: wait for its final report, with a bound.
            let done = async {
                loop {
                    let seen = progress.seen.lock().unwrap().len();
                    if seen > before
                        && progress.read("").await.unwrap().state == ProgressState::Ready
                    {
                        break;
                    }
                    progress.arrived.notified().await;
                }
            };
            tokio::time::timeout(Duration::from_secs(30), done)
                .await
                .expect("the request never finished");
            let row = registry.get(&source).await.unwrap().unwrap();
            assert_eq!(row.status, PrepareStatus::Ready, "{mime}");
        }
    }

    pub(crate) async fn with_the_switch_off_or_another_mime_the_runner_touches_nothing(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        // The xlsx mime is routed to `prepare_xlsx` (see `driver_xlsx_tests`); the
        // legacy `.xls` one is not an accepted type and is dropped like any other.
        for (switch, mime) in [
            ("off", "text/csv"),
            ("off", XLSX_MIME),
            ("on", "application/vnd.ms-excel"),
            ("on", "text/csvx"),
            ("on", ""),
        ] {
            let source = fresh_source();
            let storage = PlacedStorage::with_source(&source, five_rows());
            let env = env(registry.clone(), storage.clone());
            let config = PrepareConfig::from_switch(Some(switch));
            let runner = CsvPrepareRunner::new(Arc::new(env), &config);
            let mut req = request(&source, 40);
            req.mime_type = mime.to_string();
            runner.run(req).await;
            assert!(
                registry.get(&source).await.unwrap().is_none(),
                "{switch} {mime}"
            );
            assert_eq!(*storage.opens.lock().unwrap(), 0);
            assert_eq!(*storage.stores.lock().unwrap(), 0);
        }
    }

    pub(crate) async fn dropping_the_prepare_future_mid_run_leaves_every_stored_object_tracked_in_the_row(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // The second put never completes; the whole preparation is then dropped.
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        let env = env(registry.clone(), storage.clone());
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await });
        storage.hung.notified().await;
        run.abort();
        let _ = run.await;
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.status, PrepareStatus::Running);
        // Every object in storage is listed, and so are the one in flight and
        // the manifest that was never reached.
        let manifest = format!("{}/manifest.json", root_of(source));
        for k in objects_but_source(&storage, source) {
            assert!(row.blob_keys.contains(&k), "{k} stored but not tracked");
        }
        for k in part_keys(source, 2).into_iter().chain([manifest]) {
            assert!(row.blob_keys.contains(&k), "{k} not tracked ahead");
        }
    }

    pub(crate) async fn a_part_beyond_the_first_batch_is_tracked_before_its_put_a_batch_ahead(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        // Forty-two rows of two per part: twenty-one parts.
        let mut body = b"id\n".to_vec();
        for i in 0..42 {
            body.extend_from_slice(format!("{i}\n").as_bytes());
        }
        let storage = PlacedStorage::with_source(source, body);
        // The seventeenth put (part 16) is the first one past the first batch.
        *storage.hang_stores_from.lock().unwrap() = Some(TRACK_AHEAD);
        let env = env(registry.clone(), storage.clone());
        let req = request(source, 100);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await });
        storage.hung.notified().await;
        run.abort();
        let _ = run.await;
        let row = registry.get(source).await.unwrap().unwrap();
        // Part 16 is not stored yet, and it is listed with the whole second batch.
        assert_eq!(objects_but_source(&storage, source).len(), TRACK_AHEAD);
        for k in part_keys(source, 2 * TRACK_AHEAD) {
            assert!(row.blob_keys.contains(&k), "{k} not tracked ahead");
        }
        assert_eq!(row.blob_keys.len(), 2 * TRACK_AHEAD + 1);
    }

    pub(crate) async fn a_row_that_vanishes_before_the_first_tracking_write_stops_the_job_before_any_object(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        use std::sync::atomic::Ordering::SeqCst;
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let faulty = FaultyRegistry::new(registry.clone());
        faulty.faults.delete_before_track.store(true, SeqCst);
        let env = env(faulty, storage.clone());
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        assert_eq!(*storage.stores.lock().unwrap(), 0);
        assert!(registry.get(source).await.unwrap().is_none());
    }

    pub(crate) async fn a_row_deleted_before_the_budget_ends_the_run_still_gets_its_objects_deleted(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        let (env, gate) = gated(env(registry.clone(), storage.clone()));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.hung.notified().await;
        registry.delete(source).await.unwrap();
        gate.fire.notify_one();
        let out = run.await.unwrap();
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        // The first part was stored; with no row it belongs to nobody.
        assert_eq!(storage.keys(), vec![source.to_string()]);
        assert!(registry.get(source).await.unwrap().is_none());
    }

    pub(crate) async fn a_row_deleted_before_a_storage_failure_still_gets_its_objects_deleted(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // The second put is held; the row goes; the put then fails.
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        *storage.fail_stores_from.lock().unwrap() = Some(1);
        let out = run_hung_at(&registry, &storage, source, 1, || async {
            registry.delete(source).await.unwrap();
        })
        .await;
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        assert_eq!(storage.keys(), vec![source.to_string()]);
        assert!(registry.get(source).await.unwrap().is_none());
    }

    /// A late word forces a second read of the file; `during` runs while that
    /// second open is stopped.
    async fn run_paused_at_the_second_open<F, Fut>(
        registry: &Arc<dyn PreparationRegistry>,
        storage: &Arc<PlacedStorage>,
        source: &str,
        during: F,
    ) -> PrepareOutcome
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ()>,
    {
        *storage.pause_second_open.lock().unwrap() = true;
        let env = env(registry.clone(), storage.clone()).with_writer(WriterConfig {
            max_rows: 2000,
            max_bytes: usize::MAX,
        });
        let req = request(source, 60_000);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.second_open_reached.notified().await;
        during().await;
        storage.second_open_go.notify_one();
        run.await.unwrap()
    }

    fn late_word_file() -> Vec<u8> {
        let mut body = b"id\n".to_vec();
        for i in 0..10_000 {
            body.extend_from_slice(format!("{i}\n").as_bytes());
        }
        body.extend_from_slice(b"late\n");
        body
    }

    pub(crate) async fn a_row_deleted_before_a_source_read_failure_still_gets_its_objects_deleted(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, late_word_file());
        *storage.fail_second_open.lock().unwrap() = true;
        let out = run_paused_at_the_second_open(&registry, &storage, source, || async {
            registry.delete(source).await.unwrap();
        })
        .await;
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        // The four parts the first run wrote are removed.
        assert_eq!(storage.keys(), vec![source.to_string()]);
        assert!(registry.get(source).await.unwrap().is_none());
    }

    pub(crate) async fn a_lease_taken_before_the_source_turns_out_missing_deletes_nothing_and_releases_nothing(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, late_word_file());
        *storage.remove_source_on_second_open.lock().unwrap() = true;
        let out = run_paused_at_the_second_open(&registry, &storage, source, || async {
            take_over(&registry, source).await;
        })
        .await;
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        // The new owner's row stays, and the keys it writes are not touched.
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.lease_owner.as_deref(), Some("other"));
        assert!(storage.deleted.lock().unwrap().is_empty());
        assert_eq!(objects_but_source(&storage, source).len(), 4);
    }

    pub(crate) async fn a_row_taken_before_the_budget_ends_the_run_keeps_the_new_owners_objects(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        let (env, gate) = gated(env(registry.clone(), storage.clone()));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.hung.notified().await;
        take_over(&registry, source).await;
        gate.fire.notify_one();
        let out = run.await.unwrap();
        assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
        assert!(storage.deleted.lock().unwrap().is_empty());
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.lease_owner.as_deref(), Some("other"));
        assert_eq!(row.error_code, None);
    }

    /// Waits for a preparation that must end by its own bounds; a bound that does
    /// not hold fails the test instead of hanging it.
    async fn joined(run: tokio::task::JoinHandle<PrepareOutcome>) -> PrepareOutcome {
        tokio::time::timeout(Duration::from_secs(30), run)
            .await
            .expect("the job outlived its bound")
            .unwrap()
    }

    pub(crate) async fn a_manifest_put_that_never_completes_is_ended_by_the_same_budget(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // Three parts are stored; the manifest, the fourth put, never completes.
        *storage.hang_stores_from.lock().unwrap() = Some(3);
        let (env, gate) = gated(env(registry.clone(), storage.clone()));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.hung.notified().await;
        gate.fire.notify_one();
        let out = joined(run).await;
        let (f, row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::TIME);
        // One budget for the whole job: it was asked for once.
        assert_eq!(*gate.asked.lock().unwrap(), vec![PREP_TIMEOUT]);
        assert_eq!(row.manifest_key, None);
        assert_eq!(storage.keys(), vec![source.to_string()]);
    }

    pub(crate) async fn a_completion_that_never_returns_is_ended_by_its_own_bound_and_deletes_nothing(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        use std::sync::atomic::Ordering::SeqCst;
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let faulty = FaultyRegistry::new(registry.clone());
        faulty.faults.hang_complete.store(true, SeqCst);
        let (env, gate) = gated(env(faulty.clone(), storage.clone()));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await });
        faulty.faults.complete_reached.notified().await;
        gate.step.notify_one();
        let out = tokio::time::timeout(Duration::from_secs(30), run)
            .await
            .expect("the job outlived its bound")
            .unwrap();
        assert!(out.is_err(), "{out:?}");
        // Whether it was applied is unknown: nothing deleted, everything listed.
        assert!(storage.deleted.lock().unwrap().is_empty());
        let row = registry.get(source).await.unwrap().unwrap();
        for k in objects_but_source(&storage, source) {
            assert!(row.blob_keys.contains(&k), "{k} not tracked");
        }
    }

    pub(crate) async fn a_delete_that_never_returns_does_not_hold_the_failure_back(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // The manifest cannot be stored, and the cleanup that follows never returns.
        *storage.fail_stores_from.lock().unwrap() = Some(3);
        *storage.hang_delete.lock().unwrap() = true;
        let (env, gate) = gated(env(registry.clone(), storage.clone()));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        storage.delete_reached.notified().await;
        gate.step.notify_one();
        let out = joined(run).await;
        let (f, _row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::STORAGE);
    }

    pub(crate) async fn the_claimed_lease_is_the_budget_plus_the_job_grace(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        *storage.hang_stores_from.lock().unwrap() = Some(1);
        let env = env(registry.clone(), storage.clone()).with_budget(Duration::from_secs(300));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await });
        storage.hung.notified().await;
        let row = registry.get(source).await.unwrap().unwrap();
        run.abort();
        let _ = run.await;
        let claimed = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        assert_eq!(
            row.lease_until,
            Some(claimed + chrono::Duration::seconds(300) + chrono::Duration::seconds(90))
        );
    }

    /// The proof that the job is over before its lease is, by running it: the
    /// longest path an owner can take, with every bounded step taking its whole
    /// bound on a virtual clock. The conversion has used the whole budget when the
    /// source turns out to be gone, the cleanup of it fails, and the failure is then
    /// recorded: claim, first progress, two ownership reads, two deletes and the
    /// failure write. A step added to this path changes the log below and the time.
    pub(crate) async fn the_longest_owner_path_ends_before_the_lease_does(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::{FaultyRegistry, Slow};
        let source = fresh_source();
        let source = source.as_str();
        let start = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let slow = Slow::new(start);
        let storage = PlacedStorage::with_source(source, late_word_file());
        *storage.slow.lock().unwrap() = Some(slow.clone());
        *storage.second_open_cost.lock().unwrap() = Some(PREP_TIMEOUT);
        *storage.remove_source_on_second_open.lock().unwrap() = true;
        *storage.fail_delete.lock().unwrap() = true;
        let faulty = FaultyRegistry::new(registry.clone());
        *faulty.faults.slow.lock().unwrap() = Some(slow.clone());
        let progress = Arc::new(Recording::default());
        *progress.slow.lock().unwrap() = Some(slow.clone());
        let clock = slow.clone();
        let env = env(faulty, storage.clone())
            .with_clock(Arc::new(move || clock.now()))
            .with_progress(progress)
            .with_writer(WriterConfig {
                max_rows: 2000,
                max_bytes: usize::MAX,
            });
        let out = prepare_csv(&env, &request(source, 60_000)).await.unwrap();
        assert!(matches!(out, PrepareOutcome::Failed(_)), "{out:?}");
        let log = slow.log.lock().unwrap().clone();
        let ops: Vec<&str> = log.iter().map(|(op, _)| *op).collect();
        assert_eq!(
            ops,
            [
                "claim",
                "progress",
                "still_owned",
                "delete",
                "still_owned",
                "delete",
                "fail_with_blobs",
                "progress"
            ],
            "the path changed: update the proof (JOB_GRACE, MAX_OWNER_STEPS)"
        );
        // Every action that needs the lease happened before it ran out.
        let lease_until = slow.lease_until.lock().unwrap().expect("the claimed lease");
        for (op, at) in &log {
            if matches!(*op, "delete" | "fail_with_blobs" | "release" | "complete") {
                assert!(
                    at < &lease_until,
                    "{op} at {at} after the lease {lease_until}"
                );
            }
        }
        // And the lease is the budget plus the grace.
        assert_eq!(lease_until, start + chrono::Duration::seconds(300 + 90));
    }

    pub(crate) async fn a_cleanup_of_a_deleted_source_that_does_not_answer_keeps_the_row_and_its_keys(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, late_word_file());
        *storage.remove_source_on_second_open.lock().unwrap() = true;
        *storage.hang_delete.lock().unwrap() = true;
        let (env, gate) = gated(
            env(registry.clone(), storage.clone()).with_writer(WriterConfig {
                max_rows: 2000,
                max_bytes: usize::MAX,
            }),
        );
        let req = request(source, 60_000);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await.unwrap() });
        let steps = async {
            // The delete of the deleted source's objects, then the one of the failure.
            for _ in 0..2 {
                storage.delete_reached.notified().await;
                gate.step.notify_one();
            }
        };
        tokio::time::timeout(Duration::from_secs(30), steps)
            .await
            .expect("the job did not go on after the delete timed out");
        let out = joined(run).await;
        // The objects could not be removed, so the row stays, failed, listing them.
        let (f, row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::STORAGE);
        for k in part_keys(source, 4) {
            assert!(row.blob_keys.contains(&k), "{k} not tracked");
        }
    }

    /// Collects what the tracing events of this thread write.
    #[derive(Clone, Default)]
    struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Captured;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// A log event can be lost when the first hit of its call site races another
    /// test's (tracing's call-site registration is global), never invented. So the
    /// scenarios run again until every event is seen, and what must not appear is
    /// checked on every run.
    pub(crate) async fn logs_carry_a_fixed_sentence_a_kind_and_an_opaque_id_never_a_key_or_adapter_text(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        for attempt in 0..10 {
            let (text, ids) = log_scenarios(&registry).await;
            for forbidden in ["secret", "chat-attachments", ".csv", "prepared/"] {
                assert!(
                    !text.contains(forbidden),
                    "{forbidden:?} in the log:\n{text}"
                );
            }
            let seen = ids.iter().all(|id| text.contains(id.as_str()))
                && text.contains("kind=\"registry\"")
                && text.contains("kind=\"upload_failed\"");
            for id in &ids {
                assert_eq!(id.len(), 12);
                // Hexadecimal digits of a digest, not a piece of the key.
                assert!(id.bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
                assert!(!"chat-attachments/u/s/".contains(id.as_str()));
            }
            if seen {
                return;
            }
            assert!(attempt < 9, "events missing after ten runs:\n{text}");
        }
    }

    async fn log_scenarios(registry: &Arc<dyn PreparationRegistry>) -> (String, Vec<String>) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        use std::sync::atomic::Ordering::SeqCst;
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(captured.clone())
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        // Callsites first hit by other tests with no subscriber cache "never": look again.
        tracing::callsite::rebuild_interest_cache();
        let config = PrepareConfig::from_switch(Some("on"));
        let mut ids = Vec::new();
        // 1. The registry cannot record the outcome (its text names a key).
        let source = fresh_source();
        let storage = PlacedStorage::with_source(&source, five_rows());
        let faulty = FaultyRegistry::new(registry.clone());
        faulty.faults.fail_complete.store(true, SeqCst);
        let runner = CsvPrepareRunner::new(Arc::new(env(faulty, storage)), &config);
        runner.run(request(&source, 40)).await;
        ids.push(opaque_id(&source));
        // 2. The failure is recorded and the cleanup that follows is refused with
        // text that names a key.
        let source = fresh_source();
        let storage = PlacedStorage::with_source(&source, five_rows());
        *storage.fail_stores_from.lock().unwrap() = Some(1);
        *storage.fail_delete.lock().unwrap() = true;
        let runner = CsvPrepareRunner::new(Arc::new(env(registry.clone(), storage)), &config);
        runner.run(request(&source, 40)).await;
        ids.push(opaque_id(&source));
        // 3. A request that is not a CSV.
        let source = fresh_source();
        let storage = PlacedStorage::with_source(&source, five_rows());
        let runner = CsvPrepareRunner::new(Arc::new(env(registry.clone(), storage)), &config);
        let mut req = request(&source, 40);
        req.mime_type = "application/pdf".into();
        runner.run(req).await;
        ids.push(opaque_id(&source));
        // 4. No derived root.
        let source = fresh_source();
        let storage = PlacedStorage::with_source(&source, five_rows());
        *storage.no_root.lock().unwrap() = true;
        let runner = CsvPrepareRunner::new(Arc::new(env(registry.clone(), storage)), &config);
        runner.run(request(&source, 40)).await;
        ids.push(opaque_id(&source));

        let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        (text, ids)
    }

    pub(crate) async fn a_source_key_that_cannot_be_a_key_is_refused_without_a_row(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let storage = Arc::new(PlacedStorage::default());
        let env = env(registry, storage.clone());
        let out = prepare_csv(&env, &request("chat-attachments/../x.csv", 40))
            .await
            .unwrap();
        assert!(matches!(
            out,
            PrepareOutcome::Refused(PrepareStartError::InvalidSourceKey)
        ));
        assert_eq!(*storage.opens.lock().unwrap(), 0);
    }

    pub(crate) async fn a_retry_that_claims_right_after_the_failure_write_keeps_its_part(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // The manifest cannot be stored: the job fails after three parts.
        *storage.fail_stores_from.lock().unwrap() = Some(3);
        let faulty = FaultyRegistry::new(registry.clone());
        // The moment the failure is recorded the row is claimable: a retry takes
        // it and writes the first part, under the same deterministic key.
        let (reg, st, src) = (registry.clone(), storage.clone(), source.to_string());
        let hook: crate::tabular_prepare::registry_faults::AfterFail = Arc::new(move || {
            let (reg, st, src) = (reg.clone(), st.clone(), src.clone());
            Box::pin(async move {
                let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 1).unwrap();
                let claim = ClaimRequest {
                    source_key: src.clone(),
                    source_bytes: 1,
                    format_version: FORMAT_VERSION,
                    owner: "retry".into(),
                    lease: lease_for(chrono::Duration::seconds(300)),
                    now,
                };
                assert!(reg.claim(claim).await.unwrap().is_some());
                let key = format!("{}/t0/part-00000.parquet", root_of(&src));
                st.objects
                    .lock()
                    .unwrap()
                    .insert(key, Bytes::from_static(b"retry-part"));
            })
        });
        *faulty.faults.after_fail.lock().unwrap() = Some(hook);
        let env = env(faulty, storage.clone());
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        assert!(matches!(out, PrepareOutcome::Failed(_)), "{out:?}");
        // What the retry wrote survives: the failed job deleted before it recorded
        // the failure, and not after.
        let key = format!("{}/t0/part-00000.parquet", root_of(source));
        assert_eq!(
            storage.objects.lock().unwrap().get(&key).cloned(),
            Some(Bytes::from_static(b"retry-part"))
        );
        assert_eq!(
            registry
                .get(source)
                .await
                .unwrap()
                .unwrap()
                .lease_owner
                .as_deref(),
            Some("retry")
        );
    }

    pub(crate) async fn a_failure_write_that_never_returns_is_given_up_on(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        use std::sync::atomic::Ordering::SeqCst;
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        *storage.fail_stores_from.lock().unwrap() = Some(3);
        let faulty = FaultyRegistry::new(registry.clone());
        faulty.faults.hang_fail.store(true, SeqCst);
        let (env, gate) = gated(env(faulty.clone(), storage.clone()));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await });
        faulty.faults.fail_reached.notified().await;
        gate.step.notify_one();
        let out = tokio::time::timeout(Duration::from_secs(30), run)
            .await
            .expect("the job outlived its bound")
            .unwrap();
        assert!(out.is_err(), "{out:?}");
        // The objects were deleted before the failure write, and the row lists them.
        assert_eq!(storage.keys(), vec![source.to_string()]);
        assert_eq!(
            registry.get(source).await.unwrap().unwrap().status,
            PrepareStatus::Running
        );
    }

    pub(crate) async fn a_release_that_never_returns_is_given_up_on(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        use std::sync::atomic::Ordering::SeqCst;
        let source = fresh_source();
        let source = source.as_str();
        // The source does not exist: the row is to be released.
        let storage = Arc::new(PlacedStorage::default());
        let faulty = FaultyRegistry::new(registry.clone());
        faulty.faults.hang_release.store(true, SeqCst);
        let (env, gate) = gated(env(faulty.clone(), storage.clone()));
        let req = request(source, 40);
        let run = tokio::spawn(async move { prepare_csv(&env, &req).await });
        faulty.faults.release_reached.notified().await;
        gate.step.notify_one();
        let out = tokio::time::timeout(Duration::from_secs(30), run)
            .await
            .expect("the job outlived its bound")
            .unwrap();
        assert!(out.is_err(), "{out:?}");
    }

    pub(crate) async fn tracking_is_kept_per_table_so_a_second_tables_first_part_is_listed_before_it_is_put(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let claim = ClaimRequest {
            source_key: source.to_string(),
            source_bytes: 1,
            format_version: FORMAT_VERSION,
            owner: "me".into(),
            lease: lease_for(chrono::Duration::seconds(300)),
            now,
        };
        assert!(registry.claim(claim).await.unwrap().is_some());
        let sink = OwnedSink {
            inner: Arc::new(StoragePartSink::new(storage.clone(), source).unwrap()),
            registry: registry.clone(),
            clock: Arc::new(move || now),
            source_key: source.to_string(),
            owner: "me".into(),
            lost: AtomicBool::new(false),
            registry_failed: AtomicBool::new(false),
            tracked_parts: std::sync::Mutex::new(std::collections::HashMap::new()),
            manifest_tracked: AtomicBool::new(false),
            stored: Default::default(),
            max_stored: u64::MAX,
            too_big: AtomicBool::new(false),
        };
        let data = Bytes::from_static(b"x");
        sink.put("t0/part-00000.parquet", data.clone())
            .await
            .unwrap();
        sink.put("t1/part-00000.parquet", data).await.unwrap();
        let row = registry.get(source).await.unwrap().unwrap();
        let root = root_of(source);
        for table in 0..2 {
            for part in 0..TRACK_AHEAD {
                let key = format!("{root}/t{table}/part-{part:05}.parquet");
                assert!(row.blob_keys.contains(&key), "{key} not tracked");
            }
        }
    }

    pub(crate) async fn a_registry_error_while_tracking_is_an_internal_failure_not_a_storage_one(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        use std::sync::atomic::Ordering::SeqCst;
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let faulty = FaultyRegistry::new(registry.clone());
        faulty.faults.fail_track.store(true, SeqCst);
        let env = env(faulty, storage.clone());
        let out = prepare_csv(&env, &request(source, 40)).await.unwrap();
        let (f, _row) = failed_row(&registry, source, out).await;
        assert_eq!(f.code, reason::INTERNAL);
        assert!(!f.detail.contains("secret"), "{}", f.detail);
        assert_eq!(*storage.stores.lock().unwrap(), 0);
    }

    pub(crate) async fn a_failed_delete_with_no_row_does_not_promise_a_cleanup(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        for attempt in 0..10 {
            let source = fresh_source();
            let storage = PlacedStorage::with_source(&source, five_rows());
            *storage.fail_delete.lock().unwrap() = true;
            let captured = Captured::default();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(captured.clone())
                .with_ansi(false)
                .finish();
            let _guard = tracing::subscriber::set_default(subscriber);
            tracing::callsite::rebuild_interest_cache();
            let env = env(registry.clone(), storage);
            // No row exists for this source: the job lost it and deletes what it wrote.
            let keys = vec![format!("{}/t0/part-00000.parquet", root_of(&source))];
            let req = request(&source, 40);
            let out = settle_lost(&env, &req, &keys).await.unwrap();
            assert!(matches!(out, PrepareOutcome::Cancelled));
            let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
            assert!(!text.contains("cleanup pass will"), "{text}");
            if text.contains("no row lists them") {
                return;
            }
            assert!(attempt < 9, "event missing after ten runs:\n{text}");
        }
    }

    pub(crate) async fn a_registry_error_at_completion_leaves_the_stored_objects_tracked_and_not_deleted(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        use std::sync::atomic::Ordering::SeqCst;
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        let faulty = FaultyRegistry::new(registry.clone());
        faulty.faults.fail_complete.store(true, SeqCst);
        let env = env(faulty, storage.clone());
        let err = prepare_csv(&env, &request(source, 40)).await.unwrap_err();
        let _ = err;
        // Whether `complete` applied is unknown, so nothing is deleted; every
        // object is listed in the row instead.
        let row = registry.get(source).await.unwrap().unwrap();
        assert!(storage.deleted.lock().unwrap().is_empty());
        let stored = objects_but_source(&storage, source);
        assert_eq!(stored.len(), 4);
        for k in stored {
            assert!(row.blob_keys.contains(&k), "{k} stored but not tracked");
        }
    }

    pub(crate) async fn a_registry_error_recording_a_failure_still_deletes_the_objects_that_are_tracked(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        use crate::tabular_prepare::registry_faults::FaultyRegistry;
        use std::sync::atomic::Ordering::SeqCst;
        let source = fresh_source();
        let source = source.as_str();
        let storage = PlacedStorage::with_source(source, five_rows());
        // The manifest cannot be stored, and the failure cannot be recorded.
        *storage.fail_stores_from.lock().unwrap() = Some(3);
        let faulty = FaultyRegistry::new(registry.clone());
        faulty.faults.fail_fail.store(true, SeqCst);
        let env = env(faulty, storage.clone());
        assert!(prepare_csv(&env, &request(source, 40)).await.is_err());
        // The failure is safe to act on whether or not it was recorded.
        assert_eq!(storage.keys(), vec![source.to_string()]);
        let row = registry.get(source).await.unwrap().unwrap();
        assert_eq!(row.status, PrepareStatus::Running);
        assert_eq!(row.blob_keys.len(), TRACK_AHEAD + 1);
    }

    pub(crate) async fn a_restart_with_fewer_parts_leaves_stale_keys_the_manifest_never_names(
        registry: Arc<dyn PreparationRegistry>,
    ) {
        let source = fresh_source();
        let source = source.as_str();
        // Ten thousand integers and a late word: the first run writes four
        // parts (8,000 rows; the rest of its batch was never flushed), then the column becomes text and the file is read again; it
        // changed in between, and the second run writes one part.
        let mut first = b"id\n".to_vec();
        for i in 0..10_000 {
            first.extend_from_slice(format!("{i}\n").as_bytes());
        }
        first.extend_from_slice(b"late\n");
        let storage = PlacedStorage::with_source(source, first);
        *storage.swap_on_second_open.lock().unwrap() = Some(Bytes::from_static(b"id\nx\ny\n"));
        let env = env(registry.clone(), storage.clone()).with_writer(WriterConfig {
            max_rows: 2000,
            max_bytes: usize::MAX,
        });
        let PrepareOutcome::Ready(table) =
            prepare_csv(&env, &request(source, 60_000)).await.unwrap()
        else {
            panic!("expected a ready table");
        };
        assert_eq!(table.manifest.tables[0].parts, 1);
        assert_eq!(table.stale_keys, part_keys(source, 4)[1..].to_vec());
        let row = registry.get(source).await.unwrap().unwrap();
        // Tracked for cleanup, still stored, and not referenced.
        for k in &table.stale_keys {
            assert!(row.blob_keys.contains(k));
            assert!(storage.objects.lock().unwrap().contains_key(k));
        }
        for k in part_keys(source, 4) {
            assert!(row.blob_keys.contains(&k));
        }
        assert!(row
            .blob_keys
            .contains(&format!("{}/manifest.json", root_of(source))));
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
    sqlite_case!(
        tabular_prepare_a_file_that_is_not_a_csv_fails_with_a_fixed_text_and_no_cell,
        a_file_that_is_not_a_csv_fails_with_a_fixed_text_and_no_cell
    );
    sqlite_case!(
        tabular_prepare_a_storage_failure_midway_tracks_every_key_tried_and_deletes_them,
        a_storage_failure_midway_tracks_every_key_tried_and_deletes_them
    );
    sqlite_case!(
        tabular_prepare_a_manifest_that_could_not_be_stored_never_makes_the_row_ready,
        a_manifest_that_could_not_be_stored_never_makes_the_row_ready
    );
    sqlite_case!(
        tabular_prepare_a_restart_with_fewer_parts_leaves_stale_keys_the_manifest_never_names,
        a_restart_with_fewer_parts_leaves_stale_keys_the_manifest_never_names
    );
    sqlite_case!(
        tabular_prepare_the_budget_ends_a_stuck_run_with_the_time_reason_and_removes_its_output,
        the_budget_ends_a_stuck_run_with_the_time_reason_and_removes_its_output
    );
    sqlite_case!(
        tabular_prepare_a_source_that_never_yields_ends_with_the_time_reason,
        a_source_that_never_yields_ends_with_the_time_reason
    );
    sqlite_case!(
        tabular_prepare_a_job_whose_row_was_deleted_stops_before_its_next_part_and_leaves_no_row,
        a_job_whose_row_was_deleted_stops_before_its_next_part_and_leaves_no_row
    );
    sqlite_case!(
        tabular_prepare_a_job_whose_lease_was_taken_stops_and_leaves_the_new_owners_row_and_objects,
        a_job_whose_lease_was_taken_stops_and_leaves_the_new_owners_row_and_objects
    );
    sqlite_case!(
        tabular_prepare_a_completion_that_finds_the_row_gone_is_cancelled_and_never_ready,
        a_completion_that_finds_the_row_gone_is_cancelled_and_never_ready
    );
    sqlite_case!(
        tabular_prepare_an_unopenable_source_releases_the_row_without_a_failure,
        an_unopenable_source_releases_the_row_without_a_failure
    );
    sqlite_case!(
        tabular_prepare_a_source_deleted_during_a_restart_removes_what_was_written_and_releases_the_row,
        a_source_deleted_during_a_restart_removes_what_was_written_and_releases_the_row
    );
    sqlite_case!(
        tabular_prepare_progress_is_reported_every_interval_to_the_port_and_never_written_to_the_registry,
        progress_is_reported_every_interval_to_the_port_and_never_written_to_the_registry
    );
    sqlite_case!(
        tabular_prepare_a_finished_preparation_reports_its_final_state_to_the_port,
        a_finished_preparation_reports_its_final_state_to_the_port
    );
    sqlite_case!(
        tabular_prepare_the_inline_trigger_runs_a_csv_request_through_the_driver,
        the_inline_trigger_runs_a_csv_request_through_the_driver
    );
    sqlite_case!(
        tabular_prepare_with_the_switch_off_or_another_mime_the_runner_touches_nothing,
        with_the_switch_off_or_another_mime_the_runner_touches_nothing
    );
    sqlite_case!(
        tabular_prepare_dropping_the_prepare_future_mid_run_leaves_every_stored_object_tracked_in_the_row,
        dropping_the_prepare_future_mid_run_leaves_every_stored_object_tracked_in_the_row
    );
    sqlite_case!(
        tabular_prepare_a_registry_error_at_completion_leaves_the_stored_objects_tracked_and_not_deleted,
        a_registry_error_at_completion_leaves_the_stored_objects_tracked_and_not_deleted
    );
    sqlite_case!(
        tabular_prepare_a_registry_error_recording_a_failure_still_deletes_the_objects_that_are_tracked,
        a_registry_error_recording_a_failure_still_deletes_the_objects_that_are_tracked
    );
    sqlite_case!(
        tabular_prepare_a_part_beyond_the_first_batch_is_tracked_before_its_put_a_batch_ahead,
        a_part_beyond_the_first_batch_is_tracked_before_its_put_a_batch_ahead
    );
    sqlite_case!(
        tabular_prepare_a_row_that_vanishes_before_the_first_tracking_write_stops_the_job_before_any_object,
        a_row_that_vanishes_before_the_first_tracking_write_stops_the_job_before_any_object
    );
    sqlite_case!(
        tabular_prepare_a_row_deleted_before_the_budget_ends_the_run_still_gets_its_objects_deleted,
        a_row_deleted_before_the_budget_ends_the_run_still_gets_its_objects_deleted
    );
    sqlite_case!(
        tabular_prepare_a_row_deleted_before_a_storage_failure_still_gets_its_objects_deleted,
        a_row_deleted_before_a_storage_failure_still_gets_its_objects_deleted
    );
    sqlite_case!(
        tabular_prepare_a_row_deleted_before_a_source_read_failure_still_gets_its_objects_deleted,
        a_row_deleted_before_a_source_read_failure_still_gets_its_objects_deleted
    );
    sqlite_case!(
        tabular_prepare_a_lease_taken_before_the_source_turns_out_missing_deletes_nothing_and_releases_nothing,
        a_lease_taken_before_the_source_turns_out_missing_deletes_nothing_and_releases_nothing
    );
    sqlite_case!(
        tabular_prepare_a_row_taken_before_the_budget_ends_the_run_keeps_the_new_owners_objects,
        a_row_taken_before_the_budget_ends_the_run_keeps_the_new_owners_objects
    );
    sqlite_case!(
        tabular_prepare_a_manifest_put_that_never_completes_is_ended_by_the_same_budget,
        a_manifest_put_that_never_completes_is_ended_by_the_same_budget
    );
    sqlite_case!(
        tabular_prepare_a_completion_that_never_returns_is_ended_by_its_own_bound_and_deletes_nothing,
        a_completion_that_never_returns_is_ended_by_its_own_bound_and_deletes_nothing
    );
    sqlite_case!(
        tabular_prepare_a_delete_that_never_returns_does_not_hold_the_failure_back,
        a_delete_that_never_returns_does_not_hold_the_failure_back
    );
    sqlite_case!(
        tabular_prepare_the_claimed_lease_is_the_budget_plus_the_job_grace,
        the_claimed_lease_is_the_budget_plus_the_job_grace
    );
    sqlite_case!(
        tabular_prepare_a_cleanup_of_a_deleted_source_that_does_not_answer_keeps_the_row_and_its_keys,
        a_cleanup_of_a_deleted_source_that_does_not_answer_keeps_the_row_and_its_keys
    );
    sqlite_case!(
        tabular_prepare_logs_carry_a_fixed_sentence_a_kind_and_an_opaque_id_never_a_key_or_adapter_text,
        logs_carry_a_fixed_sentence_a_kind_and_an_opaque_id_never_a_key_or_adapter_text
    );
    sqlite_case!(
        tabular_prepare_a_source_key_that_cannot_be_a_key_is_refused_without_a_row,
        a_source_key_that_cannot_be_a_key_is_refused_without_a_row
    );
    sqlite_case!(
        tabular_prepare_a_retry_that_claims_right_after_the_failure_write_keeps_its_part,
        a_retry_that_claims_right_after_the_failure_write_keeps_its_part
    );
    sqlite_case!(
        tabular_prepare_a_failure_write_that_never_returns_is_given_up_on,
        a_failure_write_that_never_returns_is_given_up_on
    );
    sqlite_case!(
        tabular_prepare_a_release_that_never_returns_is_given_up_on,
        a_release_that_never_returns_is_given_up_on
    );
    sqlite_case!(
        tabular_prepare_the_longest_owner_path_ends_before_the_lease_does,
        the_longest_owner_path_ends_before_the_lease_does
    );
    sqlite_case!(
        tabular_prepare_tracking_is_kept_per_table_so_a_second_tables_first_part_is_listed_before_it_is_put,
        tracking_is_kept_per_table_so_a_second_tables_first_part_is_listed_before_it_is_put
    );
    sqlite_case!(
        tabular_prepare_a_registry_error_while_tracking_is_an_internal_failure_not_a_storage_one,
        a_registry_error_while_tracking_is_an_internal_failure_not_a_storage_one
    );
    sqlite_case!(
        tabular_prepare_a_failed_delete_with_no_row_does_not_promise_a_cleanup,
        a_failed_delete_with_no_row_does_not_promise_a_cleanup
    );
}
