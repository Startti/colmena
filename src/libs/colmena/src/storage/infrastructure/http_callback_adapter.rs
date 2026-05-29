//! HTTP-callback storage adapter. Used in production by the ADP worker: asks
//! the API for a one-shot signed PUT URL, uploads the bytes to it, and returns
//! the resulting `storage_key` + `read_url`. Keeps colmena fully storage-blind
//! (no GCS credentials live here).
//!
//! The remote endpoint contract is:
//!
//! - Sign / upload: `POST {callback_url}` with header `X-Internal-Token: <secret>`
//!   and a JSON body of `{ session_id, agent_session_id, mime_type, filename, purpose }`.
//!   Response (200): `{ put_url, read_url, storage_key }`.
//! - Delete (Plan C): `POST {<sibling-of-callback_url>/delete}` with the same
//!   `X-Internal-Token` header and body `{ storage_key }`. Idempotent — 404
//!   is treated as success. Used by the `attachment_gc` binary.
//! - Sign-get (cross-process read): `POST {<sibling-of-callback_url>/sign-get}`
//!   with the same `X-Internal-Token` header and body `{ storage_key }`.
//!   Response (200): `{ read_url }`. Called on a `meta_cache` miss so a
//!   `storage_key` produced by another process (a prior turn's worker, or an
//!   upload ADP minted directly) can still be read. 404 → `InvalidInput`
//!   (treated as not-found by `AttachmentStreamResolver`).
//!
//! Sign-step non-2xx is mapped to [`StorageError::CallbackFailed`]; transport
//! failures map to [`StorageError::BackendUnavailable`]. The delete step maps
//! any non-success non-404 status to `BackendUnavailable` so the gc binary
//! retries on the next scheduled run.

use async_trait::async_trait;
use dashmap::DashMap;
use serde::Deserialize;
use std::sync::Arc;

use crate::storage::domain::{
    OutputStorageRepository, StorageError, StoreRequest, StoredBytes, StoredOutput,
};

/// Cached metadata kept in-process as the fast path for `read(storage_key)`.
/// The first time we store a blob we remember its (mime_type, filename,
/// read_url) so later reads in the same process fetch bytes via GET without a
/// sign-get round-trip. On a miss the adapter falls back to sign-get.
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
    /// In-process fast-path cache from `store()` to satisfy `read()` without a
    /// network round-trip. Persists for the lifetime of the engine process.
    /// On a miss, `read()`/`read_stream()` fall back to the cross-process
    /// sign-get endpoint (see [`HttpCallbackStorageAdapter::sign_get_read_url`]),
    /// so keys produced by other processes are still readable.
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

    /// Derive the delete-endpoint URL by sibling-path replacement on
    /// `callback_url`. The production ADP API exposes `POST /internal/gcs/sign-put`
    /// for upload and (after Plan C) `POST /internal/gcs/delete` for deletion;
    /// they share the same `/internal/gcs/` prefix and `X-Internal-Token`
    /// authentication. Replacing the final path segment lets the adapter
    /// derive both endpoints from a single `COLMENA_STORAGE_CALLBACK_URL`
    /// env var without forcing every deployment to add a second variable.
    ///
    /// If `callback_url` does not end with `/sign-put`, we fall back to
    /// appending `/delete` so the call still reaches a sensible default
    /// path — but production deployments should always use the canonical
    /// `.../sign-put` URL.
    fn delete_url(&self) -> String {
        if let Some(prefix) = self.callback_url.strip_suffix("/sign-put") {
            format!("{prefix}/delete")
        } else if let Some(prefix) = self.callback_url.strip_suffix('/') {
            format!("{prefix}/delete")
        } else {
            format!("{}/delete", self.callback_url)
        }
    }

    /// Derive the sign-get endpoint URL by sibling-path replacement on
    /// `callback_url` (same pattern as [`Self::delete_url`]). ADP exposes
    /// `POST /internal/gcs/sign-get` alongside `/sign-put`; it returns a
    /// freshly-signed read URL for any `storage_key` ADP owns.
    fn sign_get_url(&self) -> String {
        if let Some(prefix) = self.callback_url.strip_suffix("/sign-put") {
            format!("{prefix}/sign-get")
        } else if let Some(prefix) = self.callback_url.strip_suffix('/') {
            format!("{prefix}/sign-get")
        } else {
            format!("{}/sign-get", self.callback_url)
        }
    }

    /// Cross-process fallback: POST `{ storage_key }` to the sign-get endpoint
    /// and return the freshly-signed read URL. Called when `meta_cache` does
    /// not have the key (it was stored by a different process). 404 →
    /// `InvalidInput` (so `AttachmentStreamResolver` treats it as not-found);
    /// other non-2xx → `CallbackFailed`; transport error → `BackendUnavailable`.
    async fn sign_get_read_url(&self, storage_key: &str) -> Result<String, StorageError> {
        let url = self.sign_get_url();
        let resp = self
            .client
            .post(&url)
            .header("X-Internal-Token", &self.shared_secret)
            .json(&serde_json::json!({ "storage_key": storage_key }))
            .send()
            .await
            .map_err(|e| StorageError::BackendUnavailable(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(StorageError::InvalidInput(format!(
                "storage_key '{storage_key}' not found via sign-get (404)"
            )));
        }
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(StorageError::CallbackFailed { status, body });
        }

        #[derive(Deserialize)]
        struct SignGetResponse {
            read_url: String,
        }
        let sg: SignGetResponse = resp
            .json()
            .await
            .map_err(|e| StorageError::CallbackFailed {
                status: 0,
                body: format!("invalid sign-get response JSON: {e}"),
            })?;
        Ok(sg.read_url)
    }

    /// Resolve `storage_key` to `(read_url, mime_hint, filename_hint)`.
    /// Fast path: the in-process `meta_cache`. Miss → cross-process sign-get
    /// (hints `None`; the caller derives mime from the GET response's
    /// `Content-Type` and filename from the key).
    async fn resolve_read_target(
        &self,
        storage_key: &str,
    ) -> Result<(String, Option<String>, Option<String>), StorageError> {
        if let Some(meta) = self.meta_cache.get(storage_key).map(|e| e.value().clone()) {
            return Ok((meta.read_url, Some(meta.mime_type), Some(meta.filename)));
        }
        let read_url = self.sign_get_read_url(storage_key).await?;
        Ok((read_url, None, None))
    }
}

#[derive(Debug, Deserialize)]
struct SignResponse {
    put_url: String,
    read_url: String,
    storage_key: String,
}

/// Extract a clean MIME type from a response's `Content-Type` header, falling
/// back to `application/octet-stream`. Used on the cross-process read path
/// where the original `mime_type` was not cached in this process.
fn content_type_or_default(resp: &reqwest::Response) -> String {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

/// Derive a best-effort filename from a `storage_key` (its last path segment).
fn filename_from_key(storage_key: &str) -> String {
    storage_key
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("attachment")
        .to_string()
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
        // Fast path: in-process meta_cache. Miss → cross-process sign-get.
        let (read_url, mime_hint, filename_hint) = self.resolve_read_target(storage_key).await?;

        let resp = self
            .client
            .get(&read_url)
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

        // On the cross-process path we have no cached mime/filename — derive
        // them from the response and the key.
        let mime_type = mime_hint.unwrap_or_else(|| content_type_or_default(&resp));
        let filename = filename_hint.unwrap_or_else(|| filename_from_key(storage_key));

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| StorageError::BackendUnavailable(e.to_string()))?
            .to_vec();

        Ok(StoredBytes {
            bytes,
            mime_type,
            filename,
        })
    }

    async fn read_stream(
        &self,
        storage_key: &str,
    ) -> Result<crate::storage::domain::StoredStream, StorageError> {
        use futures::StreamExt;

        // Fast path: in-process meta_cache. Miss → cross-process sign-get.
        let (read_url, mime_hint, filename_hint) = self.resolve_read_target(storage_key).await?;

        let resp = self
            .client
            .get(&read_url)
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

        let mime_type = mime_hint.unwrap_or_else(|| content_type_or_default(&resp));
        let filename = filename_hint.unwrap_or_else(|| filename_from_key(storage_key));

        let stream = resp
            .bytes_stream()
            .map(|chunk| chunk.map_err(|e| StorageError::BackendUnavailable(e.to_string())));

        Ok(crate::storage::domain::StoredStream {
            stream: Box::pin(stream),
            size_bytes,
            mime_type,
            filename,
        })
    }

    async fn delete(&self, storage_key: &str) -> Result<(), StorageError> {
        // POST { storage_key } to the sibling /delete endpoint with the same
        // shared-secret header used by /sign-put. The host application is
        // responsible for deleting the underlying GCS object.
        let url = self.delete_url();
        let resp = self
            .client
            .post(&url)
            .header("X-Internal-Token", &self.shared_secret)
            .json(&serde_json::json!({ "storage_key": storage_key }))
            .send()
            .await
            .map_err(|e| {
                StorageError::BackendUnavailable(format!("delete callback transport: {e}"))
            })?;

        let status = resp.status();
        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
            // Drop the local meta cache entry so a subsequent `read` after
            // delete fails fast without a doomed network round-trip.
            self.meta_cache.remove(storage_key);
            // 404 is treated as success — the blob is already gone, which is
            // exactly the post-condition we want (idempotent delete).
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(StorageError::BackendUnavailable(format!(
                "delete callback returned {status}: {body}"
            )))
        }
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
    async fn read_cache_miss_attempts_sign_get_then_fails_when_unreachable() {
        // Key was never stored in THIS process → meta_cache miss → adapter
        // tries the cross-process sign-get endpoint, which is unreachable here.
        let adapter = HttpCallbackStorageAdapter::new(
            "http://does-not-exist.invalid/sign-put".to_string(),
            "s3cr3t".to_string(),
        );
        let err = adapter.read("never-stored").await.unwrap_err();
        assert!(
            matches!(err, StorageError::BackendUnavailable(_)),
            "expected BackendUnavailable from unreachable sign-get, got {err:?}"
        );
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
    async fn read_stream_cache_miss_attempts_sign_get_then_fails_when_unreachable() {
        let adapter = HttpCallbackStorageAdapter::new(
            "http://does-not-exist.invalid/sign-put".to_string(),
            "s3cr3t".to_string(),
        );
        let err = adapter.read_stream("never-stored").await.unwrap_err();
        assert!(
            matches!(err, StorageError::BackendUnavailable(_)),
            "expected BackendUnavailable from unreachable sign-get, got {err:?}"
        );
    }

    #[tokio::test]
    async fn sign_get_url_derived_from_sign_put_sibling() {
        let adapter = HttpCallbackStorageAdapter::new(
            "https://api.dev.startti.ai/internal/gcs/sign-put".to_string(),
            "s3cr3t".to_string(),
        );
        assert_eq!(
            adapter.sign_get_url(),
            "https://api.dev.startti.ai/internal/gcs/sign-get"
        );
    }

    #[tokio::test]
    async fn read_stream_cross_process_resolves_via_sign_get() {
        use futures::StreamExt;

        // Fresh adapter — nothing in meta_cache. The key was "produced by
        // another process", so reading it MUST go through sign-get.
        let server = MockServer::start().await;
        let read_url = format!("{}/read", server.uri());
        let storage_key = "chat-attachments/u/s/uploads/prior-turn.png";

        // sign-get returns a freshly-signed read URL.
        Mock::given(method("POST"))
            .and(path("/sign-get"))
            .and(header("X-Internal-Token", "s3cr3t"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "read_url": read_url,
            })))
            .mount(&server)
            .await;

        // GET returns bytes + a Content-Type the adapter must surface as mime.
        Mock::given(method("GET"))
            .and(path("/read"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(vec![0x89u8, 0x50, 0x4e, 0x47])
                    .insert_header("content-type", "image/png"),
            )
            .mount(&server)
            .await;

        let adapter = HttpCallbackStorageAdapter::new(
            format!("{}/sign-put", server.uri()),
            "s3cr3t".to_string(),
        );

        let mut s = adapter
            .read_stream(storage_key)
            .await
            .expect("read_stream ok");
        assert_eq!(s.size_bytes, 4);
        assert_eq!(s.mime_type, "image/png", "mime derived from Content-Type");
        assert_eq!(
            s.filename, "prior-turn.png",
            "filename derived from storage_key"
        );
        let mut collected = Vec::new();
        while let Some(chunk) = s.stream.next().await {
            collected.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(collected, vec![0x89, 0x50, 0x4e, 0x47]);
    }

    #[tokio::test]
    async fn sign_get_404_maps_to_invalid_input() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sign-get"))
            .respond_with(ResponseTemplate::new(404).set_body_string("no such object"))
            .mount(&server)
            .await;

        let adapter = HttpCallbackStorageAdapter::new(
            format!("{}/sign-put", server.uri()),
            "s3cr3t".to_string(),
        );

        let err = adapter.read_stream("missing-key").await.unwrap_err();
        assert!(
            matches!(err, StorageError::InvalidInput(_)),
            "sign-get 404 should map to InvalidInput (not-found), got {err:?}"
        );
    }

    #[tokio::test]
    async fn delete_url_derived_from_sign_put_sibling() {
        let adapter = HttpCallbackStorageAdapter::new(
            "https://api.dev.startti.ai/internal/gcs/sign-put".to_string(),
            "s3cr3t".to_string(),
        );
        assert_eq!(
            adapter.delete_url(),
            "https://api.dev.startti.ai/internal/gcs/delete"
        );
    }

    #[tokio::test]
    async fn delete_url_falls_back_when_callback_url_lacks_sign_put_suffix() {
        let adapter = HttpCallbackStorageAdapter::new(
            "https://api.example.com/custom-endpoint".to_string(),
            "s3cr3t".to_string(),
        );
        // No /sign-put suffix and no trailing slash → append /delete.
        assert_eq!(
            adapter.delete_url(),
            "https://api.example.com/custom-endpoint/delete"
        );
    }

    #[tokio::test]
    async fn delete_posts_storage_key_to_sibling_endpoint_with_secret_header() {
        let server = MockServer::start().await;

        // The adapter must POST to the sibling /delete endpoint with the
        // shared-secret header and a JSON body of { storage_key }.
        Mock::given(method("POST"))
            .and(path("/internal/gcs/delete"))
            .and(header("X-Internal-Token", "s3cr3t"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let adapter = HttpCallbackStorageAdapter::new(
            format!("{}/internal/gcs/sign-put", server.uri()),
            "s3cr3t".to_string(),
        );

        adapter.delete("some-key").await.unwrap();
        // wiremock auto-verifies the expected call count on drop.
    }

    #[tokio::test]
    async fn delete_treats_404_as_success() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/internal/gcs/delete"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let adapter = HttpCallbackStorageAdapter::new(
            format!("{}/internal/gcs/sign-put", server.uri()),
            "s3cr3t".to_string(),
        );
        // 404 means "already gone" — idempotent delete.
        adapter.delete("missing-key").await.unwrap();
    }

    #[tokio::test]
    async fn delete_returns_backend_unavailable_on_5xx() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/internal/gcs/delete"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream down"))
            .mount(&server)
            .await;

        let adapter = HttpCallbackStorageAdapter::new(
            format!("{}/internal/gcs/sign-put", server.uri()),
            "s3cr3t".to_string(),
        );
        let err = adapter.delete("any-key").await.unwrap_err();
        match err {
            StorageError::BackendUnavailable(msg) => {
                assert!(msg.contains("503"), "expected 503 in error, got: {msg}");
            }
            other => panic!("expected BackendUnavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_evicts_meta_cache_so_subsequent_read_falls_back_to_sign_get() {
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
            .and(path("/internal/gcs/sign-put"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "put_url": put_url,
                "read_url": read_url,
                "storage_key": storage_key,
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/internal/gcs/delete"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        // After deletion the blob is gone, so the cross-process sign-get that a
        // post-eviction read falls back to correctly returns 404.
        Mock::given(method("POST"))
            .and(path("/internal/gcs/sign-get"))
            .respond_with(ResponseTemplate::new(404).set_body_string("object deleted"))
            .mount(&server)
            .await;

        let adapter = HttpCallbackStorageAdapter::new(
            format!("{}/internal/gcs/sign-put", server.uri()),
            "s3cr3t".to_string(),
        );

        // Store → cache populated.
        let stored = adapter
            .store(req(vec![0xDE, 0xAD], "image/png"))
            .await
            .unwrap();
        // Delete → cache evicted.
        adapter.delete(&stored.storage_key).await.unwrap();
        // Subsequent read misses the cache, falls back to sign-get, which 404s
        // (blob deleted) → InvalidInput (treated as not-found).
        let err = adapter.read(&stored.storage_key).await.unwrap_err();
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
