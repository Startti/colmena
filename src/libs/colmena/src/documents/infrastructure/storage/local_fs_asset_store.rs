//! LocalFsAssetStore — filesystem adapter for AssetStore.
//!
//! Layout:
//!   {root}/sessions/{session_id}/{asset_id}/bytes.bin
//!   {root}/sessions/{session_id}/{asset_id}/meta.json

use crate::documents::domain::error::AssetError;
use crate::documents::domain::ids::{AssetId, SessionId};
use crate::documents::domain::ports::{AssetStore, AssetSummary};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

pub struct LocalFsAssetStore {
    root: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredMeta {
    session_id: String,
    mime: String,
    size_bytes: u64,
    label: Option<String>,
    created_at: DateTime<Utc>,
}

impl LocalFsAssetStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    fn asset_dir(&self, session: &SessionId, id: &AssetId) -> PathBuf {
        self.root
            .join("sessions")
            .join(session.as_str())
            .join(id.as_str())
    }

    async fn find_asset_dir(&self, id: &AssetId) -> Result<PathBuf, AssetError> {
        let sessions_dir = self.root.join("sessions");
        let mut rd = fs::read_dir(&sessions_dir)
            .await
            .map_err(|e| AssetError::Storage(format!("read sessions dir: {e}")))?;
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| AssetError::Storage(format!("read entry: {e}")))?
        {
            let candidate = entry.path().join(id.as_str());
            if fs::try_exists(&candidate).await.unwrap_or(false) {
                return Ok(candidate);
            }
        }
        Err(AssetError::NotFound { id: id.clone() })
    }
}

#[async_trait]
impl AssetStore for LocalFsAssetStore {
    async fn upload(
        &self,
        session: &SessionId,
        id: &AssetId,
        bytes: Vec<u8>,
        mime: &str,
        label: Option<&str>,
    ) -> Result<(), AssetError> {
        let dir = self.asset_dir(session, id);
        fs::create_dir_all(&dir)
            .await
            .map_err(|e| AssetError::Storage(format!("mkdir {dir:?}: {e}")))?;

        let tmp = dir.join("bytes.bin.tmp");
        let final_path = dir.join("bytes.bin");
        let size = bytes.len() as u64;
        fs::write(&tmp, &bytes)
            .await
            .map_err(|e| AssetError::Storage(format!("write bytes: {e}")))?;
        fs::rename(&tmp, &final_path)
            .await
            .map_err(|e| AssetError::Storage(format!("rename bytes: {e}")))?;

        let meta = StoredMeta {
            session_id: session.as_str().to_string(),
            mime: mime.to_string(),
            size_bytes: size,
            label: label.map(|s| s.to_string()),
            created_at: Utc::now(),
        };
        let meta_tmp = dir.join("meta.json.tmp");
        let meta_final = dir.join("meta.json");
        let meta_bytes = serde_json::to_vec_pretty(&meta)
            .map_err(|e| AssetError::Storage(format!("ser meta: {e}")))?;
        fs::write(&meta_tmp, &meta_bytes)
            .await
            .map_err(|e| AssetError::Storage(format!("write meta: {e}")))?;
        fs::rename(&meta_tmp, &meta_final)
            .await
            .map_err(|e| AssetError::Storage(format!("rename meta: {e}")))?;
        Ok(())
    }

    async fn read(&self, id: &AssetId) -> Result<(Vec<u8>, String), AssetError> {
        let dir = self.find_asset_dir(id).await?;
        let meta_bytes = fs::read(dir.join("meta.json"))
            .await
            .map_err(|e| AssetError::Storage(format!("read meta: {e}")))?;
        let meta: StoredMeta = serde_json::from_slice(&meta_bytes)
            .map_err(|e| AssetError::Storage(format!("parse meta: {e}")))?;
        let bytes = fs::read(dir.join("bytes.bin"))
            .await
            .map_err(|e| AssetError::Storage(format!("read bytes: {e}")))?;
        Ok((bytes, meta.mime))
    }

    async fn list_by_session(
        &self,
        session: &SessionId,
    ) -> Result<Vec<AssetSummary>, AssetError> {
        let sdir = self.root.join("sessions").join(session.as_str());
        if !fs::try_exists(&sdir).await.unwrap_or(false) {
            return Ok(vec![]);
        }
        let mut out = vec![];
        let mut rd = fs::read_dir(&sdir)
            .await
            .map_err(|e| AssetError::Storage(format!("read session dir: {e}")))?;
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| AssetError::Storage(format!("read entry: {e}")))?
        {
            let id_str = entry.file_name().to_string_lossy().into_owned();
            let meta_path = entry.path().join("meta.json");
            if !fs::try_exists(&meta_path).await.unwrap_or(false) {
                continue;
            }
            let meta_bytes = fs::read(&meta_path)
                .await
                .map_err(|e| AssetError::Storage(format!("read meta: {e}")))?;
            let meta: StoredMeta = serde_json::from_slice(&meta_bytes)
                .map_err(|e| AssetError::Storage(format!("parse meta: {e}")))?;
            out.push(AssetSummary {
                id: AssetId::new(id_str),
                session_id: SessionId::new(meta.session_id),
                mime: meta.mime,
                size_bytes: meta.size_bytes,
                label: meta.label,
                created_at: meta.created_at,
            });
        }
        Ok(out)
    }

    async fn delete(&self, id: &AssetId) -> Result<(), AssetError> {
        let dir = self.find_asset_dir(id).await?;
        fs::remove_dir_all(&dir)
            .await
            .map_err(|e| AssetError::Storage(format!("remove dir: {e}")))?;
        Ok(())
    }

    async fn head(&self, id: &AssetId) -> Result<AssetSummary, AssetError> {
        let dir = self.find_asset_dir(id).await?;
        let meta_bytes = fs::read(dir.join("meta.json"))
            .await
            .map_err(|e| AssetError::Storage(format!("read meta: {e}")))?;
        let meta: StoredMeta = serde_json::from_slice(&meta_bytes)
            .map_err(|e| AssetError::Storage(format!("parse meta: {e}")))?;
        let session_id = dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(AssetSummary {
            id: id.clone(),
            session_id: SessionId::new(session_id),
            mime: meta.mime,
            size_bytes: meta.size_bytes,
            label: meta.label,
            created_at: meta.created_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn upload_then_read_roundtrip() {
        let tmp = tempdir().unwrap();
        let store = LocalFsAssetStore::new(tmp.path());
        let session = SessionId::new("s1");
        let id = AssetId::new("asset_a");
        store
            .upload(&session, &id, b"hello".to_vec(), "image/png", Some("logo"))
            .await
            .unwrap();
        let (bytes, mime) = store.read(&id).await.unwrap();
        assert_eq!(bytes, b"hello");
        assert_eq!(mime, "image/png");
    }

    #[tokio::test]
    async fn list_returns_summaries_with_label() {
        let tmp = tempdir().unwrap();
        let store = LocalFsAssetStore::new(tmp.path());
        let s = SessionId::new("s1");
        store
            .upload(&s, &AssetId::new("asset_1"), b"a".to_vec(), "image/png", Some("a"))
            .await
            .unwrap();
        store
            .upload(&s, &AssetId::new("asset_2"), b"bb".to_vec(), "image/jpeg", None)
            .await
            .unwrap();
        let list = store.list_by_session(&s).await.unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|a| a.label.as_deref() == Some("a")));
    }

    #[tokio::test]
    async fn delete_removes_asset() {
        let tmp = tempdir().unwrap();
        let store = LocalFsAssetStore::new(tmp.path());
        let s = SessionId::new("s1");
        let id = AssetId::new("asset_x");
        store
            .upload(&s, &id, b"x".to_vec(), "image/png", None)
            .await
            .unwrap();
        store.delete(&id).await.unwrap();
        let err = store.read(&id).await.unwrap_err();
        assert!(matches!(err, AssetError::NotFound { .. }));
    }

    #[tokio::test]
    async fn list_empty_session_returns_empty_vec() {
        let tmp = tempdir().unwrap();
        let store = LocalFsAssetStore::new(tmp.path());
        let list = store.list_by_session(&SessionId::new("nobody")).await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn head_without_loading_bytes() {
        let tmp = tempdir().unwrap();
        let store = LocalFsAssetStore::new(tmp.path());
        let s = SessionId::new("s1");
        store
            .upload(&s, &AssetId::new("asset_h"), vec![0u8; 1024], "image/png", None)
            .await
            .unwrap();
        let head = store.head(&AssetId::new("asset_h")).await.unwrap();
        assert_eq!(head.size_bytes, 1024);
        assert_eq!(head.mime, "image/png");
    }
}
