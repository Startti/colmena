//! Fixtures shared by the tests of this module: a storage that holds the
//! prepared objects in memory, and a real SQLite registry holding a ready row.

use super::mounted::{MountedCall, MountedError, MountedExecutor, MountedResult};
use super::runtime::{LargeTabularRuntime, RuntimeConfig};
use super::stage::Staged;
use crate::dag_engine::domain::python_executor::{PythonRunRequest, PythonRunResult};
use crate::storage::domain::{
    OutputStorageRepository, StorageError, StoreRequest, StoredBytes, StoredOutput, StoredStream,
};
use crate::tabular_prepare::manifest::{
    part_path, ColumnInfo, ColumnType, Manifest, TableInfo, MANIFEST_PATH,
};
use crate::tabular_prepare::ports::PrepareConfig;
use crate::tabular_prepare::registry::{
    ClaimRequest, PreparationRegistry, ReadyInfo, FORMAT_VERSION,
};
use crate::tabular_prepare::sqlite_registry::SqlitePreparationRegistry;
use crate::tabular_prepare::TabularPrepare;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{Duration, Utc};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub(crate) const SOURCE: &str = "chat-attachments/u1/s1/doc-1";
pub(crate) const ROOT: &str = "chat-attachments/u1/s1/prepared/doc-1";

/// Storage holding objects in memory. It answers `derived_root` as the host
/// does, serves streams in chunks of `chunk` bytes, and records what was read.
pub(crate) struct FakeStorage {
    pub objects: Mutex<HashMap<String, Vec<u8>>>,
    pub root: Option<String>,
    pub chunk: usize,
    pub reads: Mutex<Vec<String>>,
    /// What `store_stream` was given: file name and bytes.
    pub stored: Mutex<Vec<(String, Vec<u8>)>>,
    /// The file names `store_stream` was really given (with the call's prefix).
    pub stored_filenames: Mutex<Vec<String>>,
    /// Keys `delete` was asked for.
    pub deleted: Arc<Mutex<Vec<String>>>,
    /// Fault injection for `store_stream`: fail the nth call (1-based), or
    /// stall it for a while.
    pub store_fail_on: Mutex<Option<usize>>,
    pub store_stall_on: Mutex<Option<(usize, std::time::Duration)>>,
    pub store_calls: Mutex<usize>,
    /// Keys that fail to read.
    pub broken: Mutex<Vec<String>>,
    /// Keys served by a generator instead of the bytes in `objects`.
    #[allow(clippy::type_complexity)]
    pub custom: Mutex<HashMap<String, Box<dyn Fn() -> StoredStream + Send + Sync>>>,
}

impl FakeStorage {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            objects: Mutex::new(HashMap::new()),
            root: Some(ROOT.to_string()),
            chunk: 1024,
            reads: Mutex::new(vec![]),
            stored: Mutex::new(vec![]),
            deleted: Arc::default(),
            stored_filenames: Mutex::new(vec![]),
            store_fail_on: Mutex::new(None),
            store_stall_on: Mutex::new(None),
            store_calls: Mutex::new(0),
            broken: Mutex::new(vec![]),
            custom: Mutex::new(HashMap::new()),
        })
    }

    pub fn without_root() -> Arc<Self> {
        Arc::new(Self {
            root: None,
            ..Arc::try_unwrap(Self::new()).ok().unwrap()
        })
    }

    /// Serve `key` from `make`, called at each read.
    pub fn serve(&self, key: &str, make: impl Fn() -> StoredStream + Send + Sync + 'static) {
        self.custom
            .lock()
            .unwrap()
            .insert(key.to_string(), Box::new(make));
    }

    pub fn put(&self, key: &str, bytes: Vec<u8>) {
        self.objects.lock().unwrap().insert(key.to_string(), bytes);
    }
}

#[async_trait]
impl OutputStorageRepository for FakeStorage {
    async fn store(&self, _req: StoreRequest) -> Result<StoredOutput, StorageError> {
        panic!("the run path never writes to storage")
    }
    async fn read(&self, _key: &str) -> Result<StoredBytes, StorageError> {
        panic!("the run path never reads an object whole")
    }
    async fn read_stream(&self, key: &str) -> Result<StoredStream, StorageError> {
        self.reads.lock().unwrap().push(key.to_string());
        if let Some(make) = self.custom.lock().unwrap().get(key) {
            return Ok(make());
        }
        if self.broken.lock().unwrap().iter().any(|k| k == key) {
            return Err(StorageError::BackendUnavailable(format!(
                "secret detail about {key}"
            )));
        }
        let bytes = self
            .objects
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or_else(|| StorageError::InvalidInput(format!("no such object {key}")))?;
        let size = bytes.len() as u64;
        let chunks: Vec<Result<Bytes, StorageError>> = bytes
            .chunks(self.chunk.max(1))
            .map(|c| Ok(Bytes::copy_from_slice(c)))
            .collect();
        Ok(StoredStream {
            stream: Box::pin(futures::stream::iter(chunks)),
            size_bytes: size,
            mime_type: "application/octet-stream".into(),
            filename: "object".into(),
        })
    }
    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        // Only a call that did not complete deletes what it stored.
        self.deleted.lock().unwrap().push(key.to_string());
        Ok(())
    }
    async fn store_stream(
        &self,
        mut req: crate::storage::domain::StoreStreamRequest,
    ) -> Result<StoredOutput, StorageError> {
        use futures::StreamExt;
        let call = {
            let mut n = self.store_calls.lock().unwrap();
            *n += 1;
            *n
        };
        let stall = *self.store_stall_on.lock().unwrap();
        if let Some((nth, d)) = stall {
            if nth == call {
                tokio::time::sleep(d).await;
            }
        }
        if *self.store_fail_on.lock().unwrap() == Some(call) {
            return Err(StorageError::BackendUnavailable(
                "secret adapter detail".into(),
            ));
        }
        let mut all = vec![];
        while let Some(chunk) = req.stream.next().await {
            all.extend_from_slice(&chunk?);
        }
        let size_bytes = all.len() as u64;
        // The sink prefixes every name with a per-call id; the tests read the name
        // without it (`stored_filenames` keeps what was really given).
        self.stored_filenames
            .lock()
            .unwrap()
            .push(req.filename.clone());
        let plain = strip_call_prefix(&req.filename).to_string();
        self.stored.lock().unwrap().push((plain.clone(), all));
        Ok(StoredOutput {
            storage_key: format!("generated/{plain}"),
            read_url: String::new(),
            mime_type: req.mime_type,
            filename: req.filename,
            size_bytes,
        })
    }
    fn derived_root(&self, _source: &str) -> Option<String> {
        self.root.clone()
    }
}

/// `<12 hex>-name` becomes `name`.
pub(crate) fn strip_call_prefix(filename: &str) -> &str {
    match filename.split_once('-') {
        Some((id, rest)) if id.len() == 12 && id.bytes().all(|b| b.is_ascii_hexdigit()) => rest,
        _ => filename,
    }
}

pub(crate) fn manifest_with(tables: &[(&str, u32)]) -> Manifest {
    Manifest::new(
        tables
            .iter()
            .map(|(name, parts)| TableInfo {
                name: (*name).to_string(),
                rows: u64::from(*parts) * 10,
                parts: *parts,
                columns: vec![ColumnInfo {
                    name: "a".into(),
                    column_type: ColumnType::Int,
                    uncompressed_bytes: 100,
                    in_memory_bytes: 10_000,
                }],
            })
            .collect(),
    )
}

/// A prepared copy in `storage` and a ready row for it: the manifest, and a
/// part per `parts` of `part_bytes` filler bytes (`b'a' + table index`).
pub(crate) struct Prepared {
    pub registry: Arc<SqlitePreparationRegistry>,
    pub storage: Arc<FakeStorage>,
    pub manifest: Manifest,
    _dir: tempfile::TempDir,
}

pub(crate) async fn sqlite_registry() -> (Arc<SqlitePreparationRegistry>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let options = SqliteConnectOptions::new()
        .filename(dir.path().join("registry.db"))
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(10));
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

pub(crate) async fn claim(registry: &SqlitePreparationRegistry, format_version: i32) {
    let now = Utc::now();
    registry
        .claim(ClaimRequest {
            source_key: SOURCE.into(),
            source_bytes: 60_000_000,
            format_version,
            owner: "job".into(),
            lease: Duration::minutes(5),
            now,
        })
        .await
        .unwrap()
        .expect("claim won");
}

pub(crate) async fn prepared(tables: &[(&str, u32)], part_bytes: usize) -> Prepared {
    prepared_with(tables, part_bytes, FORMAT_VERSION, |_| {}).await
}

/// `tweak` may change the [`ReadyInfo`] before it is recorded.
pub(crate) async fn prepared_with(
    tables: &[(&str, u32)],
    part_bytes: usize,
    format_version: i32,
    tweak: impl FnOnce(&mut ReadyInfo),
) -> Prepared {
    let (registry, dir) = sqlite_registry().await;
    let storage = FakeStorage::new();
    let manifest = manifest_with(tables);
    let mut keys = vec![];
    let mut total = 0usize;
    for (t, (_, parts)) in tables.iter().enumerate() {
        for p in 0..*parts as usize {
            let key = format!("{ROOT}/{}", part_path(t, p).unwrap());
            storage.put(&key, vec![b'a' + t as u8; part_bytes]);
            total += part_bytes;
            keys.push(key);
        }
    }
    let manifest_json = manifest.to_json().unwrap();
    let manifest_key = format!("{ROOT}/{MANIFEST_PATH}");
    total += manifest_json.len();
    storage.put(&manifest_key, manifest_json.into_bytes());
    keys.push(manifest_key.clone());
    claim(&registry, format_version).await;
    let mut info = ReadyInfo {
        manifest_key,
        blob_keys: keys.clone(),
        tables_json: manifest.tables_json().unwrap(),
        prepared_bytes: total as i64,
    };
    tweak(&mut info);
    registry
        .complete(SOURCE, "job", info, Utc::now())
        .await
        .unwrap();
    Prepared {
        registry,
        storage,
        manifest,
        _dir: dir,
    }
}

/// How many bytes of generated chunks are alive at once.
#[derive(Default)]
pub(crate) struct Live {
    pub now: std::sync::atomic::AtomicUsize,
    pub peak: std::sync::atomic::AtomicUsize,
    pub produced: std::sync::atomic::AtomicUsize,
}

struct Chunk {
    data: Vec<u8>,
    live: Arc<Live>,
}

impl AsRef<[u8]> for Chunk {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl Drop for Chunk {
    fn drop(&mut self) {
        self.live
            .now
            .fetch_sub(self.data.len(), std::sync::atomic::Ordering::SeqCst);
    }
}

/// A stream of `total` bytes in chunks of `chunk`, made one at a time as the
/// consumer asks, that DECLARES `declared` bytes (a lie when it differs from
/// `total`). `live` records the most chunk bytes alive at once. When `fail_at`
/// is `Some(n)` the nth chunk is an error.
pub(crate) fn generated(
    total: u64,
    chunk: usize,
    declared: u64,
    live: Arc<Live>,
    fail_at: Option<usize>,
) -> StoredStream {
    use std::sync::atomic::Ordering::SeqCst;
    let stream = futures::stream::unfold((0u64, 0usize), move |(sent, n)| {
        let live = live.clone();
        async move {
            if sent >= total {
                return None;
            }
            if fail_at == Some(n) {
                return Some((
                    Err(StorageError::BackendUnavailable(
                        "secret detail from the adapter".into(),
                    )),
                    (total, n + 1),
                ));
            }
            let len = (total - sent).min(chunk as u64) as usize;
            let now = live.now.fetch_add(len, SeqCst) + len;
            live.peak.fetch_max(now, SeqCst);
            live.produced.fetch_add(len, SeqCst);
            let data = vec![b'x'; len];
            let bytes = Bytes::from_owner(Chunk { data, live });
            Some((Ok(bytes), (sent + len as u64, n + 1)))
        }
    });
    StoredStream {
        stream: Box::pin(stream),
        size_bytes: declared,
        mime_type: "application/octet-stream".into(),
        filename: "part".into(),
    }
}

/// A mounted executor that records what it was given and answers as told.
pub(crate) struct Recorder {
    pub seen: Mutex<Vec<(PythonRunRequest, Vec<usize>, u64)>>,
    pub answer: Mutex<Option<Result<MountedResult, MountedError>>>,
    /// Files the fake "code" leaves in the output volume, fed to the sink.
    pub outputs: Mutex<Vec<(String, Vec<u8>)>>,
    /// How long the fake "code" takes, to exercise progress and the call's budget.
    pub delay: Mutex<Option<std::time::Duration>>,
}

impl Recorder {
    pub fn answering(answer: Result<MountedResult, MountedError>) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(vec![]),
            answer: Mutex::new(Some(answer)),
            outputs: Mutex::new(vec![]),
            delay: Mutex::new(None),
        })
    }
    pub fn ok(output: Value) -> Arc<Self> {
        Self::answering(Ok(MountedResult {
            result: PythonRunResult {
                output: Some(output),
                stdout: "hi\n".into(),
            },
            staged: Staged {
                tables: vec![],
                parts: 0,
                bytes: 0,
            },
            emitted: vec![],
            rejected: vec![],
            too_many_entries: false,
        }))
    }
    /// Like [`Self::ok`], and the code leaves these files in `/out`.
    pub fn ok_with_files(output: Value, files: &[(&str, &[u8])]) -> Arc<Self> {
        let r = Self::ok(output);
        *r.outputs.lock().unwrap() = files
            .iter()
            .map(|(n, b)| (n.to_string(), b.to_vec()))
            .collect();
        r
    }
    pub fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[async_trait]
impl MountedExecutor for Recorder {
    async fn run_with_mounts(
        &self,
        req: PythonRunRequest,
        call: MountedCall<'_>,
    ) -> Result<MountedResult, MountedError> {
        self.seen
            .lock()
            .unwrap()
            .push((req, call.tables.to_vec(), call.out_mb));
        let delay = *self.delay.lock().unwrap();
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        let mut answer = self.answer.lock().unwrap().take().expect("answered once");
        let files = self.outputs.lock().unwrap().clone();
        if let (Some(sink), Ok(done), false) = (call.sink, answer.as_mut(), files.is_empty()) {
            let dir = tempfile::tempdir().unwrap();
            for (name, bytes) in &files {
                std::fs::write(dir.path().join(name), bytes).unwrap();
            }
            let found = super::collect::collect_out(dir.path(), Default::default()).unwrap();
            done.rejected = found.rejected;
            for file in found.files {
                done.emitted.push(file.name.clone());
                sink.accept(file).await.map_err(MountedError::Refused)?;
            }
        }
        answer
    }
}

pub(crate) fn runtime(p: &Prepared, exec: Arc<Recorder>, on: bool) -> LargeTabularRuntime {
    let config = PrepareConfig {
        large_tabular: on,
        ..PrepareConfig::default()
    };
    LargeTabularRuntime::new(
        TabularPrepare::new(config, p.registry.clone()),
        p.registry.clone(),
        p.storage.clone(),
        exec,
    )
    .with_config(RuntimeConfig {
        prepare_wait: std::time::Duration::from_millis(50),
        ..RuntimeConfig::default()
    })
}
