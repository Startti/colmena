//! HTTP-callback storage adapter. Used in production by the ADP worker: asks
//! the API for a one-shot signed PUT URL, uploads the bytes to it, and returns
//! the resulting `storage_key` + `read_url`. Keeps colmena fully storage-blind
//! (no GCS credentials live here).
//!
//! The remote endpoint contract is:
//!
//! - Request: `POST {callback_url}` with header `X-Internal-Token: <secret>`
//!   and a JSON body of `{ session_id, agent_session_id, mime_type, filename, purpose }`.
//! - Response (200): `{ put_url, read_url, storage_key }`.
//!
//! Anything non-2xx is mapped to [`StorageError::CallbackFailed`]; transport
//! failures map to [`StorageError::BackendUnavailable`].

use async_trait::async_trait;
use dashmap::DashMap;
use serde::Deserialize;
use std::sync::Arc;

use crate::storage::domain::{
    OutputStorageRepository, StorageError, StoreRequest, StoredBytes, StoredOutput,
};

/// Cached metadata kept in-process to satisfy `read(storage_key)` without
/// requiring the API to maintain a separate sign-get endpoint. The first time
/// we store a blob we remember its (mime_type, filename, read_url) so that
/// later reads can fetch the bytes via GET on the read_url.
#[derive(Debug, Clone)]
struct CachedMeta {
    read_url: String,
    mime_type: String,
    filename: String,
}

pub struct HttpCallbackStorageAdapter {
    client: reqwest::Client,
    callback_url: String,
    shared_secret: String,
    /// In-process cache from `store()` to satisfy `read()` without requiring
    /// a separate sign-get endpoint from the API. Persists for the lifetime
    /// of the engine process. Acceptable because the read URL is what the
    /// API would re-sign anyway on demand.
    meta_cache: Arc<DashMap<String, CachedMeta>>,
}

impl HttpCallbackStorageAdapter {
    pub fn new(callback_url: String, shared_secret: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            callback_url,
            shared_secret,
            meta_cache: Arc::new(DashMap::new()),
        }
    }

    /// Test-only constructor that injects a pre-configured reqwest client (used
    /// to point the adapter at a `wiremock` server).
    #[cfg(test)]
    pub fn with_client(
        client: reqwest::Client,
        callback_url: String,
        shared_secret: String,
    ) -> Self {
        Self {
            client,
            callback_url,
            shared_secret,
            meta_cache: Arc::new(DashMap::new()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SignResponse {
    put_url: String,
    read_url: String,
    storage_key: String,
}

#[async_trait]
impl OutputStorageRepository for HttpCallbackStorageAdapter {
    async fn store(&self, req: StoreRequest) -> Result<StoredOutput, StorageError> {
        if req.bytes.is_empty() {
            return Err(StorageError::InvalidInput(
                "bytes must be non-empty".to_string(),
            ));
        }

        // 1) Ask the API for a signed PUT URL.
        let body = serde_json::json!({
            "session_id": req.session_id,
            "agent_session_id": req.agent_session_id,
            "mime_type": req.mime_type,
            "filename": req.filename,
            "purpose": "generated_output",
        });

        let resp = self
            .client
            .post(&self.callback_url)
            .header("X-Internal-Token", &self.shared_secret)
            .json(&body)
            .send()
            .await
            .map_err(|e| StorageError::BackendUnavailable(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(StorageError::CallbackFailed { status, body });
        }

        let sign: SignResponse = resp
            .json()
            .await
            .map_err(|e| StorageError::CallbackFailed {
                status: 0,
                body: format!("invalid sign response JSON: {}", e),
            })?;

        // 2) Upload bytes to the signed URL.
        let size_bytes = req.bytes.len() as u64;
        let put = self
            .client
            .put(&sign.put_url)
            .header("Content-Type", &req.mime_type)
            .body(req.bytes)
            .send()
            .await
            .map_err(|e| StorageError::UploadFailed(e.to_string()))?;

        if !put.status().is_success() {
            let status = put.status();
            let body = put.text().await.unwrap_or_default();
            return Err(StorageError::UploadFailed(format!(
                "PUT to signed URL failed: status={} body={}",
                status, body
            )));
        }

        // Remember (storage_key → read_url + metadata) so subsequent read()
        // calls can fetch bytes without round-tripping through a separate
        // sign-get endpoint.
        self.meta_cache.insert(
            sign.storage_key.clone(),
            CachedMeta {
                read_url: sign.read_url.clone(),
                mime_type: req.mime_type.clone(),
                filename: req.filename.clone(),
            },
        );

        Ok(StoredOutput {
            storage_key: sign.storage_key,
            read_url: sign.read_url,
            mime_type: req.mime_type,
            filename: req.filename,
            size_bytes,
        })
    }

    async fn read(&self, storage_key: &str) -> Result<StoredBytes, StorageError> {
        let meta = self
            .meta_cache
            .get(storage_key)
            .map(|e| e.value().clone())
            .ok_or_else(|| {
                StorageError::InvalidInput(format!(
                    "storage_key '{storage_key}' not found in HttpCallback meta cache. \
                     (Was it stored via this adapter in the current process? Cross-process \
                     reads require a server-side sign-get endpoint — not implemented yet.)"
                ))
            })?;

        let resp = self
            .client
            .get(&meta.read_url)
            .send()
            .await
            .map_err(|e| StorageError::BackendUnavailable(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(StorageError::UploadFailed(format!(
                "GET read_url for storage_key '{storage_key}' failed: status={status} body={body}"
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| StorageError::BackendUnavailable(e.to_string()))?
            .to_vec();

        Ok(StoredBytes {
            bytes,
            mime_type: meta.mime_type,
            filename: meta.filename,
        })
    }

    async fn read_stream(
        &self,
        storage_key: &str,
    ) -> Result<crate::storage::domain::StoredStream, StorageError> {
        use futures::StreamExt;

        let meta = self
            .meta_cache
            .get(storage_key)
            .map(|e| e.value().clone())
            .ok_or_else(|| {
                StorageError::InvalidInput(format!(
                    "storage_key '{storage_key}' not found in HttpCallback meta cache. \
                     (Was it stored via this adapter in the current process? Cross-process \
                     reads require a server-side sign-get endpoint — not implemented yet.)"
                ))
            })?;

        let resp = self
            .client
            .get(&meta.read_url)
            .send()
            .await
            .map_err(|e| StorageError::BackendUnavailable(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(StorageError::UploadFailed(format!(
                "GET read_url for storage_key '{storage_key}' failed: status={status} body={body}"
            )));
        }

        let size_bytes = resp.content_length().ok_or_else(|| {
            StorageError::BackendUnavailable(format!(
                "GET read_url for storage_key '{storage_key}' returned no Content-Length"
            ))
        })?;

        let stream = resp
            .bytes_stream()
            .map(|chunk| chunk.map_err(|e| StorageError::BackendUnavailable(e.to_string())));

        Ok(crate::storage::domain::StoredStream {
            stream: Box::pin(stream),
            size_bytes,
            mime_type: meta.mime_type,
            filename: meta.filename,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn req(bytes: Vec<u8>, mime: &str) -> StoreRequest {
        StoreRequest {
            bytes,
            mime_type: mime.to_string(),
            filename: "out.png".to_string(),
            session_id: Some("ses_1".into()),
            agent_session_id: Some("agent_1".into()),
        }
    }

    #[tokio::test]
    async fn happy_path_calls_callback_then_puts_and_returns_stored_output() {
        let server = MockServer::start().await;

        // PUT mock — return 200 for any PUT under /upload
        Mock::given(method("PUT"))
            .and(path("/upload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let put_url = format!("{}/upload", server.uri());
        let read_url = "https://example.com/signed-read";
        let storage_key = "chat-attachments/u/s/generated/abc-out.png";

        // Callback mock — assert shared secret header is forwarded
        Mock::given(method("POST"))
            .and(path("/sign-put"))
            .and(header("X-Internal-Token", "s3cr3t"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "put_url": put_url,
                "read_url": read_url,
                "storage_key": storage_key,
            })))
            .mount(&server)
            .await;

        let adapter = HttpCallbackStorageAdapter::new(
            format!("{}/sign-put", server.uri()),
            "s3cr3t".to_string(),
        );

        let stored = adapter
            .store(req(vec![0xDE, 0xAD, 0xBE, 0xEF], "image/png"))
            .await
            .expect("store ok");

        assert_eq!(stored.storage_key, storage_key);
        assert_eq!(stored.read_url, read_url);
        assert_eq!(stored.mime_type, "image/png");
        assert_eq!(stored.size_bytes, 4);
    }

    #[tokio::test]
    async fn callback_401_maps_to_callback_failed() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/sign-put"))
            .respond_with(ResponseTemplate::new(401).set_body_string("nope"))
            .mount(&server)
            .await;

        let adapter = HttpCallbackStorageAdapter::new(
            format!("{}/sign-put", server.uri()),
            "wrong".to_string(),
        );

        let err = adapter
            .store(req(vec![1, 2, 3], "image/png"))
            .await
            .unwrap_err();

        match err {
            StorageError::CallbackFailed { status, body } => {
                assert_eq!(status, 401);
                assert_eq!(body, "nope");
            }
            other => panic!("expected CallbackFailed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn put_500_maps_to_upload_failed() {
        let server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/upload"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let put_url = format!("{}/upload", server.uri());
        Mock::given(method("POST"))
            .and(path("/sign-put"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "put_url": put_url,
                "read_url": "https://example.com/r",
                "storage_key": "k",
            })))
            .mount(&server)
            .await;

        let adapter = HttpCallbackStorageAdapter::new(
            format!("{}/sign-put", server.uri()),
            "s3cr3t".to_string(),
        );

        let err = adapter
            .store(req(vec![1, 2, 3], "image/png"))
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::UploadFailed(_)));
    }

    #[tokio::test]
    async fn empty_bytes_is_invalid_input_without_network_call() {
        // No mock server needed — the adapter must short-circuit before calling out.
        let adapter = HttpCallbackStorageAdapter::new(
            "http://does-not-exist.invalid/sign-put".to_string(),
            "s3cr3t".to_string(),
        );
        let err = adapter.store(req(vec![], "image/png")).await.unwrap_err();
        assert!(matches!(err, StorageError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn read_after_store_returns_bytes_via_cached_read_url() {
        let server = MockServer::start().await;

        // Mock the PUT (write) — accept anything
        Mock::given(method("PUT"))
            .and(path("/upload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let put_url = format!("{}/upload", server.uri());
        let read_url = format!("{}/read", server.uri());
        let storage_key = "chat-attachments/u/s/generated/abc-out.png";

        // Mock the sign callback
        Mock::given(method("POST"))
            .and(path("/sign-put"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "put_url": put_url,
                "read_url": read_url,
                "storage_key": storage_key,
            })))
            .mount(&server)
            .await;

        // Mock the GET (read) — return the bytes we expect
        Mock::given(method("GET"))
            .and(path("/read"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]))
            .mount(&server)
            .await;

        let adapter = HttpCallbackStorageAdapter::new(
            format!("{}/sign-put", server.uri()),
            "s3cr3t".to_string(),
        );

        // Store first
        let stored = adapter
            .store(req(vec![0xDE, 0xAD, 0xBE, 0xEF], "image/png"))
            .await
            .expect("store ok");

        // Now read using the storage_key
        let got = adapter
            .read(&stored.storage_key)
            .await
            .expect("read ok after store");
        assert_eq!(got.bytes, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(got.mime_type, "image/png");
    }

    #[tokio::test]
    async fn read_unknown_key_errors_without_network_call() {
        let adapter = HttpCallbackStorageAdapter::new(
            "http://does-not-exist.invalid/sign-put".to_string(),
            "s3cr3t".to_string(),
        );
        let err = adapter.read("never-stored").await.unwrap_err();
        assert!(matches!(err, StorageError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn read_stream_after_store_streams_bytes_via_cached_read_url() {
        use futures::StreamExt;

        let server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/upload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let put_url = format!("{}/upload", server.uri());
        let read_url = format!("{}/read", server.uri());
        let storage_key = "chat-attachments/u/s/generated/abc-out.png";

        Mock::given(method("POST"))
            .and(path("/sign-put"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "put_url": put_url,
                "read_url": read_url,
                "storage_key": storage_key,
            })))
            .mount(&server)
            .await;

        // GET returns a 4-byte payload; Content-Length header is set by wiremock automatically
        Mock::given(method("GET"))
            .and(path("/read"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]))
            .mount(&server)
            .await;

        let adapter = HttpCallbackStorageAdapter::new(
            format!("{}/sign-put", server.uri()),
            "s3cr3t".to_string(),
        );
        let stored = adapter
            .store(req(vec![0xDE, 0xAD, 0xBE, 0xEF], "image/png"))
            .await
            .unwrap();

        let mut s = adapter.read_stream(&stored.storage_key).await.unwrap();
        assert_eq!(s.size_bytes, 4);
        assert_eq!(s.mime_type, "image/png");
        assert_eq!(s.filename, "out.png");

        let mut collected = Vec::new();
        while let Some(chunk) = s.stream.next().await {
            collected.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(collected, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[tokio::test]
    async fn read_stream_unknown_key_errors_without_network_call() {
        let adapter = HttpCallbackStorageAdapter::new(
            "http://does-not-exist.invalid/sign-put".to_string(),
            "s3cr3t".to_string(),
        );
        let err = adapter.read_stream("never-stored").await.unwrap_err();
        assert!(matches!(err, StorageError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn read_stream_get_500_maps_to_upload_failed() {
        let server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/upload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let put_url = format!("{}/upload", server.uri());
        let read_url = format!("{}/read", server.uri());
        Mock::given(method("POST"))
            .and(path("/sign-put"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "put_url": put_url,
                "read_url": read_url,
                "storage_key": "k",
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/read"))
            .respond_with(ResponseTemplate::new(500).set_body_string("oops"))
            .mount(&server)
            .await;

        let adapter = HttpCallbackStorageAdapter::new(
            format!("{}/sign-put", server.uri()),
            "s3cr3t".to_string(),
        );
        let stored = adapter.store(req(vec![1, 2], "image/png")).await.unwrap();
        let err = adapter.read_stream(&stored.storage_key).await.unwrap_err();
        assert!(matches!(err, StorageError::UploadFailed(_)));
    }
}
