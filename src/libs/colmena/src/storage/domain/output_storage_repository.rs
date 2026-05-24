use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

use crate::storage::domain::storage_error::StorageError;

#[derive(Debug, Clone)]
pub struct StoreRequest {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub filename: String,
    pub session_id: Option<String>,
    pub agent_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredOutput {
    pub storage_key: String,
    pub read_url: String,
    pub mime_type: String,
    pub filename: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct StoredBytes {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub filename: String,
}

/// Streaming counterpart of [`StoredBytes`]. Used by `http_request` multipart
/// mode to forward bytes from storage to a downstream endpoint without
/// buffering the full payload in worker RAM.
///
/// `size_bytes` is mandatory — it lets the multipart sender announce
/// `Content-Length` per part, which downstream servers require.
pub struct StoredStream {
    pub stream: Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>>,
    pub size_bytes: u64,
    pub mime_type: String,
    pub filename: String,
}

impl std::fmt::Debug for StoredStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredStream")
            .field("size_bytes", &self.size_bytes)
            .field("mime_type", &self.mime_type)
            .field("filename", &self.filename)
            .field("stream", &"<async stream>")
            .finish()
    }
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait OutputStorageRepository: Send + Sync {
    async fn store(&self, req: StoreRequest) -> Result<StoredOutput, StorageError>;

    async fn read(&self, storage_key: &str) -> Result<StoredBytes, StorageError>;

    /// Streaming counterpart to [`read`]. Required by `http_request` multipart
    /// mode. Implementations must return a stream that yields the bytes of the
    /// stored object in order, plus accurate `size_bytes` metadata.
    ///
    /// Returns `StorageError::InvalidInput` for unknown `storage_key`.
    /// Returns `StorageError::BackendUnavailable` if the underlying source
    /// cannot be reached.
    async fn read_stream(&self, storage_key: &str) -> Result<StoredStream, StorageError>;
}
