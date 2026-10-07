#![cfg(target_os = "linux")]
//! A call over prepared tables on the real subprocess executor: the parts are
//! staged through the budgeted path and the code reads them at `/data`. Same
//! gate as the other jail suites: root and CAP_SYS_ADMIN, enabled with
//! `COLMENA_PYEXEC_JAIL_TESTS=1`. A staging root is shared by the suite.

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{Duration as ChronoDuration, Utc};
use colmena::dag_engine::domain::python_executor::{PythonRunError, PythonRunRequest};
use colmena::dag_engine::infrastructure::python_exec::config::SubprocessConfig;
use colmena::dag_engine::infrastructure::python_exec::subprocess::SubprocessExecutor;
use colmena::storage::domain::{
    OutputStorageRepository, StorageError, StoreRequest, StoredBytes, StoredOutput, StoredStream,
};
use colmena::tabular_prepare::manifest::{
    part_path, ColumnInfo, ColumnType, Manifest, TableInfo, MANIFEST_PATH,
};
use colmena::tabular_prepare::registry::{
    ClaimRequest, PreparationRegistry, ReadyInfo, FORMAT_VERSION,
};
use colmena::tabular_prepare::sqlite_registry::SqlitePreparationRegistry;
use colmena::tabular_run::mounted::{MountedCall, MountedError, MountedExecutor};
use colmena::tabular_run::refusal::{Budget, RunRefusal, Unavailable};
use colmena::tabular_run::stage::StageLimits;
use colmena::tabular_run::verify::{verify_prepared, PreparedTables};
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::collections::HashMap;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SOURCE: &str = "chat-attachments/u1/s1/doc-1";
const ROOT: &str = "chat-attachments/u1/s1/prepared/doc-1";

fn enabled() -> bool {
    std::env::var("COLMENA_PYEXEC_JAIL_TESTS").as_deref() == Ok("1")
}

fn staging_root() -> Option<PathBuf> {
    if !enabled() {
        eprintln!("skipped: set COLMENA_PYEXEC_JAIL_TESTS=1 (Linux, root, CAP_SYS_ADMIN)");
        return None;
    }
    let path = PathBuf::from("/var/lib/colmena-tabular-run-test");
    let made = std::fs::DirBuilder::new().mode(0o700).create(&path);
    assert!(made.is_ok() || path.is_dir(), "{made:?}");
    Some(path)
}

fn executor(root: Option<&PathBuf>) -> SubprocessExecutor {
    pyo3::Python::initialize();
    let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
    cfg.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
    cfg.slots = 1;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    cfg.uid_base = 62000 + 100 * NEXT.fetch_add(1, Ordering::Relaxed);
    cfg.max_response_bytes = 1 << 20;
    cfg.staging_root = root.cloned();
    SubprocessExecutor::new(cfg, Duration::from_secs(60)).unwrap()
}

/// Objects in memory; records what was read. `derived_root` answers as a host's.
struct Storage {
    objects: Mutex<HashMap<String, Vec<u8>>>,
    reads: Mutex<Vec<String>>,
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
        self.reads.lock().unwrap().push(key.to_string());
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
        unreachable!()
    }
    fn derived_root(&self, _s: &str) -> Option<String> {
        Some(ROOT.to_string())
    }
}

/// A ready copy of one table of `parts` parts (each `part_bytes[i]`), verified
/// through a real registry.
async fn prepared(parts: &[Vec<u8>]) -> (Arc<Storage>, PreparedTables, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let options = SqliteConnectOptions::new()
        .filename(dir.path().join("registry.db"))
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!("migrations/sqlite")
        .run(&pool)
        .await
        .unwrap();
    let registry = SqlitePreparationRegistry::from_pool(Arc::new(pool));
    let manifest = Manifest::new(vec![TableInfo {
        name: "sales".into(),
        rows: parts.len() as u64 * 10,
        parts: parts.len() as u32,
        columns: vec![ColumnInfo {
            name: "a".into(),
            column_type: ColumnType::Int,
            uncompressed_bytes: 100,
            in_memory_bytes: 10_000,
        }],
    }]);
    let storage = Arc::new(Storage {
        objects: Mutex::new(HashMap::new()),
        reads: Mutex::new(vec![]),
    });
    let mut keys = vec![];
    let mut total = 0;
    for (p, bytes) in parts.iter().enumerate() {
        let key = format!("{ROOT}/{}", part_path(0, p).unwrap());
        storage
            .objects
            .lock()
            .unwrap()
            .insert(key.clone(), bytes.clone());
        total += bytes.len();
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
    let plan = verify_prepared(&registry, &*storage, SOURCE).await.unwrap();
    storage.reads.lock().unwrap().clear();
    (storage, plan, dir)
}

fn req(code: &str, mode: &str) -> PythonRunRequest {
    PythonRunRequest {
        code: code.into(),
        mode: mode.into(),
        timeout: Some(Duration::from_secs(60)),
        inputs: Default::default(),
    }
}

fn call<'a>(
    storage: &'a Storage,
    plan: &'a PreparedTables,
    limits: StageLimits,
) -> MountedCall<'a> {
    MountedCall {
        storage,
        plan,
        tables: &[0],
        limits,
        out_mb: 4,
    }
}

fn leftovers(root: &PathBuf) -> usize {
    std::fs::read_dir(root).unwrap().count()
}

#[tokio::test]
async fn the_code_reads_the_staged_parts_at_data_and_cannot_write_there() {
    let Some(root) = staging_root() else { return };
    let ex = executor(Some(&root));
    let (storage, plan, _dir) = prepared(&[vec![b'a'; 100], vec![b'b'; 50]]).await;
    let code = r#"
import os
parts = sorted(os.listdir('/data/t0'))
sizes = [os.path.getsize('/data/t0/' + p) for p in parts]
manifest = open('/data/manifest.json').read()
try:
    open('/data/t0/new', 'w').write('x')
    wrote = True
except OSError:
    wrote = False
output = {'parts': parts, 'sizes': sizes, 'manifest_has_sales': 'sales' in manifest, 'wrote': wrote}
"#;
    let out = ex
        .run_with_mounts(req(code, "none"), call(&storage, &plan, Default::default()))
        .await
        .unwrap();
    assert_eq!(
        out.result.output,
        Some(json!({
            "parts": ["part-00000.parquet", "part-00001.parquet"],
            "sizes": [100, 50],
            "manifest_has_sales": true,
            "wrote": false,
        }))
    );
    assert_eq!(out.staged.parts, 2);
    assert_eq!(
        ex.staged_in_flight(),
        (0, 0),
        "the budget share was given back"
    );
    assert_eq!(leftovers(&root), 0, "the call's directories were removed");
}

/// What the real sandbox says about `read_parquet` on a part the trusted side
/// staged. Needs python3 with pyarrow in the test environment to make the part.
#[tokio::test]
async fn pandas_reads_a_staged_parquet_part_in_restricted_mode() {
    let Some(root) = staging_root() else { return };
    let dir = tempfile::tempdir().unwrap();
    let part = dir.path().join("p.parquet");
    let made = std::process::Command::new("python3")
        .args([
            "-c",
            "import sys, pyarrow as pa, pyarrow.parquet as pq; \
             pq.write_table(pa.table({'a': list(range(1, 11))}), sys.argv[1])",
            part.to_str().unwrap(),
        ])
        .status();
    if !matches!(made, Ok(s) if s.success()) {
        eprintln!("skipped: python3 with pyarrow is needed to make a Parquet part");
        return;
    }
    let ex = executor(Some(&root));
    let (storage, plan, _d) = prepared(&[std::fs::read(&part).unwrap()]).await;
    let code = "import pandas as pd\n\
                df = pd.read_parquet('/data/t0/part-00000.parquet', columns=['a'], use_threads=False)\n\
                output = int(df['a'].sum())\n";
    let out = ex
        .run_with_mounts(
            req(code, "restricted"),
            call(&storage, &plan, Default::default()),
        )
        .await
        .unwrap();
    assert_eq!(out.result.output, Some(json!(55)));
}

#[tokio::test]
async fn an_executor_without_a_staging_root_refuses_and_reads_nothing() {
    if staging_root().is_none() {
        return;
    }
    let ex = executor(None);
    let (storage, plan, _d) = prepared(&[vec![b'a'; 10]]).await;
    let err = ex
        .run_with_mounts(
            req("output = 1", "none"),
            call(&storage, &plan, Default::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        MountedError::Refused(RunRefusal::Unavailable(Unavailable::NoStagingRoot))
    );
    assert!(storage.reads.lock().unwrap().is_empty(), "nothing was read");
}

#[tokio::test]
async fn a_call_over_the_data_limit_is_refused_and_the_volume_is_given_back() {
    let Some(root) = staging_root() else { return };
    let ex = executor(Some(&root));
    let (storage, plan, _d) = prepared(&[vec![b'a'; 100], vec![b'b'; 100]]).await;
    let limits = StageLimits {
        total_bytes: 150,
        part_bytes: 100,
    };
    let err = ex
        .run_with_mounts(req("output = 1", "none"), call(&storage, &plan, limits))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            MountedError::Refused(RunRefusal::OverBudget(Budget::Data { .. }))
        ),
        "{err:?}"
    );
    assert_eq!(ex.staged_in_flight(), (0, 0));
    assert_eq!(leftovers(&root), 0);
}

/// The executor's budget of volumes in flight answers a typed refusal, not a
/// crash: two calls hold the budget, the third is told to retry.
#[tokio::test]
async fn a_call_over_the_executors_volume_budget_is_refused_and_stages_nothing() {
    let Some(root) = staging_root() else { return };
    let ex = executor(Some(&root));
    let a = ex.stage_call(4).unwrap();
    let b = ex.stage_call(4).unwrap();
    let (storage, plan, _d) = prepared(&[vec![b'a'; 10]]).await;
    let err = ex
        .run_with_mounts(
            req("output = 1", "none"),
            call(&storage, &plan, Default::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        MountedError::Refused(RunRefusal::OverBudget(Budget::Volumes))
    );
    assert!(storage.reads.lock().unwrap().is_empty());
    drop((a, b));
    assert_eq!(ex.staged_in_flight(), (0, 0));
}

#[tokio::test]
async fn the_codes_own_failure_is_reported_as_any_call_reports_it() {
    let Some(root) = staging_root() else { return };
    let ex = executor(Some(&root));
    let (storage, plan, _d) = prepared(&[vec![b'a'; 10]]).await;
    let err = ex
        .run_with_mounts(
            req("raise ValueError('boom')", "none"),
            call(&storage, &plan, Default::default()),
        )
        .await
        .unwrap_err();
    match err {
        MountedError::Run(PythonRunError::Python(text)) => assert!(text.contains("boom"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(ex.staged_in_flight(), (0, 0));
}
