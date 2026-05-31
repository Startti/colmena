//! ListAssetsUseCase — thin wrapper over AssetStore::list_by_session.

use crate::documents::domain::error::AssetError;
use crate::documents::domain::ids::SessionId;
use crate::documents::domain::ports::{AssetStore, AssetSummary};
use std::sync::Arc;

pub struct ListAssetsUseCase {
    pub store: Arc<dyn AssetStore>,
}

impl ListAssetsUseCase {
    pub async fn execute(&self, session_id: &SessionId) -> Result<Vec<AssetSummary>, AssetError> {
        self.store.list_by_session(session_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ids::AssetId;
    use crate::documents::infrastructure::storage::LocalFsAssetStore;
    use tempfile::tempdir;

    #[tokio::test]
    async fn returns_summaries_for_session() {
        let tmp = tempdir().unwrap();
        let store: Arc<dyn AssetStore> = Arc::new(LocalFsAssetStore::new(tmp.path()));
        let s = SessionId::new("s1");
        store
            .upload(
                &s,
                &AssetId::new("asset_1"),
                b"a".to_vec(),
                "image/png",
                None,
            )
            .await
            .unwrap();
        let uc = ListAssetsUseCase { store };
        let list = uc.execute(&s).await.unwrap();
        assert_eq!(list.len(), 1);
    }
}
