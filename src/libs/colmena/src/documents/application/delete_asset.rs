//! DeleteAssetUseCase — refuses deletion if asset is still referenced by
//! any artifact in the same session.

use crate::documents::domain::error::AssetError;
use crate::documents::domain::ids::{ArtifactId, AssetId, SessionId};
use crate::documents::domain::ports::{ArtifactStore, AssetStore, SessionArtifactIndex};
use std::sync::Arc;

pub struct DeleteAssetInput {
    pub session_id: SessionId,
    pub asset_id: AssetId,
}

pub struct DeleteAssetUseCase {
    pub assets: Arc<dyn AssetStore>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub index: Option<Arc<dyn SessionArtifactIndex>>,
}

impl DeleteAssetUseCase {
    pub async fn execute(&self, input: DeleteAssetInput) -> Result<(), AssetError> {
        let mut blockers: Vec<ArtifactId> = vec![];
        if let Some(idx) = &self.index {
            let arts = idx
                .list_by_session(&input.session_id)
                .await
                .map_err(|e| AssetError::Storage(format!("index list: {e}")))?;
            for summary in arts {
                let data = self
                    .artifacts
                    .read_current(&summary.artifact_id)
                    .await
                    .map_err(|e| AssetError::Storage(format!("read artifact: {e}")))?;
                if let Some(arr) = data.ir.get("assets_referenced").and_then(|v| v.as_array()) {
                    if arr
                        .iter()
                        .any(|v| v.as_str() == Some(input.asset_id.as_str()))
                    {
                        blockers.push(summary.artifact_id.clone());
                    }
                }
            }
        }
        if !blockers.is_empty() {
            return Err(AssetError::StillReferenced {
                id: input.asset_id,
                by_artifacts: blockers,
            });
        }
        self.assets.delete(&input.asset_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::artifact::{
        ArtifactMeta, ArtifactSummary, PatchApplied, PatchSummary, VersionData,
    };
    use crate::documents::domain::error::{IndexError, StorageError};
    use crate::documents::domain::ids::{ArtifactKind, VersionId};
    use crate::documents::infrastructure::storage::LocalFsAssetStore;
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::tempdir;

    /// Minimal in-memory ArtifactStore stub for ref-counting tests.
    struct StubArtifactStore {
        data: Mutex<HashMap<String, serde_json::Value>>,
    }
    impl StubArtifactStore {
        fn with(ir: serde_json::Value, id: &str) -> Arc<Self> {
            let mut m = HashMap::new();
            m.insert(id.into(), ir);
            Arc::new(Self {
                data: Mutex::new(m),
            })
        }
    }

    #[async_trait]
    impl ArtifactStore for StubArtifactStore {
        async fn create_artifact(&self, _m: &ArtifactMeta) -> Result<(), StorageError> {
            Ok(())
        }
        async fn write_version(
            &self,
            _id: &ArtifactId,
            _v: &VersionId,
            _d: &VersionData,
        ) -> Result<(), StorageError> {
            Ok(())
        }
        async fn read_version(
            &self,
            _id: &ArtifactId,
            _v: &VersionId,
        ) -> Result<VersionData, StorageError> {
            unimplemented!()
        }
        async fn read_current(&self, id: &ArtifactId) -> Result<VersionData, StorageError> {
            let m = self.data.lock().unwrap();
            let ir = m
                .get(id.as_str())
                .cloned()
                .ok_or_else(|| StorageError::NotFound(id.as_str().into()))?;
            Ok(VersionData {
                ir,
                rendered_binary: vec![],
                rendered_extension: "html",
                patch_applied: PatchApplied {
                    patch: serde_json::json!({}),
                    applied_at: Utc::now(),
                    resulted_in: VersionId::initial(),
                    summary: PatchSummary::default(),
                },
                blobs: vec![],
            })
        }
        async fn list_versions(&self, _id: &ArtifactId) -> Result<Vec<VersionId>, StorageError> {
            Ok(vec![])
        }
        async fn set_head(
            &self,
            _id: &ArtifactId,
            _e: Option<&VersionId>,
            _n: &VersionId,
        ) -> Result<(), StorageError> {
            Ok(())
        }
        async fn delete_version(
            &self,
            _id: &ArtifactId,
            _v: &VersionId,
        ) -> Result<(), StorageError> {
            Ok(())
        }
        async fn read_meta(&self, _id: &ArtifactId) -> Result<ArtifactMeta, StorageError> {
            unimplemented!()
        }
        async fn update_meta(
            &self,
            _id: &ArtifactId,
            _m: &ArtifactMeta,
        ) -> Result<(), StorageError> {
            Ok(())
        }
        async fn delete_artifact(&self, _id: &ArtifactId) -> Result<(), StorageError> {
            Ok(())
        }
    }

    struct StubIndex(Vec<ArtifactSummary>);

    #[async_trait]
    impl SessionArtifactIndex for StubIndex {
        async fn register(
            &self,
            _s: &SessionId,
            _id: &ArtifactId,
            _m: &ArtifactMeta,
        ) -> Result<(), IndexError> {
            Ok(())
        }
        async fn list_by_session(
            &self,
            _s: &SessionId,
        ) -> Result<Vec<ArtifactSummary>, IndexError> {
            Ok(self.0.clone())
        }
        async fn lookup(&self, _id: &ArtifactId) -> Result<Option<ArtifactSummary>, IndexError> {
            Ok(None)
        }
        async fn update_head(
            &self,
            _id: &ArtifactId,
            _v: &VersionId,
            _u: DateTime<Utc>,
        ) -> Result<(), IndexError> {
            Ok(())
        }
        async fn unregister(&self, _id: &ArtifactId) -> Result<(), IndexError> {
            Ok(())
        }
    }

    fn summary(id: &str) -> ArtifactSummary {
        ArtifactSummary {
            artifact_id: ArtifactId::new(id),
            session_id: SessionId::new("s1"),
            kind: ArtifactKind::Html,
            label: None,
            current_version: VersionId::initial(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn delete_blocked_when_referenced() {
        let tmp = tempdir().unwrap();
        let assets: Arc<dyn AssetStore> = Arc::new(LocalFsAssetStore::new(tmp.path()));
        let id = AssetId::new("asset_ref");
        assets
            .upload(&SessionId::new("s1"), &id, b"x".to_vec(), "image/png", None)
            .await
            .unwrap();
        let artifacts: Arc<dyn ArtifactStore> = StubArtifactStore::with(
            serde_json::json!({"assets_referenced": ["asset_ref"]}),
            "art_a",
        );
        let index: Arc<dyn SessionArtifactIndex> = Arc::new(StubIndex(vec![summary("art_a")]));
        let uc = DeleteAssetUseCase {
            assets,
            artifacts,
            index: Some(index),
        };
        let err = uc
            .execute(DeleteAssetInput {
                session_id: SessionId::new("s1"),
                asset_id: id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, AssetError::StillReferenced { .. }));
    }

    #[tokio::test]
    async fn delete_succeeds_when_not_referenced() {
        let tmp = tempdir().unwrap();
        let assets: Arc<dyn AssetStore> = Arc::new(LocalFsAssetStore::new(tmp.path()));
        let id = AssetId::new("asset_free");
        assets
            .upload(&SessionId::new("s1"), &id, b"x".to_vec(), "image/png", None)
            .await
            .unwrap();
        let artifacts: Arc<dyn ArtifactStore> =
            StubArtifactStore::with(serde_json::json!({"assets_referenced": []}), "art_a");
        let index: Arc<dyn SessionArtifactIndex> = Arc::new(StubIndex(vec![summary("art_a")]));
        let uc = DeleteAssetUseCase {
            assets,
            artifacts,
            index: Some(index),
        };
        uc.execute(DeleteAssetInput {
            session_id: SessionId::new("s1"),
            asset_id: id,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn delete_skips_check_when_no_index() {
        let tmp = tempdir().unwrap();
        let assets: Arc<dyn AssetStore> = Arc::new(LocalFsAssetStore::new(tmp.path()));
        let id = AssetId::new("asset_noindex");
        assets
            .upload(&SessionId::new("s1"), &id, b"x".to_vec(), "image/png", None)
            .await
            .unwrap();
        // Even with a non-empty ArtifactStore, no index means no check.
        let artifacts: Arc<dyn ArtifactStore> = StubArtifactStore::with(
            serde_json::json!({"assets_referenced": ["asset_noindex"]}),
            "art_a",
        );
        let uc = DeleteAssetUseCase {
            assets,
            artifacts,
            index: None,
        };
        uc.execute(DeleteAssetInput {
            session_id: SessionId::new("s1"),
            asset_id: id,
        })
        .await
        .unwrap();
    }
}
