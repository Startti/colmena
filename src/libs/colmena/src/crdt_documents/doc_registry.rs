//! Registry of live `yrs::Doc`s keyed by `ArtifactId`.
//!
//! Each entry owns its `Doc`, a `SnapshotHandle` (per-artifact snapshot task),
//! and the artifact metadata. The registry reloads all known artifacts from
//! disk at startup via `load_from_disk`.

use crate::crdt_documents::{
    snapshot_writer::{spawn_writer, SnapshotHandle},
    storage::{ArtifactMeta, ArtifactStorage},
    ArtifactId, StorageError,
};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Notify;
use yrs::updates::decoder::Decode;
use yrs::{Doc, Transact};

pub struct RegisteredArtifact {
    pub doc: Arc<Doc>,
    pub dirty: Arc<std::sync::atomic::AtomicBool>,
    pub notify: Arc<Notify>,
    snapshot: tokio::sync::Mutex<SnapshotHandle>,
    pub meta: ArtifactMeta,
}

impl RegisteredArtifact {
    /// Mark the artifact as dirty, triggering the snapshot writer.
    pub fn mark_dirty(&self) {
        self.dirty.store(true, std::sync::atomic::Ordering::Release);
        self.notify.notify_one();
    }
}

pub struct DocRegistry {
    docs: DashMap<String, Arc<RegisteredArtifact>>,
    storage: Arc<dyn ArtifactStorage>,
}

impl DocRegistry {
    pub fn new(storage: Arc<dyn ArtifactStorage>) -> Self {
        Self {
            docs: DashMap::new(),
            storage,
        }
    }

    /// Reload every known artifact from storage. Call at startup.
    pub async fn load_from_disk(&self) -> Result<usize, StorageError> {
        let metas = self.storage.list().await?;
        let mut loaded = 0;
        for meta in metas {
            let id = meta.artifact_id.clone();
            let doc = Arc::new(Doc::new());
            if let Some(bytes) = self.storage.load_state(&id).await? {
                if let Ok(update) = yrs::Update::decode_v1(&bytes) {
                    let _ = doc.transact_mut().apply_update(update);
                }
            }
            let snapshot = spawn_writer(id.clone(), doc.clone(), self.storage.clone());
            let notify = snapshot.notify.clone();
            let dirty = snapshot.dirty.clone();
            self.docs.insert(
                id.to_string(),
                Arc::new(RegisteredArtifact {
                    doc,
                    dirty,
                    notify,
                    snapshot: tokio::sync::Mutex::new(snapshot),
                    meta,
                }),
            );
            loaded += 1;
        }
        Ok(loaded)
    }

    pub fn get_or_create(&self, id: &ArtifactId, name_if_new: &str) -> Arc<RegisteredArtifact> {
        self.docs
            .entry(id.to_string())
            .or_insert_with(|| {
                let doc = Arc::new(Doc::new());
                let snapshot = spawn_writer(id.clone(), doc.clone(), self.storage.clone());
                let notify = snapshot.notify.clone();
                let dirty = snapshot.dirty.clone();
                let now = chrono::Utc::now().timestamp();
                let meta = ArtifactMeta {
                    artifact_id: id.clone(),
                    name: name_if_new.to_string(),
                    created_at: now,
                    updated_at: now,
                    sheet_count: 0,
                };
                // Save the initial meta best-effort in a background task.
                {
                    let storage = self.storage.clone();
                    let meta_clone = meta.clone();
                    tokio::spawn(async move {
                        let _ = storage.save_meta(&meta_clone).await;
                    });
                }
                Arc::new(RegisteredArtifact {
                    doc,
                    dirty,
                    notify,
                    snapshot: tokio::sync::Mutex::new(snapshot),
                    meta,
                })
            })
            .clone()
    }

    pub fn get(&self, id: &ArtifactId) -> Option<Arc<RegisteredArtifact>> {
        self.docs.get(&id.to_string()).map(|r| r.value().clone())
    }

    pub fn list(&self) -> Vec<ArtifactMeta> {
        self.docs.iter().map(|r| r.value().meta.clone()).collect()
    }

    /// Drain every artifact's snapshot writer: signal shutdown AND await
    /// its final flush. Call this before dropping the registry (or the
    /// tokio runtime) to guarantee pending mutations land on disk.
    ///
    /// Without this, the writer tasks are detached `tokio::spawn`s and die
    /// when the tokio runtime is torn down — losing any work between the
    /// last 5-second tick and process exit. This is exactly what happens
    /// when a CLI `dag_engine run` finishes a graph that mutated a CRDT
    /// artifact: the last mutations never reach the snapshot file.
    ///
    /// Idempotent: re-calling shutdown on a writer that already drained is
    /// a no-op (see `SnapshotHandle::shutdown`).
    pub async fn shutdown_all(&self) {
        let entries: Vec<Arc<RegisteredArtifact>> =
            self.docs.iter().map(|r| r.value().clone()).collect();
        for entry in entries {
            let mut guard = entry.snapshot.lock().await;
            guard.shutdown().await;
        }
    }

    pub async fn delete(&self, id: &ArtifactId) -> Result<(), StorageError> {
        if let Some((_, entry)) = self.docs.remove(&id.to_string()) {
            let mut guard = entry.snapshot.lock().await;
            guard.shutdown().await;
        }
        self.storage.delete(id).await
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::LocalFsStorage;
    use std::path::PathBuf;

    fn temp_storage() -> (Arc<dyn ArtifactStorage>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("reg_test_{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let storage: Arc<dyn ArtifactStorage> = Arc::new(LocalFsStorage::new(dir.clone()).unwrap());
        (storage, dir)
    }

    #[tokio::test]
    async fn get_or_create_returns_same_arc_on_repeat() {
        let (s, dir) = temp_storage();
        let reg = DocRegistry::new(s);
        let id = ArtifactId::new();
        let a = reg.get_or_create(&id, "test");
        let b = reg.get_or_create(&id, "test");
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(reg.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn different_ids_get_different_entries() {
        let (s, dir) = temp_storage();
        let reg = DocRegistry::new(s);
        let id1 = ArtifactId::new();
        let id2 = ArtifactId::new();
        let a = reg.get_or_create(&id1, "a");
        let b = reg.get_or_create(&id2, "b");
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(reg.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
