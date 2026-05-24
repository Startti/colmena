use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

use crate::storage::domain::storage_error::StorageError;

/// A request to persist a freshly generated artifact (image, audio file, etc.).
///
/// `session_id` and `agent_session_id` are forwarded to backends that derive
/// the storage path from the conversation scope (e.g. the ADP HTTP callback
/// uses them to build `chat-attachments/<userId>/<sessionId>/generated/...`).
/// Local / test adapters typically ignore them.
#[derive(Debug, Clone)]
pub struct StoreRequest {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub filename: String,
    pub session_id: Option<String>,
    pub agent_session_id: Option<String>,
}

/// Handle returned after a successful store. `storage_key` is the canonical,
/// stable reference (used by downstream tool calls to chain operations).
/// `read_url` is a (typically short-lived) URL the caller can hand to a UI or
/// to the LLM as part of the tool output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredOutput {
    pub storage_key: String,
    pub read_url: String,
    pub mime_type: String,
    pub filename: String,
    pub size_bytes: u64,
}

/// Bytes + minimal metadata returned by [`OutputStorageRepository::read`].
/// Used by cross-provider lazy upload (load_attachment with provider=Generated)
/// and by the `$attachment:<id>` placeholder resolver in http_request.
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

/// Port for persisting generated output media. Two adapters ship with
/// Colmena: `LocalCacheStorageAdapter` (in-memory + base64 `data:` URLs for
/// CLI / tests) and `HttpCallbackStorageAdapter` (production: asks an external
/// API for a signed PUT URL and uploads to object storage). Consumers wire one
/// of them into `EngineConfig::storage`.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait OutputStorageRepository: Send + Sync {
    /// Persist `req.bytes` and return a stable handle (`storage_key`) plus a
    /// fetchable `read_url`.
    async fn store(&self, req: StoreRequest) -> Result<StoredOutput, StorageError>;

    /// Retrieve the bytes for a previously-stored output. Required by:
    /// - cross-provider lazy upload (e.g. image generated via OpenAI then
    ///   requested via Anthropic in a later turn — needs the bytes to upload
    ///   to Anthropic's Files API)
    /// - the `$attachment:<id>` placeholder in `http_request`
    ///
    /// Returns `StorageError::InvalidInput` for unknown `storage_key`.
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
