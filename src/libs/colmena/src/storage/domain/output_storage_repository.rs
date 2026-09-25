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

    /// Plan C: delete the blob associated with `storage_key`. Idempotent —
    /// returns `Ok(())` whether or not the blob existed. Backend failures
    /// (e.g., GCS unavailable) bubble up as `StorageError::BackendUnavailable`
    /// so the `attachment_gc` binary can retry on the next scheduled run.
    async fn delete(&self, storage_key: &str) -> Result<(), StorageError>;

    /// Feature C (additive, part 1 of 2): ask the host for a **read URL**
    /// for an existing `storage_key`, hinted to remain valid for roughly
    /// `ttl_seconds`. Distinct from `store`'s `read_url` field (issued at
    /// write time, for a brand-new object); this one is issued on demand for
    /// an object that may already be old.
    ///
    /// Default `Ok(None)` — an existing host that predates this method
    /// compiles and runs unchanged, and `None` means "this host does not
    /// provide read URLs". Part 2 of this feature (the `$attachment_url:`
    /// placeholder in `http_request`) turns that `None` into a clear tool
    /// error ("this host does not provide attachment URLs; use
    /// `$attachment:<id>` for the bytes") instead of falling back silently.
    ///
    /// **Signing never lives in this library.** A host that wants real URLs
    /// (e.g. ADP, which signs a GCS GET) overrides this method in its own
    /// `OutputStorageRepository` adapter and calls its own signing
    /// protocol; the library only defines the shape and forwards
    /// `ttl_seconds` unmodified — the host applies its own floor/cap (ADP:
    /// 24h) on top of whatever the caller asked for.
    ///
    /// `ttl_seconds` is a **hint**, not a guarantee: an adapter may return a
    /// URL valid for more or less time than requested. `LocalHttp`'s static
    /// file server never expires, so it accepts and ignores the hint.
    async fn read_url(
        &self,
        storage_key: &str,
        ttl_seconds: u64,
    ) -> Result<Option<String>, StorageError> {
        let _ = (storage_key, ttl_seconds);
        Ok(None)
    }

    /// Capability hint consumed by the `$attachment_url:` placeholder's
    /// teaching gate (part 2): the catalog/prelude mentions that
    /// placeholder to the model **only when this returns `true`**, so the
    /// model is never taught a form that fails. Default `false`, matching
    /// the default `read_url`. An adapter that overrides `read_url` to
    /// return real URLs overrides this alongside it — every shipping
    /// adapter keeps the two in sync, though nothing at the type level
    /// forces that.
    fn supports_read_url(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// Implements only the pre-Feature-C surface, leaning entirely on the
    /// trait's default `read_url` / `supports_read_url`. Stands in for "an
    /// existing host that doesn't implement it" — the additive-compatibility
    /// claim in the port's doc comment, exercised as a real trait object
    /// (not just a compile check) so a future regression that special-cases
    /// some concrete type would still be caught.
    struct LegacyAdapter;

    #[async_trait]
    impl OutputStorageRepository for LegacyAdapter {
        async fn store(&self, _req: StoreRequest) -> Result<StoredOutput, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn read(&self, _storage_key: &str) -> Result<StoredBytes, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn read_stream(&self, _storage_key: &str) -> Result<StoredStream, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn delete(&self, _storage_key: &str) -> Result<(), StorageError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn default_read_url_is_ok_none_for_a_host_that_does_not_implement_it() {
        let adapter: Arc<dyn OutputStorageRepository> = Arc::new(LegacyAdapter);
        let got = adapter.read_url("any-key", 900).await;
        assert!(matches!(got, Ok(None)), "expected Ok(None), got {got:?}");
    }

    #[test]
    fn default_supports_read_url_is_false() {
        let adapter = LegacyAdapter;
        assert!(!adapter.supports_read_url());
    }

    /// A minimal adapter that DOES override `read_url`, to prove the
    /// `ttl_seconds` argument the part-2 placeholder resolver will pass
    /// actually reaches an implementation unmodified — not swallowed or
    /// hardcoded somewhere between the caller and the port.
    struct TtlSpyAdapter {
        seen_ttl: AtomicU64,
    }

    #[async_trait]
    impl OutputStorageRepository for TtlSpyAdapter {
        async fn store(&self, _req: StoreRequest) -> Result<StoredOutput, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn read(&self, _storage_key: &str) -> Result<StoredBytes, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn read_stream(&self, _storage_key: &str) -> Result<StoredStream, StorageError> {
            unimplemented!("not exercised by these tests")
        }
        async fn delete(&self, _storage_key: &str) -> Result<(), StorageError> {
            Ok(())
        }
        async fn read_url(
            &self,
            _storage_key: &str,
            ttl_seconds: u64,
        ) -> Result<Option<String>, StorageError> {
            self.seen_ttl.store(ttl_seconds, Ordering::SeqCst);
            Ok(Some("https://spy.test/x".to_string()))
        }
        fn supports_read_url(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn ttl_argument_reaches_the_implementation() {
        let adapter = TtlSpyAdapter {
            seen_ttl: AtomicU64::new(0),
        };
        let got = adapter.read_url("k1", 3600).await.unwrap();
        assert_eq!(got, Some("https://spy.test/x".to_string()));
        assert_eq!(adapter.seen_ttl.load(Ordering::SeqCst), 3600);
        assert!(adapter.supports_read_url());
    }
}
