//! Persistence backend for CRDT documents (state + metadata).

use crate::crdt_documents::ArtifactId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

pub mod localfs;
#[cfg(feature = "gcs")]
pub mod gcs;

pub use localfs::LocalFsStorage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMeta {
    pub artifact_id: ArtifactId,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub sheet_count: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("backend: {0}")]
    Backend(String),
}

#[async_trait]
pub trait ArtifactStorage: Send + Sync {
    async fn list(&self) -> Result<Vec<ArtifactMeta>, StorageError>;
    async fn load_state(&self, id: &ArtifactId) -> Result<Option<Vec<u8>>, StorageError>;
    async fn load_meta(&self, id: &ArtifactId) -> Result<Option<ArtifactMeta>, StorageError>;
    async fn save_state(&self, id: &ArtifactId, bytes: &[u8]) -> Result<(), StorageError>;
    async fn save_meta(&self, meta: &ArtifactMeta) -> Result<(), StorageError>;
    async fn delete(&self, id: &ArtifactId) -> Result<(), StorageError>;
}

#[derive(Debug, Clone)]
pub enum StorageConfig {
    LocalFs { root: PathBuf },
    #[cfg(feature = "gcs")]
    Gcs { bucket: String, prefix: String },
}

impl StorageConfig {
    pub fn build(self) -> Result<Arc<dyn ArtifactStorage>, StorageError> {
        match self {
            StorageConfig::LocalFs { root } => Ok(Arc::new(LocalFsStorage::new(root)?)),
            #[cfg(feature = "gcs")]
            StorageConfig::Gcs { bucket, prefix } => {
                Ok(Arc::new(gcs::GcsStorage::new(bucket, prefix)?))
            }
        }
    }
}
