//! Per-artifact snapshot writer.
//!
//! Spawns a tokio task that listens for two triggers:
//!   - explicit `notify.notify_one()` after any mutation
//!   - 5-second tick
//!
//! When triggered AND the dirty flag is set, encodes the `Doc` state via
//! `encode_state_as_update_v1(&StateVector::default())` and asks the storage
//! backend to persist it.

use crate::crdt_documents::{ArtifactId, ArtifactStorage};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, Notify};
use yrs::{Doc, ReadTxn, Transact};

const TICK: Duration = Duration::from_secs(5);

pub struct SnapshotHandle {
    pub notify: Arc<Notify>,
    pub dirty: Arc<AtomicBool>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    done_rx: Option<oneshot::Receiver<()>>,
}

impl SnapshotHandle {
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
        self.notify.notify_one();
    }

    /// Signal shutdown and AWAIT the writer task's final flush. Idempotent:
    /// calling twice is a no-op the second time.
    pub async fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
            if let Some(done) = self.done_rx.take() {
                let _ = done.await;
            }
        }
    }
}

/// Spawn the writer task. The returned handle can be used to mark mutations
/// and to request graceful shutdown.
pub fn spawn_writer(
    id: ArtifactId,
    doc: Arc<Doc>,
    storage: Arc<dyn ArtifactStorage>,
) -> SnapshotHandle {
    let notify = Arc::new(Notify::new());
    let dirty = Arc::new(AtomicBool::new(false));
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let (done_tx, done_rx) = oneshot::channel::<()>();

    let task_notify = notify.clone();
    let task_dirty = dirty.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(TICK) => {}
                _ = task_notify.notified() => {}
                _ = &mut shutdown_rx => {
                    flush(&id, &doc, storage.as_ref()).await;
                    break;
                }
            }
            if task_dirty.swap(false, Ordering::AcqRel) {
                flush(&id, &doc, storage.as_ref()).await;
            }
        }
        let _ = done_tx.send(());
    });

    SnapshotHandle {
        notify,
        dirty,
        shutdown_tx: Some(shutdown_tx),
        done_rx: Some(done_rx),
    }
}

async fn flush(id: &ArtifactId, doc: &Doc, storage: &dyn ArtifactStorage) {
    let bytes = doc
        .transact()
        .encode_state_as_update_v1(&yrs::StateVector::default());
    if let Err(e) = storage.save_state(id, &bytes).await {
        tracing::warn!("snapshot save failed for {id}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::LocalFsStorage;
    use yrs::updates::decoder::Decode;
    use yrs::{Map, Transact, WriteTxn};

    #[tokio::test]
    async fn dirty_then_shutdown_persists_state() {
        let root = std::env::temp_dir().join(format!("snap_test_{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&root).unwrap();
        let storage: Arc<dyn ArtifactStorage> =
            Arc::new(LocalFsStorage::new(root.clone()).unwrap());
        let id = ArtifactId::new();
        let doc = Arc::new(Doc::new());
        let mut handle = spawn_writer(id.clone(), doc.clone(), storage.clone());

        // Mutate.
        {
            let mut txn = doc.transact_mut();
            let m = txn.get_or_insert_map("workbook");
            m.insert(&mut txn, "marker", "hello");
        }
        handle.mark_dirty();
        handle.shutdown().await;

        let loaded = storage.load_state(&id).await.unwrap().expect("state");
        let fresh = Doc::new();
        {
            let update = yrs::Update::decode_v1(&loaded).unwrap();
            fresh.transact_mut().apply_update(update).unwrap();
        }
        let txn = fresh.transact();
        let m = txn.get_map("workbook").unwrap();
        match m.get(&txn, "marker") {
            Some(yrs::Out::Any(yrs::Any::String(s))) => assert_eq!(s.as_ref(), "hello"),
            other => panic!("missing marker: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
