//! GCS backend — to be implemented when `--features gcs` is needed.
//! v1 ships localfs only; this file exists so the cfg arm compiles.

#![cfg(feature = "gcs")]

use super::*;

pub struct GcsStorage;

impl GcsStorage {
    pub fn new(_bucket: String, _prefix: String) -> Result<Self, StorageError> {
        Err(StorageError::Backend(
            "GcsStorage not implemented yet — coming in a follow-up task".into(),
        ))
    }
}

#[async_trait]
impl ArtifactStorage for GcsStorage {
    async fn list(&self) -> Result<Vec<ArtifactMeta>, StorageError> {
        unreachable!()
    }
    async fn load_state(&self, _: &ArtifactId) -> Result<Option<Vec<u8>>, StorageError> {
        unreachable!()
    }
    async fn load_meta(&self, _: &ArtifactId) -> Result<Option<ArtifactMeta>, StorageError> {
        unreachable!()
    }
    async fn save_state(&self, _: &ArtifactId, _: &[u8]) -> Result<(), StorageError> {
        unreachable!()
    }
    async fn save_meta(&self, _: &ArtifactMeta) -> Result<(), StorageError> {
        unreachable!()
    }
    async fn delete(&self, _: &ArtifactId) -> Result<(), StorageError> {
        unreachable!()
    }
}
