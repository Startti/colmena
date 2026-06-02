//! Bundler: builds and owns every long-lived service for the v1 feature.
//!
//! One runtime per process; shared between the HTTP server, the
//! `llm_call.config.crdt_documents` synthetic tools, and the Python
//! bindings. Built from JSON config — mirrors `DocumentRuntime::from_config`
//! in the existing documents module.

use crate::crdt_documents::{
    storage::StorageConfig, ArtifactStorage, DocRegistry, StorageError,
};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub const DEFAULT_STORAGE_ROOT: &str = ".colmena/crdt_documents";

pub struct CrdtDocumentsRuntime {
    pub registry: Arc<DocRegistry>,
    pub storage: Arc<dyn ArtifactStorage>,
    pub tracker: Arc<crate::crdt_documents::ChangeTracker>,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("config: {0}")]
    Config(String),
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
}

impl CrdtDocumentsRuntime {
    /// Build a runtime from a node/tool config. Recognised fields:
    ///   - `storage_backend`: "localfs" (default). "gcs" gated by feature flag.
    ///   - `storage_root`: path for localfs (default `./.colmena/crdt_documents`).
    ///   - `gcs_bucket`: bucket name for GCS backend (required if backend is "gcs").
    ///   - `gcs_prefix`: object prefix for GCS (default "colmena/crdt_documents").
    ///
    /// Unknown fields are ignored so future config can grow without breaking
    /// existing graphs.
    pub async fn from_config(cfg: &Value) -> Result<Self, RuntimeError> {
        let backend = cfg
            .get("storage_backend")
            .and_then(Value::as_str)
            .unwrap_or("localfs");

        let storage_cfg = match backend {
            "localfs" => StorageConfig::LocalFs {
                root: cfg
                    .get("storage_root")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_STORAGE_ROOT)),
            },
            #[cfg(feature = "gcs")]
            "gcs" => StorageConfig::Gcs {
                bucket: cfg
                    .get("gcs_bucket")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        RuntimeError::Config(
                            "storage_backend `gcs` requires `gcs_bucket` in config".into(),
                        )
                    })?
                    .to_string(),
                prefix: cfg
                    .get("gcs_prefix")
                    .and_then(Value::as_str)
                    .unwrap_or("colmena/crdt_documents")
                    .to_string(),
            },
            #[cfg(not(feature = "gcs"))]
            "gcs" => {
                return Err(RuntimeError::Config(
                    "storage_backend `gcs` requires the `gcs` feature flag — \
                     rebuild with `--features gcs`"
                        .into(),
                ));
            }
            other => {
                return Err(RuntimeError::Config(format!(
                    "unknown storage_backend `{other}` — expected `localfs` or `gcs`"
                )))
            }
        };

        let storage: Arc<dyn ArtifactStorage> = storage_cfg.build()?;
        let registry = Arc::new(DocRegistry::new(storage.clone()));
        let _ = registry.load_from_disk().await?;
        let tracker = Arc::new(crate::crdt_documents::ChangeTracker::new());

        Ok(Self {
            registry,
            storage,
            tracker,
        })
    }

    /// Drain every artifact's snapshot writer to guarantee pending
    /// mutations land on disk. MUST be called before dropping the runtime
    /// (or letting its tokio context tear down) if you wrote anything via
    /// the LLM tool dispatchers or via direct Y.Doc mutations. The
    /// snapshot writer is a detached `tokio::spawn` and otherwise dies
    /// with the runtime, dropping its final flush.
    ///
    /// Idempotent.
    pub async fn shutdown(&self) {
        self.registry.shutdown_all().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn default_localfs_runtime_builds() {
        let tmp = std::env::temp_dir().join(format!("rt_test_{}", ulid::Ulid::new()));
        let cfg = json!({ "storage_root": tmp.to_str().unwrap() });
        let rt = CrdtDocumentsRuntime::from_config(&cfg).await.unwrap();
        assert_eq!(rt.registry.len(), 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn rejects_unknown_backend() {
        let cfg = json!({ "storage_backend": "s3" });
        let err = CrdtDocumentsRuntime::from_config(&cfg).await.err().unwrap();
        assert!(matches!(err, RuntimeError::Config(_)));
        assert!(err.to_string().contains("unknown storage_backend"));
    }

    #[cfg(not(feature = "gcs"))]
    #[tokio::test]
    async fn from_config_rejects_gcs_without_feature() {
        let cfg = json!({ "storage_backend": "gcs" });
        let err = CrdtDocumentsRuntime::from_config(&cfg).await.err().unwrap();
        assert!(err.to_string().contains("feature flag"));
    }

    /// Reproduces the smoke-graph persistence bug: tool-driven mutations
    /// must be flushed to disk before the runtime is dropped, otherwise a
    /// fresh runtime loading from the same storage_root cannot see them.
    ///
    /// Phase 1: spin up a dedicated tokio runtime, build a CRDT runtime,
    ///   write a marker key on a workbook, `mark_dirty`, call shutdown,
    ///   then tear down the tokio runtime.
    /// Phase 2: in this test's tokio runtime, build a fresh CRDT runtime
    ///   from the same storage_root and assert the marker is present.
    ///
    /// If `shutdown_all` doesn't drain the writers, phase 2 sees an empty
    /// workbook because the writer task died on tokio teardown.
    #[tokio::test]
    async fn shutdown_persists_pending_mutations_across_runtime_teardown() {
        use crate::crdt_documents::ArtifactId;

        let root = std::env::temp_dir().join(format!("rt_shutdown_{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&root).unwrap();
        let cfg = json!({ "storage_root": root.to_str().unwrap() });
        let artifact_id = ArtifactId::new();
        let artifact_id_for_phase2 = artifact_id.clone();
        let cfg_clone = cfg.clone();

        // Phase 1: separate tokio runtime that dies after this scope.
        std::thread::spawn(move || {
            let trt = tokio::runtime::Runtime::new().unwrap();
            trt.block_on(async {
                let crdt_rt = CrdtDocumentsRuntime::from_config(&cfg_clone).await.unwrap();
                let entry = crdt_rt.registry.get_or_create(&artifact_id, "test");
                {
                    use yrs::{Map, Transact, WriteTxn};
                    let mut txn = entry.doc.transact_mut();
                    let m = txn.get_or_insert_map("workbook");
                    m.insert(&mut txn, "marker", "hello");
                }
                entry.mark_dirty();
                // The fix under test:
                crdt_rt.shutdown().await;
            });
            // Drop the tokio runtime here. With shutdown above, marker is
            // already on disk. Without it, the writer task dies pending.
        })
        .join()
        .unwrap();

        // Phase 2: fresh runtime in THIS test's tokio runtime, expects
        // the marker to be present after disk round-trip.
        let crdt_rt = CrdtDocumentsRuntime::from_config(&cfg).await.unwrap();
        let entry = crdt_rt
            .registry
            .get(&artifact_id_for_phase2)
            .expect("artifact should reload from storage");

        use yrs::{Map, ReadTxn, Transact};
        let txn = entry.doc.transact();
        let m = txn.get_map("workbook").expect("workbook map");
        match m.get(&txn, "marker") {
            Some(yrs::Out::Any(yrs::Any::String(s))) => assert_eq!(s.as_ref(), "hello"),
            other => panic!("marker missing after shutdown→reload: {other:?}"),
        }

        // Clean up.
        crdt_rt.shutdown().await;
        let _ = std::fs::remove_dir_all(&root);
    }
}
