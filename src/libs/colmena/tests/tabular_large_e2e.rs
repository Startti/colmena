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
use colmena::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_run_python::dispatch_attachment_run_python_via_executor;
use colmena::dag_engine::infrastructure::python_exec::config::SubprocessConfig;
use colmena::dag_engine::infrastructure::python_exec::subprocess::SubprocessExecutor;
use colmena::llm::domain::attachments::{origin, AttachmentSource};
use colmena::llm::domain::{
    AttachmentRegistry, ConversationAttachment, FunctionCall, ProviderKind, ToolCall,
};
use colmena::llm::infrastructure::persistence::SqliteAttachmentRegistry;
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
    async fn delete(&self, _k: &str) -> Result<(), StorageError> {
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

#[tokio::test]
async fn the_whole_path_runs_with_nothing_canned() {
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
    let staging = PathBuf::from("/var/lib/colmena-tabular-e2e-test");
    let made = std::fs::DirBuilder::new().mode(0o700).create(&staging);
    assert!(made.is_ok() || staging.is_dir(), "{made:?}");
    let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
    cfg.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
    cfg.slots = 1;
    cfg.uid_base = 63000;
    cfg.max_response_bytes = 1 << 20;
    cfg.staging_root = Some(staging.clone());
    let exec = Arc::new(SubprocessExecutor::new(cfg, Duration::from_secs(60)).unwrap());
    let config = PrepareConfig {
        large_tabular: true,
        ..PrepareConfig::default()
    };
    let runtime = LargeTabularRuntime::new(
        TabularPrepare::new(config, registry.clone()),
        registry,
        storage.clone(),
        exec.clone(),
    );

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
    let executor = DagToolExecutor::new(Arc::new(NoNodes), Default::default())
        .with_attachments(vec![row])
        .with_attachment_storage(storage.clone())
        .with_attachment_registry(attachments.clone())
        .with_agent_session_id(Some("agent_1".into()))
        .with_large_tabular(Arc::new(runtime));
    let code = "emit_table(tables['sales'].head(3, columns=['a']), 'top')\n\
                result = int(tables['sales'].read(columns=['a'])['a'].sum())";
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
    let result = dispatch_attachment_run_python_via_executor(&executor, &call)
        .await
        .unwrap();
    let answer: Value = serde_json::from_str(&result.output).unwrap();

    assert_eq!(answer["result"], 55, "{answer}");
    assert_eq!(answer["tables"][0]["name"], "sales");
    assert_eq!(answer["emitted"][0]["name"], "top.csv", "{answer}");
    assert_eq!(answer["emitted"][0]["rows_reported_by_code"], 3);
    assert_eq!(
        *storage.stored.lock().unwrap(),
        [("top.csv".to_string(), b"a\n1\n2\n3\n".to_vec())]
    );
    let registered = attachments
        .lookup_by_document_id("agent_1", "generated/top.csv")
        .await
        .unwrap()
        .expect("the returned file is an attachment of the session");
    assert!(!registered.is_host_storage_ref());
    assert!(
        !answer.to_string().contains(SOURCE),
        "no key in the answer: {answer}"
    );
    // The volume was given back and nothing is left on the staging root.
    for _ in 0..50 {
        if exec.staged_in_flight() == (0, 0) && std::fs::read_dir(&staging).unwrap().count() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the volume was not given back");
}
