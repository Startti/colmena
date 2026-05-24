//! In-process storage adapter that keeps bytes in an internal `DashMap`
//! keyed by `storage_key`. No filesystem writes, no network calls. Intended
//! for CLI runs, unit tests, and `cargo run --bin dag_engine -- run` invocations.
//!
//! ## URL strategy
//!
//! `store()` returns a `read_url` equal to the `storage_key` itself
//! (`local://<uuid>`). This is intentionally **short**: the LLM tool-result
//! includes this URL as a handle, and a multi-MB `data:` URL would saturate
//! the LLM's TPM (tokens-per-minute) rate limit on the very next turn.
//!
//! For inspection (CLI dev workflow), the bytes are retrieved via
//! [`read`](OutputStorageRepository::read) — see the `scripts/` helpers or
//! `cargo run` event-stream extraction examples. In production, the
//! `HttpCallbackStorageAdapter` returns a real signed GCS URL of similar
//! length, so the LLM-bound shape stays identical across environments.

use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;

use crate::storage::domain::{
    OutputStorageRepository, StorageError, StoreRequest, StoredBytes, StoredOutput,
};

/// Default adapter when no external storage callback is configured.
///
/// `store()` returns a `StoredOutput` whose `read_url == storage_key`
/// (`local://<uuid>`). This is intentionally a short opaque handle, NOT a
/// `data:` URL, so that LLM tool-results stay small. Bytes are retained in
/// an in-process cache and retrieved via [`read`](Self::read).
pub struct LocalCacheStorageAdapter {
    cache: Arc<DashMap<String, StoredBytes>>,
}

impl LocalCacheStorageAdapter {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
        }
    }
}

impl Default for LocalCacheStorageAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OutputStorageRepository for LocalCacheStorageAdapter {
    async fn store(&self, req: StoreRequest) -> Result<StoredOutput, StorageError> {
        if req.bytes.is_empty() {
            return Err(StorageError::InvalidInput(
                "bytes must be non-empty".to_string(),
            ));
        }
        if req.mime_type.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "mime_type must be non-empty".to_string(),
            ));
        }

        let storage_key = format!("local://{}", uuid::Uuid::new_v4());
        let size_bytes = req.bytes.len() as u64;

        // Retain bytes in-process so subsequent read(storage_key) works.
        self.cache.insert(
            storage_key.clone(),
            StoredBytes {
                bytes: req.bytes,
                mime_type: req.mime_type.clone(),
                filename: req.filename.clone(),
            },
        );

        // read_url is intentionally the short storage_key, NOT a data: URL —
        // this prevents megabyte-sized base64 payloads from polluting the
        // LLM's tool-result context (rate-limit footgun in dev mode).
        Ok(StoredOutput {
            storage_key: storage_key.clone(),
            read_url: storage_key,
            mime_type: req.mime_type,
            filename: req.filename,
            size_bytes,
        })
    }

    async fn read(&self, storage_key: &str) -> Result<StoredBytes, StorageError> {
        self.cache
            .get(storage_key)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| {
                StorageError::InvalidInput(format!(
                    "storage_key '{storage_key}' not found in LocalCache (was it stored in this process?)"
                ))
            })
    }

    async fn read_stream(
        &self,
        _storage_key: &str,
    ) -> Result<crate::storage::domain::StoredStream, StorageError> {
        Err(StorageError::BackendUnavailable(
            "read_stream not yet implemented for LocalCacheStorageAdapter".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(bytes: Vec<u8>, mime: &str) -> StoreRequest {
        StoreRequest {
            bytes,
            mime_type: mime.to_string(),
            filename: "x.bin".to_string(),
            session_id: None,
            agent_session_id: None,
        }
    }

    #[tokio::test]
    async fn store_returns_short_handle_as_url_not_data_uri() {
        let adapter = LocalCacheStorageAdapter::new();
        let bytes = vec![1u8, 2, 3, 4, 5];
        let stored = adapter
            .store(req(bytes.clone(), "image/png"))
            .await
            .expect("store ok");

        assert!(stored.storage_key.starts_with("local://"));
        // read_url MUST be the short handle — never a data: URI — so the
        // LLM's tool-result stays small.
        assert_eq!(stored.read_url, stored.storage_key);
        assert!(!stored.read_url.starts_with("data:"));
        assert_eq!(stored.mime_type, "image/png");
        assert_eq!(stored.size_bytes, 5);

        // The bytes are recoverable via read() — that's how downstream
        // consumers (image_edit, load_attachment, http_post_image) get them.
        let got = adapter.read(&stored.storage_key).await.unwrap();
        assert_eq!(got.bytes, bytes);
    }

    #[tokio::test]
    async fn rejects_empty_bytes() {
        let adapter = LocalCacheStorageAdapter::new();
        let err = adapter.store(req(vec![], "image/png")).await.unwrap_err();
        assert!(matches!(err, StorageError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn rejects_empty_mime() {
        let adapter = LocalCacheStorageAdapter::new();
        let err = adapter.store(req(vec![1, 2], "  ")).await.unwrap_err();
        assert!(matches!(err, StorageError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn distinct_calls_yield_distinct_storage_keys() {
        let adapter = LocalCacheStorageAdapter::new();
        let a = adapter.store(req(vec![0xAB], "image/png")).await.unwrap();
        let b = adapter.store(req(vec![0xCD], "image/png")).await.unwrap();
        assert_ne!(a.storage_key, b.storage_key);
    }

    #[tokio::test]
    async fn read_returns_stored_bytes_for_known_key() {
        let adapter = LocalCacheStorageAdapter::new();
        let bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let stored = adapter
            .store(req(bytes.clone(), "image/png"))
            .await
            .unwrap();
        let got = adapter.read(&stored.storage_key).await.unwrap();
        assert_eq!(got.bytes, bytes);
        assert_eq!(got.mime_type, "image/png");
    }

    #[tokio::test]
    async fn read_unknown_key_errors() {
        let adapter = LocalCacheStorageAdapter::new();
        let err = adapter.read("local://does-not-exist").await.unwrap_err();
        assert!(matches!(err, StorageError::InvalidInput(_)));
    }
}
