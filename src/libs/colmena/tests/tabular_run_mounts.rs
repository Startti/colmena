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
use colmena::tabular_run::collect::{OutFile, RejectReason};
use colmena::tabular_run::mounted::{MountedCall, MountedError, MountedExecutor, OutputSink};
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
        sink: None,
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
        ..StageLimits::default()
    };
    let err = ex
        .run_with_mounts(req("output = 1", "none"), call(&storage, &plan, limits))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            MountedError::Refused(RunRefusal::OverBudget(Budget::Table { .. }))
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

/// Collects the names of the outputs it is given and reads each one.
#[derive(Default)]
struct Names(Mutex<Vec<(String, Vec<u8>)>>);

#[async_trait]
impl OutputSink for Names {
    async fn accept(&self, file: OutFile) -> Result<(), RunRefusal> {
        use std::io::Read;
        let name = file.name.clone();
        let mut bytes = vec![];
        file.into_file().read_to_end(&mut bytes).unwrap();
        self.0.lock().unwrap().push((name, bytes));
        Ok(())
    }
}

/// What the code leaves in `/out` is hostile: a link to a host file, a pipe, two
/// names for one file and a name outside the charset reach the sink as nothing;
/// the one good file arrives whole; the call's volume is still given back.
#[tokio::test]
async fn hostile_outputs_never_reach_the_sink_and_the_good_one_does() {
    let Some(root) = staging_root() else { return };
    let ex = executor(Some(&root));
    let (storage, plan, _d) = prepared(&[vec![b'a'; 10]]).await;
    let code = r#"
import os
open('/out/good.csv', 'w').write('a\n1\n')
open('/out/two.csv', 'w').write('x')
def attempt(f):
    try:
        f()
    except OSError:
        pass
attempt(lambda: os.link('/out/two.csv', '/out/three.csv'))
attempt(lambda: os.symlink('/etc/hostname', '/out/link.csv'))
attempt(lambda: os.mkfifo('/out/pipe.csv'))
open('/out/bad name.csv', 'w').write('x')
output = 1
"#;
    let sink = Names::default();
    let mut c = call(&storage, &plan, Default::default());
    c.sink = Some(&sink);
    let out = ex.run_with_mounts(req(code, "none"), c).await.unwrap();
    let got = sink.0.lock().unwrap().clone();
    assert_eq!(got, [("good.csv".to_string(), b"a\n1\n".to_vec())]);
    assert_eq!(out.emitted, ["good.csv"]);
    let reasons: Vec<_> = out
        .rejected
        .iter()
        .map(|r| (r.name.clone(), r.reason))
        .collect();
    assert!(
        reasons.contains(&(None, RejectReason::BadName)),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&(Some("two.csv".into()), RejectReason::HardLinked))
            || reasons.iter().all(|r| r.1 != RejectReason::HardLinked),
        "{reasons:?}"
    );
    assert!(!reasons.iter().any(|r| r.0.as_deref() == Some("good.csv")));
    assert_eq!(ex.staged_in_flight(), (0, 0));
    assert_eq!(leftovers(&root), 0);
}

/// What a server asks before it reads a call's data: an executor without a
/// staging root says so, one with a root and a healthy template says nothing.
#[tokio::test]
async fn an_executor_says_why_it_cannot_take_a_mounts_call() {
    let Some(root) = staging_root() else { return };
    assert_eq!(
        executor(None).mounts_unavailable().await.as_deref(),
        Some("no_staging_root")
    );
    assert_eq!(executor(Some(&root)).mounts_unavailable().await, None);
}

// ---- the order of things, and the volume on every path ----

/// What a sink does when it is given a file.
enum Act {
    /// Look at the executor's budget, to see the volume is still held.
    Observe(Arc<SubprocessExecutor>),
    Fail,
    Panic,
    Hang,
}

struct ActingSink {
    act: Act,
    seen: Mutex<Vec<(usize, u64)>>,
}

#[async_trait]
impl OutputSink for ActingSink {
    async fn accept(&self, _file: OutFile) -> Result<(), RunRefusal> {
        match &self.act {
            Act::Observe(ex) => {
                self.seen.lock().unwrap().push(ex.staged_in_flight());
                Ok(())
            }
            Act::Fail => Err(RunRefusal::Storage),
            Act::Panic => panic!("a sink that panics"),
            Act::Hang => std::future::pending().await,
        }
    }
}

const WRITES_A_FILE: &str = "open('/out/a.csv', 'w').write('x')\noutput = 1";

async fn with_sink(
    act: impl FnOnce(Arc<SubprocessExecutor>) -> Act,
) -> (
    Arc<SubprocessExecutor>,
    Arc<ActingSink>,
    PathBuf,
    Arc<Storage>,
    PreparedTables,
    tempfile::TempDir,
) {
    let root = staging_root().expect("enabled");
    let ex = Arc::new(executor(Some(&root)));
    let sink = Arc::new(ActingSink {
        act: act(ex.clone()),
        seen: Mutex::new(vec![]),
    });
    let (storage, plan, dir) = prepared(&[vec![b'a'; 10]]).await;
    (ex, sink, root, storage, plan, dir)
}

async fn eventually_released(ex: &SubprocessExecutor, root: &PathBuf) {
    for _ in 0..100 {
        if ex.staged_in_flight() == (0, 0) && leftovers(root) == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the volume was not given back: {:?}", ex.staged_in_flight());
}

/// The sink is given the outputs while the volume is still held (it is the
/// outputs' last chance to be read), and the volume is given back after.
#[tokio::test]
async fn the_sink_runs_before_the_volume_is_released() {
    if staging_root().is_none() {
        return;
    }
    let (ex, sink, root, storage, plan, _d) = with_sink(Act::Observe).await;
    let mut c = call(&storage, &plan, Default::default());
    c.sink = Some(&*sink);
    ex.run_with_mounts(req(WRITES_A_FILE, "none"), c)
        .await
        .unwrap();
    assert_eq!(
        sink.seen.lock().unwrap().as_slice(),
        &[(1, 4)],
        "held while the sink ran"
    );
    eventually_released(&ex, &root).await;
}

#[tokio::test]
async fn a_sink_that_fails_still_gives_the_volume_back() {
    if staging_root().is_none() {
        return;
    }
    let (ex, sink, root, storage, plan, _d) = with_sink(|_| Act::Fail).await;
    let mut c = call(&storage, &plan, Default::default());
    c.sink = Some(&*sink);
    let err = ex
        .run_with_mounts(req(WRITES_A_FILE, "none"), c)
        .await
        .unwrap_err();
    assert_eq!(err, MountedError::Refused(RunRefusal::Storage));
    eventually_released(&ex, &root).await;
}

#[tokio::test]
async fn a_sink_that_panics_still_gives_the_volume_back() {
    if staging_root().is_none() {
        return;
    }
    let (ex, sink, root, storage, plan, _d) = with_sink(|_| Act::Panic).await;
    let ex2 = ex.clone();
    let task = tokio::spawn(async move {
        let mut c = call(&storage, &plan, Default::default());
        c.sink = Some(&*sink);
        ex2.run_with_mounts(req(WRITES_A_FILE, "none"), c)
            .await
            .map(|_| ())
    });
    assert!(task.await.unwrap_err().is_panic());
    eventually_released(&ex, &root).await;
}

/// The call's future dropped while the sink waits: the volume is released from
/// its guard, on the blocking pool, not left mounted.
#[tokio::test]
async fn a_call_cancelled_while_the_sink_waits_gives_the_volume_back() {
    if staging_root().is_none() {
        return;
    }
    let (ex, sink, root, storage, plan, _d) = with_sink(|_| Act::Hang).await;
    let mut c = call(&storage, &plan, Default::default());
    c.sink = Some(&*sink);
    let cut = tokio::time::timeout(
        Duration::from_secs(5),
        ex.run_with_mounts(req(WRITES_A_FILE, "none"), c),
    )
    .await;
    assert!(cut.is_err(), "still waiting on the sink when it was cut");
    eventually_released(&ex, &root).await;
}

/// What the code leaves running does not survive into the read: the program starts
/// a background process that keeps writing to `/out`, and returns. By the time the
/// sink is given the file, nothing of the call's uid is left to change it (the
/// executor confirmed that before returning the result).
#[tokio::test]
async fn nothing_of_the_call_is_still_running_when_its_output_is_read() {
    if staging_root().is_none() {
        return;
    }
    let root = staging_root().unwrap();
    let ex = executor(Some(&root));
    let (storage, plan, _d) = prepared(&[vec![b'a'; 10]]).await;
    struct Watch(Mutex<Vec<(u64, u64)>>);
    #[async_trait]
    impl OutputSink for Watch {
        async fn accept(&self, file: OutFile) -> Result<(), RunRefusal> {
            let f = file.into_file();
            let first = f.metadata().unwrap().len();
            tokio::time::sleep(Duration::from_millis(400)).await;
            self.0
                .lock()
                .unwrap()
                .push((first, f.metadata().unwrap().len()));
            Ok(())
        }
    }
    // The sandbox does not let the code fork (the jail denies it), so what could
    // outlive the answer is its own threads and whatever it spawns where it can:
    // a writer thread is started and the program returns while it is writing.
    let code = r#"
import threading, time
open('/out/grow.csv', 'w').write('x')
def writer():
    f = open('/out/grow.csv', 'a')
    while True:
        f.write('y' * 100)
        f.flush()
        time.sleep(0.02)
threading.Thread(target=writer, daemon=True).start()
time.sleep(0.2)
output = 1
"#;
    let sink = Watch(Mutex::new(vec![]));
    let mut c = call(&storage, &plan, Default::default());
    c.sink = Some(&sink);
    ex.run_with_mounts(req(code, "none"), c).await.unwrap();
    let seen = sink.0.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].0, seen[0].1,
        "the file grew while it was being read: {seen:?}"
    );
}
