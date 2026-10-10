#![cfg(target_os = "linux")]
//! ONE run of the whole large-file path with nothing canned: the real tool
//! (`attachment_run_python`), routed by the executor to the runtime, which verifies a
//! prepared copy through a real registry, stages real Parquet parts, runs the real
//! prelude and wrapper in the real jail on the real subprocess executor, reads `/out`
//! back through the real collector, streams the file through the real sink, and
//! registers it. Needs root and CAP_SYS_ADMIN (`COLMENA_PYEXEC_JAIL_TESTS=1`) and
//! python3 with pyarrow, pandas and scipy; with `COLMENA_TABULAR_EXPECT_PANDAS=1` a
//! missing python3 library fails the test instead of skipping it.

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{Duration as ChronoDuration, Utc};
use colmena::dag_engine::application::ports::NodeRegistryPort;
use colmena::dag_engine::domain::node::ExecutableNode;
use colmena::dag_engine::domain::state::DagTaskMemoryRepository;
use colmena::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_run_python::dispatch_attachment_run_python_via_executor;
use colmena::dag_engine::infrastructure::persistence::PostgresDagStateRepository;
use colmena::dag_engine::infrastructure::pool_registry::{PgPoolRegistry, PoolConfig};
use colmena::dag_engine::infrastructure::python_exec::config::SubprocessConfig;
use colmena::dag_engine::infrastructure::python_exec::subprocess::SubprocessExecutor;
use colmena::dag_engine::infrastructure::registry::HashMapNodeRegistry;
use colmena::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;
use colmena::llm::domain::attachments::{
    origin, AttachmentError, AttachmentSource, UpsertAttachmentInput,
};
use colmena::llm::domain::{
    AttachmentRegistry, ConversationAttachment, FunctionCall, ProviderKind, ToolCall,
};
use colmena::llm::domain::{LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream};
use colmena::llm::infrastructure::persistence::SqliteAttachmentRegistry;
use colmena::llm::infrastructure::{
    ConversationRepositoryFactory, OverrideGuard, ScriptedAdapter, ScriptedResponse,
};
use colmena::storage::domain::{
    OutputStorageRepository, StorageError, StoreRequest, StoreStreamRequest, StoredBytes,
    StoredOutput, StoredStream,
};
use colmena::tabular_prepare::manifest::{
    part_path, ColumnInfo, ColumnType, Manifest, TableInfo, MANIFEST_PATH,
};
use colmena::tabular_prepare::ports::PrepareConfig;
use colmena::tabular_prepare::registry::{
    ClaimRequest, PreparationRegistry, ReadyInfo, FORMAT_VERSION,
};
use colmena::tabular_prepare::sqlite_registry::SqlitePreparationRegistry;
use colmena::tabular_prepare::TabularPrepare;
use colmena::tabular_run::runtime::LargeTabularRuntime;
use futures::StreamExt;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::collections::HashMap;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SOURCE: &str = "chat-attachments/u1/s1/doc-1";
const ROOT: &str = "chat-attachments/u1/s1/prepared/doc-1";

struct NoNodes;
impl NodeRegistryPort for NoNodes {
    fn get_node(&self, _: &str) -> Option<Arc<dyn ExecutableNode>> {
        None
    }
    fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
        HashMap::new()
    }
}

/// In-memory objects; `store_stream` keeps what it is given.
#[derive(Default)]
struct Storage {
    objects: Mutex<HashMap<String, Vec<u8>>>,
    stored: Mutex<Vec<(String, Vec<u8>)>>,
    deleted: Mutex<Vec<String>>,
}

#[async_trait]
impl OutputStorageRepository for Storage {
    async fn store(&self, _r: StoreRequest) -> Result<StoredOutput, StorageError> {
        unreachable!()
    }
    async fn read(&self, _k: &str) -> Result<StoredBytes, StorageError> {
        unreachable!()
    }
    async fn read_stream(&self, key: &str) -> Result<StoredStream, StorageError> {
        let bytes = self.objects.lock().unwrap().get(key).cloned().unwrap();
        let size = bytes.len() as u64;
        Ok(StoredStream {
            stream: Box::pin(futures::stream::iter(vec![Ok(Bytes::from(bytes))])),
            size_bytes: size,
            mime_type: "application/octet-stream".into(),
            filename: "object".into(),
        })
    }
    async fn delete(&self, k: &str) -> Result<(), StorageError> {
        self.deleted.lock().unwrap().push(k.to_string());
        Ok(())
    }
    fn derived_root(&self, _s: &str) -> Option<String> {
        Some(ROOT.to_string())
    }
    async fn store_stream(
        &self,
        mut req: StoreStreamRequest,
    ) -> Result<StoredOutput, StorageError> {
        let mut all = vec![];
        while let Some(chunk) = req.stream.next().await {
            all.extend_from_slice(&chunk?);
        }
        let size_bytes = all.len() as u64;
        self.stored
            .lock()
            .unwrap()
            .push((req.filename.clone(), all));
        Ok(StoredOutput {
            storage_key: format!("generated/{}", req.filename),
            read_url: String::new(),
            mime_type: req.mime_type,
            filename: req.filename,
            size_bytes,
        })
    }
}

fn gate() -> bool {
    if std::env::var("COLMENA_PYEXEC_JAIL_TESTS").as_deref() != Ok("1") {
        // The line the Linux job greps for; with the expect variable the test FAILS
        // instead (an environment that must run the jail suites cannot skip them).
        if std::env::var("COLMENA_PYEXEC_EXPECT_JAIL_TESTS").as_deref() == Ok("1") {
            panic!("COLMENA_PYEXEC_JAIL_TESTS=1 is required (COLMENA_PYEXEC_EXPECT_JAIL_TESTS=1)");
        }
        eprintln!("skipped: set COLMENA_PYEXEC_JAIL_TESTS=1 (Linux, root, CAP_SYS_ADMIN)");
        return false;
    }
    true
}

/// A Parquet part with `a` = `lo..=hi`, made with python3 and pyarrow.
fn part(dir: &std::path::Path, name: &str, lo: i32, hi: i32) -> Option<Vec<u8>> {
    let path = dir.join(name);
    let code = format!(
        "import sys, pyarrow as pa, pyarrow.parquet as pq; pq.write_table(pa.table({{'a': list(range({lo}, {hi} + 1))}}), sys.argv[1])"
    );
    let made = std::process::Command::new("python3")
        .args(["-c", &code, path.to_str().unwrap()])
        .status();
    if !matches!(made, Ok(s) if s.success()) {
        let message = "python3 with pyarrow is needed to make Parquet parts";
        if std::env::var("COLMENA_TABULAR_EXPECT_PANDAS").as_deref() == Ok("1") {
            panic!("{message} (COLMENA_TABULAR_EXPECT_PANDAS=1)");
        }
        eprintln!("skipped: {message}");
        return None;
    }
    std::fs::read(path).ok()
}

#[derive(Clone, Copy, PartialEq)]
#[repr(u32)]
enum Kind {
    /// Everything, one file returned.
    Whole,
    /// The conversation is at its file quota.
    Quota,
    /// Two files; the second registration fails.
    Rollback,
    /// The code runs past its own budget.
    Timeout,
    /// The whole path, through `data_run_python` (`output`, one binding).
    DataRun,
    /// The whole call runs past the tool's clock (shortened here from 900 s).
    Budget,
}

/// The registry, but the second `upsert` fails: a failure between two registrations.
struct FailsSecondUpsert {
    inner: Arc<SqliteAttachmentRegistry>,
    upserts: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl AttachmentRegistry for FailsSecondUpsert {
    async fn upsert(&self, input: UpsertAttachmentInput) -> Result<(), AttachmentError> {
        if self
            .upserts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            == 1
        {
            return Err(AttachmentError::RepositoryFailed("down".into()));
        }
        self.inner.upsert(input).await
    }
    async fn upsert_checked(
        &self,
        input: UpsertAttachmentInput,
    ) -> Result<colmena::llm::domain::attachments::UpsertOutcome, AttachmentError> {
        self.inner.upsert_checked(input).await
    }
    async fn lookup(
        &self,
        a: &str,
        d: &str,
        p: ProviderKind,
    ) -> Result<Option<ConversationAttachment>, AttachmentError> {
        self.inner.lookup(a, d, p).await
    }
    async fn refresh_provider_file_id(
        &self,
        a: &str,
        d: &str,
        p: ProviderKind,
        f: &str,
    ) -> Result<(), AttachmentError> {
        self.inner.refresh_provider_file_id(a, d, p, f).await
    }
    async fn update_description(
        &self,
        a: &str,
        d: &str,
        p: ProviderKind,
        t: &str,
    ) -> Result<(), AttachmentError> {
        self.inner.update_description(a, d, p, t).await
    }
    async fn list_for_session(
        &self,
        a: &str,
    ) -> Result<Vec<ConversationAttachment>, AttachmentError> {
        self.inner.list_for_session(a).await
    }
    async fn lookup_by_document_id(
        &self,
        a: &str,
        d: &str,
    ) -> Result<Option<ConversationAttachment>, AttachmentError> {
        self.inner.lookup_by_document_id(a, d).await
    }
    async fn touch_last_used(&self, a: &str, d: &str) -> Result<(), AttachmentError> {
        self.inner.touch_last_used(a, d).await
    }
    async fn find_stale_attachments(
        &self,
        q: colmena::llm::domain::attachments::StaleAttachmentQuery,
    ) -> Result<Vec<ConversationAttachment>, AttachmentError> {
        self.inner.find_stale_attachments(q).await
    }
    async fn delete_attachment(&self, a: &str, d: &str) -> Result<(), AttachmentError> {
        self.inner.delete_attachment(a, d).await
    }
}

#[tokio::test]
#[serial_test::serial(host_mounts)]
async fn the_whole_path_runs_with_nothing_canned() {
    scenario(Kind::Whole).await;
}

#[tokio::test]
#[serial_test::serial(host_mounts)]
async fn data_run_python_runs_the_whole_path_with_nothing_canned() {
    scenario(Kind::DataRun).await;
}

#[tokio::test]
#[serial_test::serial(host_mounts)]
async fn at_the_quota_the_result_is_returned_and_no_file_is_kept() {
    scenario(Kind::Quota).await;
}

#[tokio::test]
#[serial_test::serial(host_mounts)]
async fn a_failed_second_registration_rolls_both_files_back() {
    scenario(Kind::Rollback).await;
}

#[tokio::test]
#[serial_test::serial(host_mounts)]
async fn a_call_past_the_tools_clock_is_cut_off_and_gives_everything_back() {
    scenario(Kind::Budget).await;
}

#[tokio::test]
#[serial_test::serial(host_mounts)]
async fn code_past_its_budget_is_cut_off_and_the_volume_is_given_back() {
    scenario(Kind::Timeout).await;
}

async fn scenario(kind: Kind) {
    if !gate() {
        return;
    }
    pyo3::Python::initialize();
    let work = tempfile::tempdir().unwrap();
    let (Some(p0), Some(p1)) = (
        part(work.path(), "p0.parquet", 1, 5),
        part(work.path(), "p1.parquet", 6, 10),
    ) else {
        return;
    };
    // The prepared copy: real parts and manifest in storage, a ready row in a real registry.
    let storage = Arc::new(Storage::default());
    let manifest = Manifest::new(vec![TableInfo {
        name: "sales".into(),
        rows: 10,
        parts: 2,
        columns: vec![ColumnInfo {
            name: "a".into(),
            column_type: ColumnType::Int,
            uncompressed_bytes: 100,
            in_memory_bytes: 80,
        }],
    }]);
    let mut keys = vec![];
    let mut total = 0;
    for (i, bytes) in [p0, p1].into_iter().enumerate() {
        let key = format!("{ROOT}/{}", part_path(0, i).unwrap());
        total += bytes.len();
        storage.objects.lock().unwrap().insert(key.clone(), bytes);
        keys.push(key);
    }
    let manifest_key = format!("{ROOT}/{MANIFEST_PATH}");
    let json = manifest.to_json().unwrap();
    total += json.len();
    storage
        .objects
        .lock()
        .unwrap()
        .insert(manifest_key.clone(), json.into_bytes());
    keys.push(manifest_key.clone());
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(work.path().join("prep.db"))
                .create_if_missing(true),
        )
        .await
        .unwrap();
    sqlx::migrate!("migrations/sqlite")
        .run(&pool)
        .await
        .unwrap();
    let registry = Arc::new(SqlitePreparationRegistry::from_pool(Arc::new(pool)));
    registry
        .claim(ClaimRequest {
            source_key: SOURCE.into(),
            source_bytes: 60_000_000,
            format_version: FORMAT_VERSION,
            owner: "job".into(),
            lease: ChronoDuration::minutes(5),
            now: Utc::now(),
        })
        .await
        .unwrap()
        .unwrap();
    registry
        .complete(
            SOURCE,
            "job",
            ReadyInfo {
                manifest_key,
                blob_keys: keys,
                tables_json: manifest.tables_json().unwrap(),
                prepared_bytes: total as i64,
            },
            Utc::now(),
        )
        .await
        .unwrap();

    // The real executor in the real jail, as the runtime's mounted executor.
    // Its own root and uid range per scenario: the scenarios run in parallel.
    let index = kind as u32;
    let staging = PathBuf::from(format!(
        "/var/lib/colmena-tabular-e2e-test-{}-{index}",
        std::process::id()
    ));
    let made = std::fs::DirBuilder::new().mode(0o700).create(&staging);
    assert!(made.is_ok() || staging.is_dir(), "{made:?}");
    let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
    cfg.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
    cfg.slots = 1;
    cfg.uid_base = 63000 + 100 * index;
    cfg.max_response_bytes = 1 << 20;
    cfg.staging_root = Some(staging.clone());
    let exec = Arc::new(SubprocessExecutor::new(cfg, Duration::from_secs(60)).unwrap());
    // A template's start-up self-test looks at the host's mounts, so another scenario's
    // template start or volume mount at that moment reads as a leak: the tests of this
    // file take turns on the host (`serial`), each with its own root and uid range.
    exec.warm().await.unwrap();
    let config = PrepareConfig {
        large_tabular: true,
        ..PrepareConfig::default()
    };
    let mut runtime = LargeTabularRuntime::new(
        TabularPrepare::new(config, registry.clone()),
        registry,
        storage.clone(),
        exec.clone(),
    );
    if kind == Kind::Timeout {
        runtime = runtime.with_config(colmena::tabular_run::runtime::RuntimeConfig {
            heavy_timeout: Duration::from_secs(2),
            ..Default::default()
        });
    }

    // The tool, with a catalog row that is a host reference and a real attachment registry.
    let att_path = work.path().join("att.db");
    std::fs::File::create(&att_path).unwrap();
    let attachments_url = format!("sqlite://{}", att_path.display());
    let attachments = Arc::new(
        SqliteAttachmentRegistry::new(&attachments_url)
            .await
            .unwrap(),
    );
    let row = ConversationAttachment {
        agent_session_id: "agent_1".into(),
        document_id: "doc-1".into(),
        provider: ProviderKind::OpenAi,
        provider_file_id: String::new(),
        mime_type: "text/csv".into(),
        filename: "sales.csv".into(),
        size_bytes: Some(60 * 1024 * 1024),
        label: None,
        description: None,
        source: AttachmentSource::Path(SOURCE.into()),
        registered_at: Utc::now(),
        refreshed_at: Utc::now(),
        storage_key: Some(SOURCE.into()),
        origin: Some(origin::HOST_STORAGE_REF.into()),
        last_used_at: None,
    };
    if kind == Kind::Quota {
        // The conversation already holds the most files this tool may return.
        for i in 0..colmena::tabular_run::refusal::SESSION_MAX_FILES {
            attachments
                .upsert(UpsertAttachmentInput {
                    agent_session_id: "agent_1".into(),
                    document_id: format!("generated/old-{i}.csv"),
                    provider: ProviderKind::Generated,
                    provider_file_id: format!("generated/old-{i}.csv"),
                    mime_type: "text/csv".into(),
                    filename: "old.csv".into(),
                    size_bytes: Some(10),
                    label: None,
                    description: None,
                    source: AttachmentSource::Path("x".into()),
                    storage_key: Some(format!("generated/old-{i}.csv")),
                    origin: Some(origin::generated_by("attachment_run_python")),
                })
                .await
                .unwrap();
        }
    }
    let registry_for_tool: Arc<dyn AttachmentRegistry> = match kind {
        Kind::Rollback => Arc::new(FailsSecondUpsert {
            inner: attachments.clone(),
            upserts: Default::default(),
        }),
        _ => attachments.clone(),
    };
    let runtime = Arc::new(runtime);
    if kind == Kind::DataRun {
        // The node builds its catalog from the registry: the file is a host reference there.
        attachments
            .upsert(UpsertAttachmentInput {
                agent_session_id: "agent_1".into(),
                document_id: "doc-1".into(),
                provider: ProviderKind::OpenAi,
                provider_file_id: String::new(),
                mime_type: "text/csv".into(),
                filename: "sales.csv".into(),
                size_bytes: Some(60 * 1024 * 1024),
                label: None,
                description: None,
                source: AttachmentSource::Path(SOURCE.into()),
                storage_key: Some(SOURCE.into()),
                origin: Some(origin::HOST_STORAGE_REF.into()),
            })
            .await
            .unwrap();
    }
    let mut executor = DagToolExecutor::new(Arc::new(NoNodes), Default::default())
        .with_attachments(vec![row])
        .with_attachment_storage(storage.clone())
        .with_attachment_registry(registry_for_tool)
        .with_agent_session_id(Some("agent_1".into()))
        .with_large_tabular(runtime.clone());
    if kind == Kind::Budget {
        executor = executor.with_large_call_budget(Duration::from_secs(2));
    }
    let code = match kind {
        Kind::Rollback => {
            "emit_table(tables['sales'].head(2, columns=['a']), 'one')\n\
             emit_table(tables['sales'].head(3, columns=['a']), 'two')\nresult = 1"
        }
        Kind::Timeout | Kind::Budget => "while True:\n    pass",
        Kind::Quota => "emit_table(tables['sales'].head(3, columns=['a']), 'top')\nresult = 7",
        Kind::Whole => {
            "emit_table(tables['sales'].head(3, columns=['a']), 'top')\n\
             result = int(tables['sales'].read(columns=['a'])['a'].sum())"
        }
        Kind::DataRun => {
            "emit_table(tables['sales'].head(3, columns=['a']), 'top')\n\
             output = int(tables['sales'].read(columns=['a'])['a'].sum())"
        }
    };
    let call = ToolCall {
        id: "call-1".into(),
        call_type: "function".into(),
        function: FunctionCall::new(
            "attachment_run_python".into(),
            serde_json::json!({"attachment_id": "doc-1", "code": code}).to_string(),
        ),
        response: None,
        provider_signature: None,
        scope_index: None,
    };
    let answer: Value = if kind == Kind::DataRun {
        // Through the real node: its offering rule, its routing, its agent loop.
        let (answer, _seen) =
            through_the_node(runtime.clone(), storage.clone(), &attachments_url, code).await;
        answer
    } else {
        let result = dispatch_attachment_run_python_via_executor(&executor, &call)
            .await
            .unwrap();
        serde_json::from_str(&result.output).unwrap()
    };

    match kind {
        Kind::Quota => {
            assert_eq!(answer["result"], 7, "{answer}");
            assert!(answer.get("emitted").is_none(), "{answer}");
            assert!(
                answer["not_kept"]
                    .to_string()
                    .contains("return results in the answer"),
                "{answer}"
            );
            assert!(storage_is_empty_after(&storage).await);
        }
        Kind::Rollback => {
            assert_eq!(answer["result"], 1, "{answer}");
            assert!(answer.get("emitted").is_none(), "{answer}");
            assert!(
                answer["not_kept"].to_string().contains("none was kept"),
                "{answer}"
            );
            // Both files were stored; both rows are gone and no object is reported kept.
            let rows = attachments.list_for_session("agent_1").await.unwrap();
            assert!(
                rows.iter()
                    .all(|r| r.document_id.starts_with("generated/old")),
                "{rows:?}"
            );
            assert_eq!(storage.deleted.lock().unwrap().len(), 2);
        }
        Kind::DataRun => {
            // `output` is the answer, the tables are the prepared ones, the returned
            // file is an attachment tagged with this tool's name, and nothing was
            // read from the original.
            assert_eq!(answer["result"], 55, "{answer}");
            assert_eq!(answer["tables"][0]["name"], "sales");
            assert_eq!(answer["emitted"][0]["name"], "top.csv", "{answer}");
            let id = answer["emitted"][0]["document_id"].as_str().unwrap();
            let row = attachments
                .lookup_by_document_id("agent_1", id)
                .await
                .unwrap()
                .expect("registered");
            assert_eq!(
                row.origin.as_deref(),
                Some("generated_by:data_run_python_large")
            );
        }
        Kind::Budget => {
            // Cut by the call's clock, not the code's: the answer names the clock and
            // the phase it reached, keeps nothing and is not retryable as it is.
            assert_eq!(answer["retryable"], false, "{answer}");
            let text = answer["error"].as_str().unwrap();
            assert!(text.contains("did not finish within 2s"), "{answer}");
            assert!(text.contains("nothing was kept"), "{answer}");
        }
        Kind::Timeout => {
            assert_eq!(answer["retryable"], false, "{answer}");
            assert!(
                answer["error"].as_str().unwrap().contains("timeout"),
                "{answer}"
            );
        }
        Kind::Whole => {
            assert_eq!(answer["result"], 55, "{answer}");
            assert_eq!(answer["tables"][0]["name"], "sales");
            assert_eq!(answer["emitted"][0]["name"], "top.csv", "{answer}");
            assert_eq!(answer["emitted"][0]["rows_reported_by_code"], 3);
            // The stored name is the file's own behind an id unique to the call.
            let stored = storage.stored.lock().unwrap().clone();
            assert_eq!(stored.len(), 1);
            assert!(stored[0].0.ends_with("-top.csv") && stored[0].0.len() == "top.csv".len() + 13);
            assert_eq!(stored[0].1, b"a\n1\n2\n3\n");
            let id = answer["emitted"][0]["document_id"]
                .as_str()
                .unwrap()
                .to_string();
            assert_eq!(id, format!("generated/{}", stored[0].0));
            let registered = attachments
                .lookup_by_document_id("agent_1", &id)
                .await
                .unwrap()
                .expect("the returned file is an attachment of the session");
            assert!(!registered.is_host_storage_ref());
            assert!(
                !answer.to_string().contains(SOURCE),
                "no key in the answer: {answer}"
            );
        }
    }
    // The volume was given back and nothing is left on the staging root.
    for _ in 0..50 {
        if exec.staged_in_flight() == (0, 0) && std::fs::read_dir(&staging).unwrap().count() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the volume was not given back");
}

/// Nothing stored survives: every returned file was deleted (the fake keeps a list).
async fn storage_is_empty_after(storage: &Storage) -> bool {
    for _ in 0..30 {
        let stored = storage.stored.lock().unwrap().len();
        let deleted = storage.deleted.lock().unwrap().len();
        if stored == deleted {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// A model that plays a script and keeps every message it was sent.
struct Recording {
    inner: ScriptedAdapter,
    seen: Mutex<Vec<String>>,
}

impl Recording {
    fn note(&self, request: &LlmRequest) {
        let mut seen = self.seen.lock().unwrap();
        seen.extend(request.messages().iter().map(|m| m.content().to_string()));
    }
}

#[async_trait]
impl LlmRepository for Recording {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        self.note(&request);
        self.inner.call(request).await
    }
    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        self.note(&request);
        self.inner.stream(request).await
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
    fn provider_name(&self) -> &'static str {
        "recording"
    }
}

/// `data_run_python` over the large file THROUGH THE REAL `llm_call` NODE: the node's own
/// offering rule decides the tools and the routing, the scripted model calls the tool by
/// name, and the call reaches the real runtime and jail. Returns the tool's answer as the
/// model received it, and the tools the node offered.
async fn through_the_node(
    runtime: Arc<LargeTabularRuntime>,
    storage: Arc<dyn OutputStorageRepository>,
    attachments_url: &str,
    code: &str,
) -> (Value, Vec<String>) {
    // With DATABASE_URL set the node would keep its attachments in that Postgres, not in
    // the SQLite database of this scenario. Every test of this file is serial.
    std::env::remove_var("DATABASE_URL");
    let pools = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
    let repos = Arc::new(ConversationRepositoryFactory::new(pools.clone()));
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    let task_memory: Arc<dyn DagTaskMemoryRepository> =
        Arc::new(PostgresDagStateRepository::new(pool));
    let reg = HashMapNodeRegistry::new_with_secure_values(
        repos,
        Arc::new(SqlPortFactory::new(pools)),
        Some(task_memory),
        None,
        Some(storage),
        None,
        None,
    );
    reg.set_large_tabular(true);
    reg.set_large_tabular_runtime(runtime);
    let node = reg.get_node("llm_call").expect("llm_call is registered");
    let config = serde_json::json!({
        "provider": "openai", "model": "m", "api_key": "k", "stream": false,
        "prompt": "go", "connection_url": attachments_url,
        "tool_configurations": {"data_run_python": {"node_type": "data_run_python"}},
    });
    let inputs = |files: Value| -> HashMap<String, Value> {
        HashMap::from([
            ("__colmena_session_id".to_string(), "s1".into()),
            ("__colmena_agent_session_id".to_string(), "agent_1".into()),
            ("files".to_string(), files),
        ])
    };
    let script = |responses| {
        Arc::new(Recording {
            inner: ScriptedAdapter::new(responses),
            seen: Mutex::default(),
        })
    };
    // Turn 1 only registers the file the way a host does; the row is already there from
    // the catalog row of the scenario, so the model just answers.
    let first = script(vec![ScriptedResponse::Text("ok".into())]);
    {
        let _guard = OverrideGuard::install(first.clone());
        node.execute(
            &inputs(Value::Array(vec![])),
            &config,
            &mut Value::Null,
            None,
        )
        .await
        .unwrap();
    }
    let second = script(vec![
        ScriptedResponse::ToolCall {
            id: "call-1".into(),
            tool_name: "data_run_python".into(),
            arguments: serde_json::json!({
                "bindings": [{"var": "big", "attachment_id": "doc-1"}],
                "code": code,
            }),
        },
        ScriptedResponse::Text("ok".into()),
    ]);
    {
        let _guard = OverrideGuard::install(second.clone());
        node.execute(
            &inputs(Value::Array(vec![])),
            &config,
            &mut Value::Null,
            None,
        )
        .await
        .unwrap();
    }
    let seen = second.seen.lock().unwrap().clone();
    let answer = seen
        .iter()
        .rev()
        .find_map(|m| {
            serde_json::from_str::<Value>(m)
                .ok()
                .filter(|v| v.get("tables").is_some())
        })
        .unwrap_or_else(|| panic!("the tool's answer reached the model: {seen:?}"));
    (answer, seen)
}
