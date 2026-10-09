//! Storing the outputs a call kept: each checked file is streamed from its open
//! descriptor to the host's storage, one chunk at a time, so no output is ever
//! held whole in memory. The size the reader checked is the size that must
//! arrive: a file that turns out shorter (truncated after the check) is refused
//! and stored nowhere.

use super::collect::OutFile;
use super::mounted::OutputSink;
use super::refusal::RunRefusal;
use crate::storage::domain::{
    OutputStorageRepository, StorageError, StorePlacement, StoreStreamRequest,
};
use async_trait::async_trait;
use bytes::Bytes;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;

const CHUNK: usize = 1024 * 1024;

/// An output that reached storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emitted {
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    /// The storage handle, the way every generated attachment is named.
    pub storage_key: String,
}

/// Objects stored for a call that is not finished until [`OutputGuard::commit`]:
/// dropped without it (the call failed, was cut off by the call's clock, or its
/// future was dropped), it deletes them, so "nothing was kept" is true. The
/// deletes run on the runtime and their failures are logged, not hidden.
#[derive(Debug)]
pub struct OutputGuard {
    storage: Option<Arc<dyn OutputStorageRepository>>,
    keys: Vec<String>,
}

impl OutputGuard {
    fn new(storage: Arc<dyn OutputStorageRepository>, keys: Vec<String>) -> Self {
        Self {
            storage: Some(storage),
            keys,
        }
    }

    /// The call is done and the files are registered: keep them.
    pub fn commit(mut self) {
        self.keys.clear();
        self.storage = None;
    }

    /// The keys held.
    pub fn keys(&self) -> &[String] {
        &self.keys
    }

    /// Hands the cleanup to a ledger that also owns the registry rows: this guard no
    /// longer deletes anything by itself.
    pub fn into_ledger(mut self, rows: Option<RowRegistry>) -> OutputLedger {
        let storage = self
            .storage
            .take()
            .expect("an uncommitted guard holds its storage");
        let entries = std::mem::take(&mut self.keys)
            .into_iter()
            .map(|k| (k, false))
            .collect();
        OutputLedger {
            storage,
            rows,
            entries,
            done: false,
        }
    }
}

/// How long one step of a cleanup (a row removal, an object delete) may take, and how
/// many times a delete is tried.
const CLEANUP_STEP: std::time::Duration = std::time::Duration::from_secs(30);

/// Runs `work` on the runtime if there is one, else on a thread of its own with a
/// small runtime: a cleanup that can only run from `Drop` must not be skipped because
/// no runtime is current.
fn run_detached<F: std::future::Future<Output = ()> + Send + 'static>(work: F) {
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn(work);
        }
        Err(_) => {
            let _ = std::thread::Builder::new()
                .name("colmena-output-cleanup".into())
                .spawn(move || {
                    if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        rt.block_on(work);
                    }
                });
        }
    }
}

/// Deletes `key`, bounded in time and tried twice; a failure is logged (never the key).
async fn delete_object(storage: &dyn OutputStorageRepository, key: &str) -> bool {
    for _ in 0..2 {
        if let Ok(Ok(())) = tokio::time::timeout(CLEANUP_STEP, storage.delete(key)).await {
            return true;
        }
    }
    tracing::warn!(target: "colmena::tabular_run", "a stored output could not be deleted");
    false
}

fn delete_in_background(storage: Arc<dyn OutputStorageRepository>, keys: Vec<String>) {
    if keys.is_empty() {
        return;
    }
    run_detached(async move {
        for key in keys {
            delete_object(&*storage, &key).await;
        }
    });
}

/// The registry a ledger removes rows from.
pub type RowRegistry = (Arc<dyn crate::llm::domain::AttachmentRegistry>, String);

/// One owner of everything a call created for its returned files: the objects it
/// stored and the registry rows it made. Dropped (or rolled back) without
/// [`OutputLedger::commit`], it removes the ROWS first and the OBJECTS second, and
/// never deletes an object whose row it could not remove (that pair is left whole
/// and reported as kept). The cleanup runs from `Drop` too, spawned and bounded.
pub struct OutputLedger {
    storage: Arc<dyn OutputStorageRepository>,
    rows: Option<RowRegistry>,
    entries: Vec<(String, bool)>,
    done: bool,
}

/// Undoes `entries` (key, row made?): rows first, then objects. Returns the entries
/// that could not be undone because their row would not go: both are left in place.
async fn undo(
    storage: &dyn OutputStorageRepository,
    rows: Option<&RowRegistry>,
    entries: Vec<(String, bool)>,
) -> Vec<(String, bool)> {
    let mut stuck = vec![];
    let mut objects = vec![];
    for (key, registered) in entries.into_iter().rev() {
        if registered {
            let removed = match rows {
                Some((registry, session)) => matches!(
                    tokio::time::timeout(
                        CLEANUP_STEP,
                        registry.delete_attachment_for_provider(
                            session,
                            &key,
                            crate::llm::domain::ProviderKind::Generated,
                        )
                    )
                    .await,
                    Ok(Ok(()))
                ),
                None => false,
            };
            if !removed {
                tracing::warn!(target: "colmena::tabular_run", "a registered output row could not be removed; it and its object are left in place");
                stuck.push((key, true));
                continue;
            }
        }
        objects.push(key);
    }
    for key in objects {
        delete_object(storage, &key).await;
    }
    stuck
}

impl OutputLedger {
    /// A row for `key` was made.
    pub fn note_registered(&mut self, key: &str) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.0 == key) {
            e.1 = true;
        }
    }

    /// Everything is registered: keep it all.
    pub fn commit(mut self) {
        self.done = true;
        self.entries.clear();
    }

    /// Undoes it now. Returns the keys whose row could not be removed: those stay,
    /// whole (row and object), and are kept in fact.
    pub async fn rollback(mut self) -> Vec<String> {
        self.done = true;
        let entries = std::mem::take(&mut self.entries);
        undo(&*self.storage, self.rows.as_ref(), entries)
            .await
            .into_iter()
            .map(|e| e.0)
            .collect()
    }
}

impl Drop for OutputLedger {
    fn drop(&mut self) {
        if self.done || self.entries.is_empty() {
            return;
        }
        let (storage, rows) = (self.storage.clone(), self.rows.take());
        let entries = std::mem::take(&mut self.entries);
        run_detached(async move {
            undo(&*storage, rows.as_ref(), entries).await;
        });
    }
}

impl Drop for OutputGuard {
    fn drop(&mut self) {
        if let Some(storage) = self.storage.take() {
            delete_in_background(storage, std::mem::take(&mut self.keys));
        }
    }
}

impl std::fmt::Debug for dyn OutputStorageRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OutputStorageRepository")
    }
}

/// A sink that stores each output with `store_stream` and remembers where.
/// Dropped with outputs it has not handed out through [`StoreSink::take_guarded`],
/// it deletes them.
pub struct StoreSink {
    storage: Arc<dyn OutputStorageRepository>,
    session_id: Option<String>,
    agent_session_id: Option<String>,
    stored: Mutex<Vec<Emitted>>,
    /// One store may take this long, and all of a call's together `total`
    /// (counted from the first).
    per_store: std::time::Duration,
    total: std::time::Duration,
    begun: Mutex<Option<tokio::time::Instant>>,
    /// Unique to this call: prefixes every stored file name.
    call_id: String,
}

/// Logs, when dropped armed, that a store did not finish: an object named `filename`
/// may remain and its key is not known, so no cleanup can name it. (A host's orphan
/// sweep of generated objects is what reaches it; the unique name finds it.)
struct OrphanWatch {
    filename: String,
    armed: bool,
}

impl Drop for OrphanWatch {
    fn drop(&mut self) {
        if self.armed {
            tracing::warn!(target: "colmena::tabular_run", orphan_filename = %self.filename, "an output store did not finish; an object with this name may remain");
        }
    }
}

impl StoreSink {
    pub fn new(
        storage: Arc<dyn OutputStorageRepository>,
        session_id: Option<String>,
        agent_session_id: Option<String>,
    ) -> Self {
        Self {
            storage,
            session_id,
            agent_session_id,
            stored: Mutex::new(vec![]),
            per_store: std::time::Duration::from_secs(60),
            total: super::runtime::COLLECT_BUDGET,
            begun: Mutex::new(None),
            call_id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
        }
    }

    /// Different limits (the tests).
    pub fn with_limits(
        mut self,
        per_store: std::time::Duration,
        total: std::time::Duration,
    ) -> Self {
        (self.per_store, self.total) = (per_store, total);
        self
    }

    /// What was stored, in the order it was accepted, and the guard that deletes
    /// it unless the caller commits.
    pub fn take_guarded(&self) -> (Vec<Emitted>, OutputGuard) {
        let stored = std::mem::take(&mut *self.stored.lock().unwrap_or_else(|e| e.into_inner()));
        let keys = stored.iter().map(|e| e.storage_key.clone()).collect();
        (stored, OutputGuard::new(self.storage.clone(), keys))
    }

    /// What was stored (for the tests that do not care about the guard).
    #[cfg(test)]
    pub fn take(&self) -> Vec<Emitted> {
        let (stored, guard) = self.take_guarded();
        guard.commit();
        stored
    }
}

/// `size` bytes from `file`, a chunk at a time; an early end is an error.
fn chunks(
    file: std::fs::File,
    size: u64,
) -> impl futures::Stream<Item = Result<Bytes, StorageError>> + Send {
    futures::stream::unfold(
        (tokio::fs::File::from_std(file), size),
        |(mut file, left)| async move {
            if left == 0 {
                return None;
            }
            let mut buf = vec![0u8; (left.min(CHUNK as u64)) as usize];
            let got = match file.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    let e = StorageError::InvalidInput("the output ended early".into());
                    return Some((Err(e), (file, 0)));
                }
                Ok(n) => n,
            };
            buf.truncate(got);
            Some((Ok(Bytes::from(buf)), (file, left - got as u64)))
        },
    )
}

impl Drop for StoreSink {
    fn drop(&mut self) {
        let left = std::mem::take(&mut *self.stored.lock().unwrap_or_else(|e| e.into_inner()));
        delete_in_background(
            self.storage.clone(),
            left.into_iter().map(|e| e.storage_key).collect(),
        );
    }
}

#[async_trait]
impl OutputSink for StoreSink {
    async fn accept(&self, file: OutFile) -> Result<(), RunRefusal> {
        let (name, size, mime) = (file.name.clone(), file.size, file.format.mime());
        // A storage that stalls or trickles holds nothing past its limit: one store
        // has a time, and all of a call's together have one.
        let begun = *self
            .begun
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(tokio::time::Instant::now);
        let left = (begun + self.total).saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Err(RunRefusal::Storage);
        }
        // A name no other call shares, so the key a storage derives from it cannot repeat
        // between calls (a later call's cleanup can never name an earlier call's object),
        // and an object left by a store that was cut can be found by it.
        let unique = format!("{}-{name}", self.call_id);
        let mut watch = OrphanWatch {
            filename: unique.clone(),
            armed: true,
        };
        let stored = tokio::time::timeout(
            self.per_store.min(left),
            self.storage.store_stream(StoreStreamRequest {
                stream: Box::pin(chunks(file.into_file(), size)),
                size_hint: Some(size),
                mime_type: mime.to_string(),
                filename: unique.clone(),
                session_id: self.session_id.clone(),
                agent_session_id: self.agent_session_id.clone(),
                placement: StorePlacement::Generated,
            }),
        )
        .await
        .map_err(|_| RunRefusal::Storage)?
        .map_err(|_| RunRefusal::Storage)?;
        watch.armed = false;
        self.stored
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Emitted {
                name,
                mime_type: mime.to_string(),
                size_bytes: size,
                storage_key: stored.storage_key,
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::collect::{collect_out, CollectLimits};
    use super::*;
    use crate::storage::domain::{StoreRequest, StoredBytes, StoredOutput, StoredStream};
    use futures::StreamExt;

    /// Storage that keeps what `store_stream` is given and the biggest chunk.
    #[derive(Default)]
    struct Capturing {
        got: Mutex<Vec<(String, Vec<u8>)>>,
        names: Mutex<Vec<String>>,
        biggest: Mutex<usize>,
    }

    #[async_trait]
    impl OutputStorageRepository for Capturing {
        async fn store(&self, _: StoreRequest) -> Result<StoredOutput, StorageError> {
            unreachable!("outputs are streamed")
        }
        async fn read(&self, _: &str) -> Result<StoredBytes, StorageError> {
            unreachable!()
        }
        async fn read_stream(&self, _: &str) -> Result<StoredStream, StorageError> {
            unreachable!()
        }
        async fn delete(&self, _: &str) -> Result<(), StorageError> {
            unreachable!()
        }
        async fn store_stream(
            &self,
            mut req: StoreStreamRequest,
        ) -> Result<StoredOutput, StorageError> {
            assert_eq!(req.placement, StorePlacement::Generated);
            let mut all = vec![];
            while let Some(chunk) = req.stream.next().await {
                let chunk = chunk?;
                let mut big = self.biggest.lock().unwrap();
                *big = (*big).max(chunk.len());
                all.extend_from_slice(&chunk);
            }
            self.names.lock().unwrap().push(req.filename.clone());
            let plain = super::super::testkit::strip_call_prefix(&req.filename).to_string();
            let key = format!("generated/{plain}");
            let size = all.len() as u64;
            self.got.lock().unwrap().push((plain, all));
            Ok(StoredOutput {
                storage_key: key,
                mime_type: req.mime_type,
                filename: req.filename,
                size_bytes: size,
                read_url: String::new(),
            })
        }
    }

    fn one(dir: &std::path::Path, name: &str, bytes: &[u8]) -> OutFile {
        std::fs::write(dir.join(name), bytes).unwrap();
        collect_out(dir, CollectLimits::default())
            .unwrap()
            .files
            .into_iter()
            .find(|f| f.name == name)
            .unwrap()
    }

    #[tokio::test]
    async fn an_output_is_streamed_to_storage_with_its_name_type_and_size() {
        let d = tempfile::tempdir().unwrap();
        let storage = Arc::new(Capturing::default());
        let sink = StoreSink::new(storage.clone(), Some("s".into()), Some("a".into()));
        sink.accept(one(d.path(), "out.csv", b"a,b\n1,2\n"))
            .await
            .unwrap();
        assert_eq!(
            sink.take(),
            [Emitted {
                name: "out.csv".into(),
                mime_type: "text/csv".into(),
                size_bytes: 8,
                storage_key: "generated/out.csv".into()
            }]
        );
        assert_eq!(storage.got.lock().unwrap()[0].1, b"a,b\n1,2\n");
        assert!(sink.take().is_empty(), "taken once");
    }

    /// Memory is one chunk: 5 MiB reach storage in chunks of at most 1 MiB.
    #[tokio::test]
    async fn a_large_output_is_never_held_whole() {
        let d = tempfile::tempdir().unwrap();
        let storage = Arc::new(Capturing::default());
        let sink = StoreSink::new(storage.clone(), None, None);
        sink.accept(one(d.path(), "big.parquet", &vec![7u8; 5 * CHUNK + 3]))
            .await
            .unwrap();
        assert_eq!(*storage.biggest.lock().unwrap(), CHUNK);
        assert_eq!(storage.got.lock().unwrap()[0].1.len(), 5 * CHUNK + 3);
    }

    /// A file truncated after the check is refused: the size that was checked
    /// is the size that must arrive.
    #[tokio::test]
    async fn an_output_that_ends_early_is_refused_and_stored_nowhere() {
        let d = tempfile::tempdir().unwrap();
        let file = one(d.path(), "cut.csv", b"0123456789");
        std::fs::OpenOptions::new()
            .write(true)
            .open(d.path().join("cut.csv"))
            .unwrap()
            .set_len(4)
            .unwrap();
        let storage = Arc::new(Capturing::default());
        let sink = StoreSink::new(storage.clone(), None, None);
        let err = sink.accept(file).await.unwrap_err();
        assert_eq!(err, RunRefusal::Storage);
        assert!(sink.take().is_empty());
        assert!(storage.got.lock().unwrap().is_empty());
    }

    /// A storage that stalls on a store, or takes longer in all than the call's
    /// collection may, is cut off; what was stored before is kept for the guard.
    #[tokio::test]
    async fn a_store_that_stalls_or_overruns_the_total_is_cut_off() {
        use std::time::Duration;
        struct Stalls(Capturing);
        #[async_trait]
        impl OutputStorageRepository for Stalls {
            async fn store(&self, r: StoreRequest) -> Result<StoredOutput, StorageError> {
                self.0.store(r).await
            }
            async fn read(&self, k: &str) -> Result<StoredBytes, StorageError> {
                self.0.read(k).await
            }
            async fn read_stream(&self, k: &str) -> Result<StoredStream, StorageError> {
                self.0.read_stream(k).await
            }
            async fn delete(&self, k: &str) -> Result<(), StorageError> {
                self.0.delete(k).await
            }
            async fn store_stream(
                &self,
                req: StoreStreamRequest,
            ) -> Result<StoredOutput, StorageError> {
                if req.filename.contains("slow") {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
                self.0.store_stream(req).await
            }
        }
        let d = tempfile::tempdir().unwrap();
        let sink = StoreSink::new(Arc::new(Stalls(Capturing::default())), None, None)
            .with_limits(Duration::from_millis(200), Duration::from_secs(5));
        let started = std::time::Instant::now();
        let err = sink
            .accept(one(d.path(), "slow.csv", b"x"))
            .await
            .unwrap_err();
        assert_eq!(err, RunRefusal::Storage);
        assert!(started.elapsed() < Duration::from_secs(3));
        // The total: a first store eats the budget, the second has nothing left.
        let sink = StoreSink::new(Arc::new(Stalls(Capturing::default())), None, None)
            .with_limits(Duration::from_secs(5), Duration::from_millis(1));
        tokio::time::sleep(Duration::from_millis(5)).await;
        let _ = sink.accept(one(d.path(), "a.csv", b"x")).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let err = sink.accept(one(d.path(), "b.csv", b"y")).await.unwrap_err();
        assert_eq!(err, RunRefusal::Storage);
    }

    /// Two calls that return the same file name store under different names, so a
    /// key a storage derives from the name cannot repeat between calls.
    #[tokio::test]
    async fn the_same_file_name_in_two_calls_is_stored_under_two_names() {
        let d = tempfile::tempdir().unwrap();
        let storage = Arc::new(Capturing::default());
        for _ in 0..2 {
            let sink = StoreSink::new(storage.clone(), None, None);
            sink.accept(one(d.path(), "out.csv", b"x")).await.unwrap();
        }
        let names = storage.names.lock().unwrap().clone();
        assert_eq!(names.len(), 2);
        assert_ne!(names[0], names[1]);
        assert!(names
            .iter()
            .all(|n| n.ends_with("-out.csv") && n.len() == "out.csv".len() + 13));
    }
}
