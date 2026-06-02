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
}
