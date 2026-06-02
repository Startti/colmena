//! Filesystem-backed `ArtifactStorage`. Layout:
//!
//! ```text
//! {root}/
//!   {artifact_id}/
//!     meta.json
//!     state.yjs
//! ```

use super::*;
use std::path::PathBuf;
use tokio::fs;

pub struct LocalFsStorage {
    root: PathBuf,
}

impl LocalFsStorage {
    pub fn new(root: PathBuf) -> Result<Self, StorageError> {
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn artifact_dir(&self, id: &ArtifactId) -> PathBuf {
        self.root.join(id.as_str())
    }

    fn meta_path(&self, id: &ArtifactId) -> PathBuf {
        self.artifact_dir(id).join("meta.json")
    }

    fn state_path(&self, id: &ArtifactId) -> PathBuf {
        self.artifact_dir(id).join("state.yjs")
    }
}

#[async_trait]
impl ArtifactStorage for LocalFsStorage {
    async fn list(&self) -> Result<Vec<ArtifactMeta>, StorageError> {
        let mut out = Vec::new();
        let mut read = fs::read_dir(&self.root).await?;
        while let Some(entry) = read.next_entry().await? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let meta_p = path.join("meta.json");
            if meta_p.exists() {
                let bytes = fs::read(&meta_p).await?;
                match serde_json::from_slice::<ArtifactMeta>(&bytes) {
                    Ok(meta) => out.push(meta),
                    Err(e) => tracing::warn!("skipping malformed meta.json at {:?}: {}", meta_p, e),
                }
            }
        }
        Ok(out)
    }

    async fn load_state(&self, id: &ArtifactId) -> Result<Option<Vec<u8>>, StorageError> {
        match fs::read(&self.state_path(id)).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StorageError::Io(e)),
        }
    }

    async fn load_meta(&self, id: &ArtifactId) -> Result<Option<ArtifactMeta>, StorageError> {
        match fs::read(&self.meta_path(id)).await {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StorageError::Io(e)),
        }
    }

    async fn save_state(&self, id: &ArtifactId, bytes: &[u8]) -> Result<(), StorageError> {
        fs::create_dir_all(self.artifact_dir(id)).await?;
        let final_path = self.state_path(id);
        let tmp = final_path.with_extension(format!("yjs.{}.tmp", ulid::Ulid::new()));
        fs::write(&tmp, bytes).await?;
        fs::rename(&tmp, &final_path).await?;
        Ok(())
    }

    async fn save_meta(&self, meta: &ArtifactMeta) -> Result<(), StorageError> {
        fs::create_dir_all(self.artifact_dir(&meta.artifact_id)).await?;
        let final_path = self.meta_path(&meta.artifact_id);
        let tmp = final_path.with_extension(format!("json.{}.tmp", ulid::Ulid::new()));
        let bytes = serde_json::to_vec_pretty(meta)?;
        fs::write(&tmp, &bytes).await?;
        fs::rename(&tmp, &final_path).await?;
        Ok(())
    }

    async fn delete(&self, id: &ArtifactId) -> Result<(), StorageError> {
        let dir = self.artifact_dir(id);
        if dir.exists() {
            fs::remove_dir_all(&dir).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "crdt_storage_test_{}",
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn save_then_load_state_round_trip() {
        let root = temp_root();
        let store = LocalFsStorage::new(root.clone()).unwrap();
        let id = ArtifactId::new();
        store.save_state(&id, b"hello").await.unwrap();
        let loaded = store.load_state(&id).await.unwrap();
        assert_eq!(loaded.as_deref(), Some(&b"hello"[..]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn list_returns_all_with_meta() {
        let root = temp_root();
        let store = LocalFsStorage::new(root.clone()).unwrap();
        let id1 = ArtifactId::new();
        let id2 = ArtifactId::new();
        store
            .save_meta(&ArtifactMeta {
                artifact_id: id1.clone(),
                name: "A".into(),
                created_at: 1,
                updated_at: 1,
                sheet_count: 0,
            })
            .await
            .unwrap();
        store
            .save_meta(&ArtifactMeta {
                artifact_id: id2.clone(),
                name: "B".into(),
                created_at: 2,
                updated_at: 2,
                sheet_count: 1,
            })
            .await
            .unwrap();
        let listed = store.list().await.unwrap();
        let names: std::collections::HashSet<_> = listed.iter().map(|m| m.name.clone()).collect();
        assert_eq!(names.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn delete_removes_dir() {
        let root = temp_root();
        let store = LocalFsStorage::new(root.clone()).unwrap();
        let id = ArtifactId::new();
        store.save_state(&id, b"x").await.unwrap();
        store.delete(&id).await.unwrap();
        assert!(store.load_state(&id).await.unwrap().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
