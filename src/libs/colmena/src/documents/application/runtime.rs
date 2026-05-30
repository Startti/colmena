//! `DocumentRuntime` — bundles every use case the documents feature needs and
//! constructs them from a JSON config.
//!
//! Used by:
//!   - The 3 DAG nodes (`document_create`, `document_edit`, `document_read`),
//!     which share a runtime via `OnceCell` per node config.
//!   - The synthetic LLM tools, which receive a `DocumentToolsContext` built
//!     on top of a runtime + session id.
//!
//! Storage backends are pluggable. v1 only knows `localfs` (the default);
//! `gcs` will be added in Block B.

use crate::documents::application::apply_patch::ApplyPatchUseCase;
use crate::documents::application::create_document::CreateDocumentUseCase;
use crate::documents::application::get_head::GetHeadUseCase;
use crate::documents::application::list_versions::ListVersionsUseCase;
use crate::documents::application::read_document::ReadDocumentUseCase;
use crate::documents::application::rollback::RollbackUseCase;
use crate::documents::domain::ports::AssetStore;
use crate::documents::domain::{ArtifactStore, IRRenderer, IRValidator, IdGenerator};
use crate::documents::infrastructure::ids::UlidIdGenerator;
use crate::documents::infrastructure::render::{ExcelRenderer, HtmlRenderer, WordRenderer};
#[cfg(feature = "gcs")]
use crate::documents::infrastructure::storage::GcsArtifactStore;
use crate::documents::infrastructure::storage::{LocalFsAssetStore, LocalFsStore};
use crate::documents::infrastructure::validation::{ExcelValidator, HtmlValidator, WordValidator};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

/// Default retention window (versions kept per artifact) when the caller
/// doesn't specify one. Kept generous because Word/Excel patches are tiny.
pub const DEFAULT_RETENTION: u32 = 20;

/// Default storage path for `localfs` backend when no `storage_root` is given.
pub const DEFAULT_STORAGE_ROOT: &str = ".colmena/documents";

/// Pre-built use cases for the documents feature, plus the underlying store
/// (so callers like the synthetic-tools wiring can attach a session index
/// later without rebuilding everything).
pub struct DocumentRuntime {
    pub store: Arc<dyn ArtifactStore>,
    pub create: Arc<CreateDocumentUseCase>,
    pub apply: Arc<ApplyPatchUseCase>,
    pub read: Arc<ReadDocumentUseCase>,
    pub get_head: Arc<GetHeadUseCase>,
    pub list_versions: Arc<ListVersionsUseCase>,
    pub rollback: Arc<RollbackUseCase>,
}

impl DocumentRuntime {
    /// Build a runtime from a node/tool config. Recognised fields:
    ///   - `storage_backend`: "localfs" (default). "gcs" lands in Block B.
    ///   - `storage_root`: path for localfs (default `./.colmena/documents`).
    ///   - `default_retention`: u32 (default `DEFAULT_RETENTION`).
    ///
    /// Unknown fields are ignored so future config can grow without breaking
    /// existing graphs.
    pub async fn from_config(config: &Value) -> Result<Arc<Self>, String> {
        let backend = config
            .get("storage_backend")
            .and_then(|v| v.as_str())
            .unwrap_or("localfs");

        let (store, asset_store): (Arc<dyn ArtifactStore>, Arc<dyn AssetStore>) = match backend {
            "localfs" => {
                let root = config
                    .get("storage_root")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_STORAGE_ROOT));
                if let Some(parent) = root.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("creating storage parent {:?}: {e}", parent))?;
                }
                std::fs::create_dir_all(&root)
                    .map_err(|e| format!("creating storage root {:?}: {e}", root))?;
                let asset_root = root.with_file_name(format!(
                    "{}_assets",
                    root.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("documents")
                ));
                std::fs::create_dir_all(&asset_root)
                    .map_err(|e| format!("creating asset root {:?}: {e}", asset_root))?;
                (
                    Arc::new(LocalFsStore::new(&root)),
                    Arc::new(LocalFsAssetStore::new(&asset_root)),
                )
            }
            "gcs" => {
                #[cfg(feature = "gcs")]
                {
                    let bucket = config
                        .get("gcs_bucket")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            "storage_backend `gcs` requires `gcs_bucket` in config".to_string()
                        })?;
                    let prefix = config
                        .get("gcs_prefix")
                        .and_then(|v| v.as_str())
                        .unwrap_or("colmena/documents");
                    let asset_prefix = config
                        .get("gcs_asset_prefix")
                        .and_then(|v| v.as_str())
                        .unwrap_or("colmena/assets");
                    use crate::documents::infrastructure::storage::GcsAssetStore;
                    let gcs = GcsArtifactStore::new(bucket, prefix).await?;
                    let gcs_assets = GcsAssetStore::new(bucket, asset_prefix).await?;
                    (Arc::new(gcs), Arc::new(gcs_assets))
                }
                #[cfg(not(feature = "gcs"))]
                {
                    return Err("storage_backend `gcs` requires the `gcs` feature flag — \
                         rebuild with `--features gcs`"
                        .into());
                }
            }
            other => {
                return Err(format!(
                    "unknown storage_backend `{other}` — expected `localfs` or `gcs`"
                ));
            }
        };

        let default_retention = config
            .get("default_retention")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(DEFAULT_RETENTION);

        Ok(Self::with_store(store, asset_store, default_retention))
    }

    /// Build a runtime around an existing store. Useful for tests and for
    /// callers that share a store across multiple feature areas.
    pub fn with_store(
        store: Arc<dyn ArtifactStore>,
        asset_store: Arc<dyn AssetStore>,
        default_retention: u32,
    ) -> Arc<Self> {
        let excel_renderer: Arc<dyn IRRenderer> = Arc::new(ExcelRenderer);
        let word_renderer: Arc<dyn IRRenderer> = Arc::new(WordRenderer);
        let html_renderer: Arc<dyn IRRenderer> = Arc::new(HtmlRenderer::new(asset_store));
        let excel_validator: Arc<dyn IRValidator> = Arc::new(ExcelValidator);
        let word_validator: Arc<dyn IRValidator> = Arc::new(WordValidator);
        let html_validator: Arc<dyn IRValidator> = Arc::new(HtmlValidator);
        let ids: Arc<dyn IdGenerator> = Arc::new(UlidIdGenerator);

        let create = Arc::new(CreateDocumentUseCase {
            store: store.clone(),
            excel_renderer: excel_renderer.clone(),
            excel_validator: excel_validator.clone(),
            word_renderer: word_renderer.clone(),
            word_validator: word_validator.clone(),
            html_renderer: html_renderer.clone(),
            html_validator: html_validator.clone(),
            ids: ids.clone(),
            default_retention,
        });
        let apply = Arc::new(ApplyPatchUseCase {
            store: store.clone(),
            excel_renderer,
            excel_validator,
            word_renderer,
            word_validator,
            ids: ids.clone(),
        });
        let read = Arc::new(ReadDocumentUseCase {
            store: store.clone(),
        });
        let get_head = Arc::new(GetHeadUseCase {
            store: store.clone(),
        });
        let list_versions = Arc::new(ListVersionsUseCase {
            store: store.clone(),
        });
        let rollback = Arc::new(RollbackUseCase {
            store: store.clone(),
        });

        Arc::new(Self {
            store,
            create,
            apply,
            read,
            get_head,
            list_versions,
            rollback,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn from_config_defaults_to_localfs() {
        let tmp = tempdir().unwrap();
        let cfg = json!({"storage_root": tmp.path().to_str().unwrap()});
        let rt = DocumentRuntime::from_config(&cfg).await.unwrap();
        assert!(Arc::strong_count(&rt) >= 1);
    }

    #[cfg(not(feature = "gcs"))]
    #[tokio::test]
    async fn from_config_rejects_gcs_without_feature() {
        let cfg = json!({"storage_backend": "gcs"});
        let err = match DocumentRuntime::from_config(&cfg).await {
            Err(e) => e,
            Ok(_) => panic!("expected error for gcs backend without feature"),
        };
        assert!(err.contains("feature flag"), "got: {err}");
    }

    #[tokio::test]
    async fn from_config_rejects_unknown_backend() {
        let cfg = json!({"storage_backend": "s3"});
        let err = match DocumentRuntime::from_config(&cfg).await {
            Err(e) => e,
            Ok(_) => panic!("expected error for unknown backend"),
        };
        assert!(err.contains("unknown storage_backend"));
    }

    #[tokio::test]
    async fn runtime_creates_and_reads_document() {
        let tmp = tempdir().unwrap();
        let cfg = json!({"storage_root": tmp.path().to_str().unwrap()});
        let rt = DocumentRuntime::from_config(&cfg).await.unwrap();

        use crate::documents::application::create_document::CreateDocumentInput;
        use crate::documents::application::read_document::ReadDocumentInput;
        use crate::documents::domain::ids::{ArtifactKind, SessionId};
        use crate::documents::domain::patch::PatchSource;

        let created = rt
            .create
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Excel,
                session_id: SessionId::new("sess_1"),
                label: None,
                retention_limit: None,
                initial_ir: None,
                source: PatchSource::Agent,
            })
            .await
            .unwrap();

        let read = rt
            .read
            .execute(ReadDocumentInput {
                artifact_id: created.artifact_id.clone(),
                version: None,
            })
            .await
            .unwrap();
        assert_eq!(read.version.0, "v1");
    }
}
