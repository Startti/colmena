//! UploadAssetUseCase — validates + persists via AssetStore port.

use crate::documents::domain::error::AssetError;
use crate::documents::domain::ids::{AssetId, SessionId};
use crate::documents::domain::ports::{AssetStore, AssetSummary};
use crate::documents::domain::IdGenerator;
use std::collections::HashSet;
use std::sync::Arc;

pub struct UploadAssetInput {
    pub session_id: SessionId,
    pub bytes: Vec<u8>,
    pub mime: String,
    pub label: Option<String>,
}

#[derive(Debug)]
pub struct UploadAssetOutput {
    pub asset_id: AssetId,
    pub summary: AssetSummary,
}

pub struct UploadAssetUseCase {
    pub store: Arc<dyn AssetStore>,
    pub ids: Arc<dyn IdGenerator>,
    pub max_size_bytes: u64,
    pub allowed_mimes: HashSet<String>,
}

impl UploadAssetUseCase {
    pub async fn execute(
        &self,
        input: UploadAssetInput,
    ) -> Result<UploadAssetOutput, AssetError> {
        let size = input.bytes.len() as u64;
        if size > self.max_size_bytes {
            return Err(AssetError::TooLarge {
                actual: size,
                max: self.max_size_bytes,
            });
        }
        if !self.allowed_mimes.contains(&input.mime) {
            return Err(AssetError::MimeNotAllowed { mime: input.mime });
        }
        let id = AssetId::new(self.ids.new_asset_id());
        self.store
            .upload(
                &input.session_id,
                &id,
                input.bytes,
                &input.mime,
                input.label.as_deref(),
            )
            .await?;
        let summary = self.store.head(&id).await?;
        Ok(UploadAssetOutput {
            asset_id: id,
            summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::infrastructure::ids::CountingIdGenerator;
    use crate::documents::infrastructure::storage::LocalFsAssetStore;
    use tempfile::tempdir;

    fn allowed() -> HashSet<String> {
        ["image/png", "image/jpeg"]
            .into_iter()
            .map(String::from)
            .collect()
    }

    #[tokio::test]
    async fn upload_happy_path() {
        let tmp = tempdir().unwrap();
        let uc = UploadAssetUseCase {
            store: Arc::new(LocalFsAssetStore::new(tmp.path())),
            ids: Arc::new(CountingIdGenerator::default()),
            max_size_bytes: 10 * 1024,
            allowed_mimes: allowed(),
        };
        let out = uc
            .execute(UploadAssetInput {
                session_id: SessionId::new("s1"),
                bytes: b"hello".to_vec(),
                mime: "image/png".into(),
                label: Some("logo".into()),
            })
            .await
            .unwrap();
        assert_eq!(out.asset_id.as_str(), "asset_01");
        assert_eq!(out.summary.size_bytes, 5);
    }

    #[tokio::test]
    async fn rejects_too_large() {
        let tmp = tempdir().unwrap();
        let uc = UploadAssetUseCase {
            store: Arc::new(LocalFsAssetStore::new(tmp.path())),
            ids: Arc::new(CountingIdGenerator::default()),
            max_size_bytes: 4,
            allowed_mimes: allowed(),
        };
        let err = uc
            .execute(UploadAssetInput {
                session_id: SessionId::new("s1"),
                bytes: b"hello".to_vec(),
                mime: "image/png".into(),
                label: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, AssetError::TooLarge { .. }));
    }

    #[tokio::test]
    async fn rejects_disallowed_mime() {
        let tmp = tempdir().unwrap();
        let uc = UploadAssetUseCase {
            store: Arc::new(LocalFsAssetStore::new(tmp.path())),
            ids: Arc::new(CountingIdGenerator::default()),
            max_size_bytes: 1024,
            allowed_mimes: allowed(),
        };
        let err = uc
            .execute(UploadAssetInput {
                session_id: SessionId::new("s1"),
                bytes: vec![0u8; 10],
                mime: "application/pdf".into(),
                label: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, AssetError::MimeNotAllowed { .. }));
    }
}
