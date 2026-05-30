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
use crate::documents::application::delete_asset::DeleteAssetUseCase;
use crate::documents::application::get_head::GetHeadUseCase;
use crate::documents::application::list_assets::ListAssetsUseCase;
use crate::documents::application::list_versions::ListVersionsUseCase;
use crate::documents::application::read_document::ReadDocumentUseCase;
use crate::documents::application::rollback::RollbackUseCase;
use crate::documents::application::upload_asset::UploadAssetUseCase;
use crate::documents::domain::ports::AssetStore;
use crate::documents::domain::{ArtifactStore, IRRenderer, IRValidator, IdGenerator};
use crate::documents::infrastructure::ids::UlidIdGenerator;
use crate::documents::infrastructure::render::{ExcelRenderer, HtmlRenderer, WordRenderer};
#[cfg(feature = "gcs")]
use crate::documents::infrastructure::storage::GcsArtifactStore;
use crate::documents::infrastructure::storage::{LocalFsAssetStore, LocalFsStore};
use crate::documents::infrastructure::validation::{ExcelValidator, HtmlValidator, WordValidator};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Default retention window (versions kept per artifact) when the caller
/// doesn't specify one. Kept generous because Word/Excel patches are tiny.
pub const DEFAULT_RETENTION: u32 = 20;

/// Default storage path for `localfs` backend when no `storage_root` is given.
pub const DEFAULT_STORAGE_ROOT: &str = ".colmena/documents";

/// Default storage path for asset files on the `localfs` backend.
pub const DEFAULT_ASSET_STORAGE_ROOT: &str = ".colmena/documents/assets";

/// Default GCS object prefix for assets when using the `gcs` backend.
pub const DEFAULT_ASSET_GCS_PREFIX: &str = "colmena/documents/assets";

/// Default maximum asset upload size (10 MiB).
pub const DEFAULT_MAX_ASSET_SIZE_BYTES: u64 = 10_485_760;

fn default_allowed_mimes() -> HashSet<String> {
    ["image/png", "image/jpeg", "image/gif", "image/webp"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Pre-built use cases for the documents feature, plus the underlying stores
/// (so callers like the synthetic-tools wiring can attach a session index
/// later without rebuilding everything).
pub struct DocumentRuntime {
    pub store: Arc<dyn ArtifactStore>,
    pub asset_store: Arc<dyn AssetStore>,
    pub create: Arc<CreateDocumentUseCase>,
    pub apply: Arc<ApplyPatchUseCase>,
    pub read: Arc<ReadDocumentUseCase>,
    pub get_head: Arc<GetHeadUseCase>,
    pub list_versions: Arc<ListVersionsUseCase>,
    pub rollback: Arc<RollbackUseCase>,
    pub upload_asset: Arc<UploadAssetUseCase>,
    pub list_assets: Arc<ListAssetsUseCase>,
    pub delete_asset: Arc<DeleteAssetUseCase>,
}

impl DocumentRuntime {
    /// Build a runtime from a node/tool config. Recognised fields:
    ///   - `storage_backend`: "localfs" (default). "gcs" lands in Block B.
    ///   - `storage_root`: path for localfs (default `./.colmena/documents`).
    ///   - `asset_storage_root`: localfs path for assets (default `DEFAULT_ASSET_STORAGE_ROOT`).
    ///   - `asset_gcs_prefix`: GCS prefix for assets (default `DEFAULT_ASSET_GCS_PREFIX`).
    ///   - `default_retention`: u32 (default `DEFAULT_RETENTION`).
    ///   - `max_asset_size_bytes`: u64 (default `DEFAULT_MAX_ASSET_SIZE_BYTES`).
    ///   - `allowed_asset_mimes`: array of MIME strings (default png/jpeg/gif/webp).
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

                // asset_storage_root can be set explicitly; if not, derive a
                // sibling directory named `<artifacts_dir>_assets`.
                let asset_root = config
                    .get("asset_storage_root")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        root.with_file_name(format!(
                            "{}_assets",
                            root.file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("documents")
                        ))
                    });
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
                        .get("asset_gcs_prefix")
                        .and_then(|v| v.as_str())
                        .unwrap_or(DEFAULT_ASSET_GCS_PREFIX);
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

        let max_asset_size_bytes = config
            .get("max_asset_size_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_MAX_ASSET_SIZE_BYTES);

        let allowed_mimes: HashSet<String> = config
            .get("allowed_asset_mimes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_else(default_allowed_mimes);

        Ok(Self::with_store(
            store,
            asset_store,
            default_retention,
            max_asset_size_bytes,
            allowed_mimes,
        ))
    }

    /// Build a runtime around existing stores. Useful for tests and for
    /// callers that share a store across multiple feature areas.
    pub fn with_store(
        store: Arc<dyn ArtifactStore>,
        asset_store: Arc<dyn AssetStore>,
        default_retention: u32,
        max_asset_size_bytes: u64,
        allowed_mimes: HashSet<String>,
    ) -> Arc<Self> {
        let excel_renderer: Arc<dyn IRRenderer> = Arc::new(ExcelRenderer);
        let word_renderer: Arc<dyn IRRenderer> = Arc::new(WordRenderer);
        let html_renderer: Arc<dyn IRRenderer> = Arc::new(HtmlRenderer::new(asset_store.clone()));
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
            html_renderer: html_renderer.clone(),
            html_validator: html_validator.clone(),
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
        let upload_asset = Arc::new(UploadAssetUseCase {
            store: asset_store.clone(),
            ids: ids.clone(),
            max_size_bytes: max_asset_size_bytes,
            allowed_mimes,
        });
        let list_assets = Arc::new(ListAssetsUseCase {
            store: asset_store.clone(),
        });
        let delete_asset = Arc::new(DeleteAssetUseCase {
            assets: asset_store.clone(),
            artifacts: store.clone(),
            index: None,
        });

        Arc::new(Self {
            store,
            asset_store,
            create,
            apply,
            read,
            get_head,
            list_versions,
            rollback,
            upload_asset,
            list_assets,
            delete_asset,
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

    #[tokio::test]
    async fn runtime_creates_html_and_serves_asset_use_cases() {
        use crate::documents::application::create_document::CreateDocumentInput;
        use crate::documents::application::upload_asset::UploadAssetInput;
        use crate::documents::domain::ids::{ArtifactKind, AssetId, SessionId};
        use crate::documents::domain::patch::PatchSource;
        use serde_json::json;
        let tmp = tempdir().unwrap();
        let cfg = json!({
            "storage_root": tmp.path().join("artifacts").to_str().unwrap(),
            "asset_storage_root": tmp.path().join("assets").to_str().unwrap(),
            "max_asset_size_bytes": 1024,
            "allowed_asset_mimes": ["image/png"]
        });
        let rt = DocumentRuntime::from_config(&cfg).await.unwrap();

        let out = rt
            .upload_asset
            .execute(UploadAssetInput {
                session_id: SessionId::new("s1"),
                bytes: b"x".to_vec(),
                mime: "image/png".into(),
                label: None,
            })
            .await
            .unwrap();
        assert!(out.asset_id.as_str().starts_with("asset_"));

        let listed = rt.list_assets.execute(&SessionId::new("s1")).await.unwrap();
        assert_eq!(listed.len(), 1);

        // Sanity: create an HTML doc using the runtime (verifies html bits wired)
        let _ = rt
            .create
            .execute(CreateDocumentInput {
                kind: ArtifactKind::Html,
                session_id: SessionId::new("s1"),
                label: Some("test".into()),
                retention_limit: None,
                initial_ir: None,
                source: PatchSource::Agent,
            })
            .await
            .unwrap();

        let _ = AssetId::new("asset_x"); // ensure import used
    }
}
