//! The driver on workbooks: the same claim, tracking, ownership, budget and
//! failure paths as a CSV, with the failures a workbook adds. SQLite registry, a
//! fake storage that honours the placement.

use crate::storage::domain::{
    OutputStorageRepository, StorageError, StorePlacement, StoreRequest, StoreStreamRequest,
    StoredBytes, StoredOutput, StoredStream,
};
use crate::tabular_prepare::driver::{
    prepare_xlsx, reason, CsvPrepareRunner, PrepareEnv, PrepareOutcome, TRACK_AHEAD_FOR_TESTS,
    XLSX_MIME,
};
use crate::tabular_prepare::manifest::Manifest;
use crate::tabular_prepare::ports::{PrepareConfig, PrepareRequest, PrepareRunner};
use crate::tabular_prepare::prepare::fake::{root_of, PlacedStorage};
use crate::tabular_prepare::registry::{PreparationRegistry, PrepareStatus};
use crate::tabular_prepare::sqlite_registry::SqlitePreparationRegistry;
use crate::tabular_prepare::writer::WriterConfig;
use crate::tabular_prepare::xlsxfix::Wb;
use crate::tabular_prepare::zipfix::{build, Entry};
use chrono::{TimeZone, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::sync::{Arc, Mutex};
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

fn source() -> String {
    format!("chat-attachments/u/s/{}.xlsx", uuid::Uuid::new_v4())
}

fn request(source: &str, size: u64) -> PrepareRequest {
    PrepareRequest {
        source_key: source.to_string(),
        mime_type: XLSX_MIME.into(),
        filename: "book.xlsx".into(),
        size_bytes: size,
    }
}

fn env(registry: Arc<dyn PreparationRegistry>, storage: Arc<PlacedStorage>) -> PrepareEnv {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    PrepareEnv::new(registry, storage)
        .with_clock(Arc::new(move || now))
        .with_writer(WriterConfig {
            max_rows: 2,
            max_bytes: usize::MAX,
        })
}

fn cell(col: &str, row: usize, text: &str) -> String {
    format!("<c r=\"{col}{row}\" t=\"inlineStr\"><is><t>{text}</t></is></c>")
}

/// Two sheets with names that differ only by case, five rows in the first.
fn workbook() -> Vec<u8> {
    let first: String =
        std::iter::once("<row r=\"1\">".to_string() + &cell("A", 1, "id") + "</row>")
            .chain((2..=6).map(|r| format!("<row r=\"{r}\"><c r=\"A{r}\"><v>{r}</v></c></row>")))
            .collect();
    let second = format!(
        "<row r=\"1\">{}</row><row r=\"2\">{}</row>",
        cell("A", 1, "note"),
        cell("A", 2, "hi")
    );
    Wb::new()
        .sheet("Q3 Sales", &first)
        .sheet("q3 sales", &second)
        .build()
}

fn objects_but_source(storage: &PlacedStorage, source: &str) -> Vec<String> {
    storage.keys().into_iter().filter(|k| k != source).collect()
}

#[tokio::test]
async fn a_workbook_is_ready_with_a_table_per_sheet_and_the_manifest_stored_last() {
    let (registry, _dir) = sqlite().await;
    let source = source();
    let bytes = workbook();
    let size = bytes.len() as u64;
    let storage = PlacedStorage::with_source(&source, bytes);
    let env = env(registry.clone(), storage.clone());
    let PrepareOutcome::Ready(table) = prepare_xlsx(&env, &request(&source, size)).await.unwrap()
    else {
        panic!("expected a ready workbook");
    };
    let names: Vec<_> = table
        .manifest
        .tables
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["Q3 Sales", "q3 sales_2"],
        "names are unique ignoring case"
    );
    let rows: Vec<_> = table
        .manifest
        .tables
        .iter()
        .map(|t| (t.rows, t.parts))
        .collect();
    assert_eq!(rows, [(5, 3), (1, 1)]);
    assert_eq!(table.tables.len(), 2);
    // Exactly the manifest, three parts of the first table and one of the second.
    let root = root_of(&source);
    let manifest_key = format!("{root}/manifest.json");
    let mut want: Vec<String> = [0, 1, 2]
        .iter()
        .map(|i| format!("{root}/t0/part-{i:05}.parquet"))
        .collect();
    want.push(format!("{root}/t1/part-00000.parquet"));
    want.push(manifest_key.clone());
    let mut got = objects_but_source(&storage, &source);
    got.sort();
    want.sort();
    assert_eq!(got, want);
    assert_eq!(storage.order.lock().unwrap().last(), Some(&manifest_key));
    // The row lists every object, and each table's next parts ahead of its first put.
    let row = registry.get(&source).await.unwrap().unwrap();
    assert_eq!(row.status, PrepareStatus::Ready);
    assert_eq!(row.manifest_key.as_deref(), Some(manifest_key.as_str()));
    for k in &want {
        assert!(row.blob_keys.contains(k), "{k} not tracked");
    }
    assert_eq!(row.blob_keys.len(), 2 * TRACK_AHEAD_FOR_TESTS + 1);
    let stored = storage.objects.lock().unwrap()[&manifest_key].clone();
    assert_eq!(Manifest::from_json(&stored).unwrap(), table.manifest);
    assert_eq!(table.manifest.conversion.len(), 2);
    let held: usize = got
        .iter()
        .map(|k| storage.objects.lock().unwrap()[k].len())
        .sum();
    assert_eq!(row.prepared_bytes, Some(held as i64));
    assert!(table.stale_keys.is_empty());
}

#[tokio::test]
async fn a_workbook_over_the_byte_cap_fails_before_a_byte_is_read() {
    let (registry, _dir) = sqlite().await;
    let source = source();
    let bytes = workbook();
    let storage = PlacedStorage::with_source(&source, bytes.clone());
    // One byte over what the host declared.
    let env = env(registry.clone(), storage.clone()).with_xlsx_max_bytes(1000);
    let PrepareOutcome::Failed(f) = prepare_xlsx(&env, &request(&source, 1001)).await.unwrap()
    else {
        panic!("expected a recorded failure");
    };
    assert_eq!(f.code, reason::XLSX_TOO_LARGE);
    assert_eq!(
        f.detail,
        "the workbook is over the size limit; export it as CSV"
    );
    let row = registry.get(&source).await.unwrap().unwrap();
    assert_eq!(row.error_code.as_deref(), Some("xlsx_too_large"));
    assert_eq!(
        (
            *storage.opens.lock().unwrap(),
            *storage.stores.lock().unwrap()
        ),
        (0, 0)
    );
}

#[tokio::test]
async fn the_byte_cap_is_inclusive_and_zero_refuses_every_workbook() {
    let (registry, _dir) = sqlite().await;
    let bytes = workbook();
    let len = bytes.len() as u64;
    let run = |cap: u64, declared: u64, actual: Vec<u8>| {
        let registry = registry.clone();
        async move {
            let source = source();
            let storage = PlacedStorage::with_source(&source, actual);
            let env = env(registry, storage.clone()).with_xlsx_max_bytes(cap);
            (
                prepare_xlsx(&env, &request(&source, declared))
                    .await
                    .unwrap(),
                storage,
            )
        }
    };
    let (out, _) = run(0, 1, bytes.clone()).await;
    assert!(matches!(out, PrepareOutcome::Failed(f) if f.code == reason::XLSX_TOO_LARGE));
    // At the cap it is prepared.
    let (out, _) = run(len, len, bytes.clone()).await;
    assert!(matches!(out, PrepareOutcome::Ready(_)));
    // A storage whose size is larger than the host said is caught while it is read,
    // before anything is written.
    let (out, storage) = run(len - 1, 10, bytes).await;
    assert!(matches!(out, PrepareOutcome::Failed(f) if f.code == reason::XLSX_TOO_LARGE));
    assert_eq!(*storage.stores.lock().unwrap(), 0);
}

/// Prepares `bytes` as a workbook and returns the recorded failure, checking that
/// the row says the same and keeps no object.
async fn failure_of(bytes: Vec<u8>) -> (&'static str, String) {
    let (registry, _dir) = sqlite().await;
    let source = source();
    let len = bytes.len() as u64;
    let storage = PlacedStorage::with_source(&source, bytes);
    let env = env(registry.clone(), storage.clone());
    let PrepareOutcome::Failed(f) = prepare_xlsx(&env, &request(&source, len)).await.unwrap()
    else {
        panic!("expected a recorded failure");
    };
    let row = registry.get(&source).await.unwrap().unwrap();
    assert_eq!(row.status, PrepareStatus::Failed);
    assert_eq!(row.error_code.as_deref(), Some(f.code));
    assert_eq!(row.error_detail.as_deref(), Some(f.detail.as_str()));
    assert!(
        objects_but_source(&storage, &source).is_empty(),
        "nothing is left behind"
    );
    (f.code, f.detail)
}

#[tokio::test]
async fn a_zip_bomb_fails_with_archive_limit_before_anything_is_inflated() {
    // An entry whose headers say 200 MiB from 1 MiB: a ratio under 1 %.
    let bomb =
        build(&[Entry::stored("xl/worksheets/sheet1.xml", b"x").claim(8, 1 << 20, 200 << 20)]);
    let (code, detail) = failure_of(bomb).await;
    assert_eq!(code, reason::ARCHIVE_LIMIT);
    assert_eq!(
        detail,
        "a part of the workbook is compressed beyond the ratio limit"
    );
    // A traversal name.
    let (code, _) = failure_of(build(&[Entry::stored("../evil.xml", b"x")])).await;
    assert_eq!(code, reason::ARCHIVE_LIMIT);
}

#[tokio::test]
async fn a_file_that_is_not_a_workbook_fails_with_a_fixed_text_and_no_cell() {
    let (code, detail) = failure_of(b"hello, this is not a workbook".to_vec()).await;
    assert_eq!(
        (code, detail.as_str()),
        (reason::UNREADABLE_FILE, "the file is not an xlsx workbook")
    );
    let (code, detail) = failure_of(build(&[Entry::stored("hello.txt", b"hi")])).await;
    assert_eq!(
        (code, detail.as_str()),
        (
            reason::UNREADABLE_FILE,
            "the file is not a valid xlsx workbook"
        )
    );
    let (code, detail) = failure_of(Wb::new().sheet("A", "").build()).await;
    assert_eq!(
        (code, detail.as_str()),
        (
            reason::UNREADABLE_FILE,
            "the workbook has no sheet with data"
        )
    );
    let rows = format!(
        "<row r=\"1\">{}</row><row r=\"2\">{}{}</row>",
        cell("A", 1, "a"),
        "<c r=\"A2\"><v>1</v></c>",
        cell("B", 2, "secret cell text")
    );
    let (code, detail) = failure_of(Wb::new().sheet("Secret sheet", &rows).build()).await;
    assert_eq!(code, reason::UNREADABLE_FILE);
    assert!(!detail.to_lowercase().contains("secret"), "{detail}");
}

#[tokio::test]
async fn a_source_that_never_answers_ends_with_the_time_reason_and_one_that_is_gone_is_released() {
    let (registry, _dir) = sqlite().await;
    let source = source();
    let storage = PlacedStorage::with_source(&source, workbook());
    *storage.stall_source.lock().unwrap() = true;
    let env = env(registry.clone(), storage.clone()).with_budget(Duration::from_millis(300));
    let out = prepare_xlsx(&env, &request(&source, 100)).await.unwrap();
    assert!(
        matches!(&out, PrepareOutcome::Failed(f) if f.code == reason::TIME),
        "{out:?}"
    );
    let row = registry.get(&source).await.unwrap().unwrap();
    assert_eq!(row.error_code.as_deref(), Some("time"));
    // A source that does not exist releases the row and keeps no failure.
    let gone = self::source();
    let storage = Arc::new(PlacedStorage::default());
    let env = self::env(registry.clone(), storage);
    let out = prepare_xlsx(&env, &request(&gone, 100)).await.unwrap();
    assert!(matches!(out, PrepareOutcome::SourceGone));
    assert!(registry.get(&gone).await.unwrap().is_none());
}

/// A storage that records what the registry says at each put and at each cleanup of
/// the derived objects, and can take the row away at a chosen put.
struct Hook {
    inner: Arc<PlacedStorage>,
    registry: Arc<dyn PreparationRegistry>,
    source: String,
    /// Each put: its key, and whether the row listed it before the put.
    puts: Mutex<Vec<(String, bool)>>,
    /// Each `delete_derived`: its keys, and the row's status when it was called.
    cleanups: Mutex<Vec<(Vec<String>, Option<PrepareStatus>)>>,
    /// Delete the registry row when a key containing this is about to be put.
    take_row_at: Mutex<Option<String>>,
}

impl Hook {
    fn new(
        inner: Arc<PlacedStorage>,
        registry: Arc<dyn PreparationRegistry>,
        source: &str,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            registry,
            source: source.to_string(),
            puts: Mutex::new(Vec::new()),
            cleanups: Mutex::new(Vec::new()),
            take_row_at: Mutex::new(None),
        })
    }
}

#[async_trait::async_trait]
impl OutputStorageRepository for Hook {
    async fn store(&self, r: StoreRequest) -> Result<StoredOutput, StorageError> {
        self.inner.store(r).await
    }
    async fn read(&self, k: &str) -> Result<StoredBytes, StorageError> {
        self.inner.read(k).await
    }
    async fn read_stream(&self, k: &str) -> Result<StoredStream, StorageError> {
        self.inner.read_stream(k).await
    }
    async fn store_stream(&self, req: StoreStreamRequest) -> Result<StoredOutput, StorageError> {
        if let StorePlacement::DerivedFrom { relative_path, .. } = &req.placement {
            let key = format!("{}/{relative_path}", root_of(&self.source));
            let row = self.registry.get(&self.source).await.unwrap();
            let tracked = row.is_some_and(|r| r.blob_keys.contains(&key));
            self.puts.lock().unwrap().push((key.clone(), tracked));
            let take = self.take_row_at.lock().unwrap().clone();
            if take.is_some_and(|t| key.contains(&t)) {
                self.registry.delete(&self.source).await.unwrap();
            }
        }
        self.inner.store_stream(req).await
    }
    fn derived_root(&self, source: &str) -> Option<String> {
        self.inner.derived_root(source)
    }
    async fn delete_derived(&self, source: &str, keys: &[String]) -> Result<(), StorageError> {
        let status = self
            .registry
            .get(&self.source)
            .await
            .unwrap()
            .map(|r| r.status);
        self.cleanups.lock().unwrap().push((keys.to_vec(), status));
        let _ = source;
        for key in keys {
            self.inner.delete(key).await?;
        }
        Ok(())
    }
    async fn delete(&self, k: &str) -> Result<(), StorageError> {
        self.inner.delete(k).await
    }
}

#[tokio::test]
async fn nothing_written_means_no_cleanup_is_asked_of_the_adapter() {
    // An adapter may read an empty key list as "delete everything under the
    // prefix": a refusal before any write must not send one.
    let (registry, _dir) = sqlite().await;
    let source = source();
    let inner = PlacedStorage::with_source(&source, workbook());
    let hook = Hook::new(inner, registry.clone(), &source);
    let env = PrepareEnv::new(registry.clone(), hook.clone()).with_xlsx_max_bytes(100);
    let out = prepare_xlsx(&env, &request(&source, 101)).await.unwrap();
    assert!(matches!(out, PrepareOutcome::Failed(f) if f.code == reason::XLSX_TOO_LARGE));
    assert!(hook.cleanups.lock().unwrap().is_empty());
    let row = registry.get(&source).await.unwrap().unwrap();
    assert_eq!(row.status, PrepareStatus::Failed);
}

fn hooked(
    registry: &Arc<dyn PreparationRegistry>,
    source: &str,
) -> (Arc<PlacedStorage>, Arc<Hook>, PrepareEnv) {
    let inner = PlacedStorage::with_source(source, workbook());
    let hook = Hook::new(inner.clone(), registry.clone(), source);
    let env = env(registry.clone(), inner.clone());
    let env = PrepareEnv {
        storage: hook.clone(),
        ..env
    };
    (inner, hook, env)
}

#[tokio::test]
async fn every_put_of_every_table_is_listed_in_the_row_before_it_happens() {
    let (registry, _dir) = sqlite().await;
    let source = source();
    let (_, hook, env) = hooked(&registry, &source);
    let size = workbook().len() as u64;
    let out = prepare_xlsx(&env, &request(&source, size)).await.unwrap();
    assert!(matches!(out, PrepareOutcome::Ready(_)));
    let puts = hook.puts.lock().unwrap().clone();
    // Three parts of table 0, one of table 1, and the manifest.
    assert_eq!(puts.len(), 5);
    assert!(puts
        .iter()
        .any(|(k, _)| k.ends_with("/t1/part-00000.parquet")));
    for (key, tracked) in puts {
        assert!(tracked, "{key} was put before the row listed it");
    }
}

#[tokio::test]
async fn a_failure_after_the_first_sheet_wrote_parts_deletes_them_before_it_is_recorded() {
    let (registry, _dir) = sqlite().await;
    let source = source();
    let (inner, hook, env) = hooked(&registry, &source);
    // The fourth put (the first part of table 1) fails.
    *inner.fail_stores_from.lock().unwrap() = Some(3);
    let size = workbook().len() as u64;
    let PrepareOutcome::Failed(f) = prepare_xlsx(&env, &request(&source, size)).await.unwrap()
    else {
        panic!("expected a recorded failure");
    };
    assert_eq!(f.code, reason::STORAGE);
    // Table 0's three parts are gone with the rest; only the source remains.
    assert!(objects_but_source(&inner, &source).is_empty());
    // One cleanup, asked while the job still held the row (running), with the keys of
    // both tables; the failure was written after it.
    let cleanups = hook.cleanups.lock().unwrap().clone();
    assert_eq!(cleanups.len(), 1);
    assert_eq!(cleanups[0].1, Some(PrepareStatus::Running));
    assert!(cleanups[0]
        .0
        .iter()
        .any(|k| k.ends_with("/t0/part-00002.parquet")));
    assert!(cleanups[0]
        .0
        .iter()
        .any(|k| k.ends_with("/t1/part-00000.parquet")));
    let row = registry.get(&source).await.unwrap().unwrap();
    assert_eq!(row.status, PrepareStatus::Failed);
    assert_eq!(row.error_code.as_deref(), Some("storage"));
}

#[tokio::test]
async fn losing_the_row_in_the_middle_of_a_workbook_stops_the_job_and_removes_what_it_wrote() {
    let (registry, _dir) = sqlite().await;
    let source = source();
    let (inner, hook, env) = hooked(&registry, &source);
    // The source is deleted (the row with it) as table 1's first part is put.
    *hook.take_row_at.lock().unwrap() = Some("/t1/part-00000".into());
    let size = workbook().len() as u64;
    let out = prepare_xlsx(&env, &request(&source, size)).await.unwrap();
    assert!(matches!(out, PrepareOutcome::Cancelled), "{out:?}");
    assert!(
        registry.get(&source).await.unwrap().is_none(),
        "no row comes back"
    );
    assert!(
        objects_but_source(&inner, &source).is_empty(),
        "what it wrote is removed"
    );
    // Nothing was put after the row went: not the manifest.
    let puts = hook.puts.lock().unwrap().clone();
    assert!(!puts.iter().any(|(k, _)| k.ends_with("manifest.json")));
}

#[tokio::test]
async fn a_restart_overwrites_the_same_keys_and_leaves_nothing_stale() {
    // The restart reads the local copy again and writes the same deterministic keys: the
    // text run cannot have fewer parts than the typed run, so no stale part is left.
    let (registry, _dir) = sqlite().await;
    let source = source();
    let mut rows = vec![format!("<row r=\"1\">{}</row>", cell("A", 1, "n"))];
    for r in 2..=10_011usize {
        let c = if r == 10_006 {
            cell("A", r, "N/A")
        } else {
            format!("<c r=\"A{r}\"><v>{r}</v></c>")
        };
        rows.push(format!("<row r=\"{r}\">{c}</row>"));
    }
    let bytes = Wb::new().sheet("Data", &rows.concat()).build();
    let size = bytes.len() as u64;
    let storage = PlacedStorage::with_source(&source, bytes);
    let env = env(registry.clone(), storage.clone());
    let PrepareOutcome::Ready(table) = prepare_xlsx(&env, &request(&source, size)).await.unwrap()
    else {
        panic!("expected a ready workbook");
    };
    assert_eq!(table.tables[0].restarts, 1);
    assert!(table.stale_keys.is_empty());
    let row = registry.get(&source).await.unwrap().unwrap();
    let parts = table.manifest.tables[0].parts as usize;
    assert_eq!(objects_but_source(&storage, &source).len(), parts + 1);
    for k in objects_but_source(&storage, &source) {
        assert!(row.blob_keys.contains(&k), "{k} not tracked");
    }
}

/// A sheet with a title above its table, and a plain one.
fn titled_workbook() -> Vec<u8> {
    let title = format!(
        "<row r=\"1\">{}</row><row r=\"2\">{}{}</row>",
        cell("A", 1, "Quarterly report"),
        cell("A", 2, "id"),
        cell("B", 2, "name")
    );
    let data = format!(
        "<row r=\"1\">{}</row><row r=\"2\"><c r=\"A2\"><v>1</v></c></row>",
        cell("A", 1, "id")
    );
    Wb::new()
        .sheet("Title sheet", &title)
        .sheet("Data", &data)
        .build()
}

#[tokio::test]
async fn a_sheet_without_a_header_is_recorded_in_the_manifest_and_the_workbook_is_ready() {
    let (registry, _dir) = sqlite().await;
    let source = source();
    let bytes = titled_workbook();
    let size = bytes.len() as u64;
    let storage = PlacedStorage::with_source(&source, bytes);
    let env = env(registry.clone(), storage.clone());
    let PrepareOutcome::Ready(table) = prepare_xlsx(&env, &request(&source, size)).await.unwrap()
    else {
        panic!("expected a ready workbook");
    };
    let names: Vec<_> = table
        .manifest
        .tables
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(names, ["Data"]);
    assert_eq!(table.manifest.skipped.len(), 1);
    assert_eq!(table.manifest.skipped[0].sheet, "Title sheet");
    assert_eq!(table.manifest.skipped[0].reason, "header_row");
    let stored =
        storage.objects.lock().unwrap()[&format!("{}/manifest.json", root_of(&source))].clone();
    assert_eq!(Manifest::from_json(&stored).unwrap(), table.manifest);
}

#[tokio::test]
async fn the_inline_runner_routes_the_xlsx_mime_and_nothing_else() {
    let (registry, _dir) = sqlite().await;
    let source = source();
    let bytes = workbook();
    let size = bytes.len() as u64;
    let storage = PlacedStorage::with_source(&source, bytes);
    let env = Arc::new(env(registry.clone(), storage.clone()));
    let on = PrepareConfig::from_switch(Some("on"));
    // Another mime is dropped: no row.
    let mut pdf = request(&source, size);
    pdf.mime_type = "application/pdf".into();
    CsvPrepareRunner::new(env.clone(), &on).run(pdf).await;
    assert!(registry.get(&source).await.unwrap().is_none());
    // The switch off runs nothing, whatever the mime.
    CsvPrepareRunner::new(env.clone(), &PrepareConfig::from_switch(None))
        .run(request(&source, size))
        .await;
    assert!(registry.get(&source).await.unwrap().is_none());
    // The xlsx mime, with a parameter and in another case, is prepared.
    let mut xlsx = request(&source, size);
    xlsx.mime_type = format!("{}; charset=binary", XLSX_MIME.to_uppercase());
    CsvPrepareRunner::new(env, &on).run(xlsx).await;
    let row = registry.get(&source).await.unwrap().unwrap();
    assert_eq!(row.status, PrepareStatus::Ready);
}
