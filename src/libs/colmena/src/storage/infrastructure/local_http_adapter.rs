//! Dev-mode storage adapter that writes bytes to disk and serves them via
//! an embedded `axum` server on `127.0.0.1:<port>`. Selected by
//! `EngineConfig::from_env` when `COLMENA_LOCAL_STORAGE_DIR` is set.
//!
//! ## Why this exists
//!
//! In production, `HttpCallbackStorageAdapter` returns a signed GCS URL —
//! a real HTTP URL that any client (browser, reqwest, the LLM provider's
//! Files API) can fetch. This adapter mirrors that semantics in dev: the
//! returned `read_url` is an `http://127.0.0.1:<port>/files/<key>` URL,
//! NOT a `data:` URI and NOT an opaque `local://` handle. The bytes also
//! land on disk under `<dir>/<key>`, so the user can `open <path>` or
//! paste the URL in their browser.
//!
//! ## URL semantics
//!
//! - `storage_key`: bare filename (`<uuid>.<ext>`). Stable handle used by
//!   `$attachment:<key>` placeholders and the `OutputStorageRepository.read`
//!   path.
//! - `read_url`: `http://127.0.0.1:<port>/files/<key>` — fetchable by any
//!   HTTP client, opens in the browser, and (most importantly) keeps the
//!   gen → load_attachment → cross-provider-upload pipeline working without
//!   special-casing because the LLM provider's Files API just does a GET.
//!
//! ## Lifecycle
//!
//! The server runs in a Tokio task spawned at construction. A `oneshot`
//! channel triggers graceful shutdown on `Drop` (or explicitly when the
//! engine shuts down). Files persist on disk after the process exits, so
//! you can inspect them after the run — but the URLs stop responding.

use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tokio::sync::oneshot;

use crate::storage::domain::{
    OutputStorageRepository, StorageError, StoreRequest, StoredBytes, StoredOutput,
};

/// Best-guess content-type from a file extension. Conservative — anything
/// unknown defaults to `application/octet-stream`. The set covers the
/// formats the media nodes can produce (PNG/JPEG/WEBP images, MP3/WAV/OGG
/// audio).
fn mime_from_filename(filename: &str) -> &'static str {
    let lower = filename.to_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".mp3") {
        "audio/mpeg"
    } else if lower.ends_with(".wav") {
        "audio/wav"
    } else if lower.ends_with(".opus") || lower.ends_with(".ogg") {
        "audio/ogg"
    } else if lower.ends_with(".pcm") {
        "audio/L16"
    } else {
        "application/octet-stream"
    }
}

/// Best-guess extension from a mime type, used when synthesizing filenames
/// for the on-disk artifact.
fn ext_from_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "audio/mpeg" => "mp3",
        "audio/wav" => "wav",
        "audio/ogg" => "ogg",
        "audio/L16" => "pcm",
        _ => "bin",
    }
}

#[derive(Clone)]
struct ServerState {
    dir: PathBuf,
}

async fn serve_file(State(state): State<ServerState>, AxumPath(key): AxumPath<String>) -> Response {
    // Guard against path traversal — disallow any slashes or `..` in the key.
    if key.contains('/') || key.contains("..") || key.is_empty() {
        return (StatusCode::BAD_REQUEST, "invalid storage_key").into_response();
    }
    let path = state.dir.join(&key);
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mime = mime_from_filename(&key);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::from(bytes))
                .unwrap()
        }
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

pub struct LocalHttpStorageAdapter {
    dir: PathBuf,
    port: u16,
    /// `Some(tx)` while the server is up; `None` after shutdown. Wrapped in
    /// a `Mutex` so `Drop` can `take` the sender without an `&mut self`.
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
}

impl LocalHttpStorageAdapter {
    /// Construct, create the directory if missing, bind the server. Returns
    /// the actual bound port (useful when `port == 0` was requested and the
    /// OS picked one).
    pub async fn new(dir: PathBuf, port: u16) -> Result<Self, StorageError> {
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| StorageError::BackendUnavailable(format!("mkdir {dir:?}: {e}")))?;

        let state = ServerState { dir: dir.clone() };
        let app = Router::new()
            .route("/files/:key", get(serve_file))
            .with_state(state);

        let bind_addr = format!("127.0.0.1:{}", port);
        let listener = tokio::net::TcpListener::bind(&bind_addr)
            .await
            .map_err(|e| StorageError::BackendUnavailable(format!("bind {bind_addr}: {e}")))?;
        let bound_port = listener
            .local_addr()
            .map_err(|e| StorageError::BackendUnavailable(format!("local_addr: {e}")))?
            .port();

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        tracing::info!(
            target: "colmena::storage",
            dir = %dir.display(),
            port = bound_port,
            "LocalHttpStorageAdapter listening at http://127.0.0.1:{}/files/<key>",
            bound_port
        );

        Ok(Self {
            dir,
            port: bound_port,
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
        })
    }

    /// Return the port the server is actually bound to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Return the on-disk directory used for storage.
    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }
}

impl Drop for LocalHttpStorageAdapter {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.shutdown_tx.lock() {
            if let Some(tx) = guard.take() {
                let _ = tx.send(());
            }
        }
    }
}

#[async_trait]
impl OutputStorageRepository for LocalHttpStorageAdapter {
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

        // Synthesize a storage_key from a UUID + extension derived from mime.
        // Using the mime-driven extension means `serve_file` can echo the
        // right Content-Type without any side metadata storage.
        let ext = ext_from_mime(&req.mime_type);
        let storage_key = format!("{}.{}", uuid::Uuid::new_v4(), ext);

        let path = self.dir.join(&storage_key);
        let size_bytes = req.bytes.len() as u64;
        tokio::fs::write(&path, &req.bytes)
            .await
            .map_err(|e| StorageError::UploadFailed(format!("write {}: {}", path.display(), e)))?;

        let read_url = format!("http://127.0.0.1:{}/files/{}", self.port, storage_key);

        Ok(StoredOutput {
            storage_key,
            read_url,
            mime_type: req.mime_type,
            filename: req.filename,
            size_bytes,
        })
    }

    async fn read(&self, storage_key: &str) -> Result<StoredBytes, StorageError> {
        if storage_key.contains('/') || storage_key.contains("..") || storage_key.is_empty() {
            return Err(StorageError::InvalidInput(format!(
                "invalid storage_key '{storage_key}'"
            )));
        }
        let path = self.dir.join(storage_key);
        let bytes = tokio::fs::read(&path).await.map_err(|e| {
            StorageError::InvalidInput(format!(
                "storage_key '{storage_key}' not readable at {}: {e}",
                path.display()
            ))
        })?;
        Ok(StoredBytes {
            bytes,
            mime_type: mime_from_filename(storage_key).to_string(),
            filename: storage_key.to_string(),
        })
    }

    async fn read_stream(
        &self,
        _storage_key: &str,
    ) -> Result<crate::storage::domain::StoredStream, StorageError> {
        Err(StorageError::BackendUnavailable(
            "read_stream not yet implemented for LocalHttpStorageAdapter".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn req(bytes: Vec<u8>, mime: &str) -> StoreRequest {
        StoreRequest {
            bytes,
            mime_type: mime.to_string(),
            filename: "out".to_string(),
            session_id: None,
            agent_session_id: None,
        }
    }

    #[tokio::test]
    async fn store_writes_to_disk_and_returns_http_url() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalHttpStorageAdapter::new(tmp.path().to_path_buf(), 0)
            .await
            .unwrap();

        let bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let stored = adapter
            .store(req(bytes.clone(), "image/png"))
            .await
            .unwrap();

        assert!(stored.storage_key.ends_with(".png"));
        assert!(stored
            .read_url
            .starts_with(&format!("http://127.0.0.1:{}/files/", adapter.port())));
        assert!(stored.read_url.ends_with(&stored.storage_key));

        // Bytes must be on disk at <dir>/<storage_key>
        let on_disk = tokio::fs::read(tmp.path().join(&stored.storage_key))
            .await
            .unwrap();
        assert_eq!(on_disk, bytes);
    }

    #[tokio::test]
    async fn read_returns_disk_bytes_for_known_key() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalHttpStorageAdapter::new(tmp.path().to_path_buf(), 0)
            .await
            .unwrap();

        let bytes = vec![1u8, 2, 3];
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
        let tmp = TempDir::new().unwrap();
        let adapter = LocalHttpStorageAdapter::new(tmp.path().to_path_buf(), 0)
            .await
            .unwrap();
        let err = adapter.read("does-not-exist.png").await.unwrap_err();
        assert!(matches!(err, StorageError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn read_rejects_path_traversal() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalHttpStorageAdapter::new(tmp.path().to_path_buf(), 0)
            .await
            .unwrap();
        for bad in ["../etc/passwd", "/tmp/foo", "sub/dir/key"] {
            let err = adapter.read(bad).await.unwrap_err();
            assert!(
                matches!(err, StorageError::InvalidInput(_)),
                "expected InvalidInput for '{bad}', got {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn http_server_serves_stored_bytes() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalHttpStorageAdapter::new(tmp.path().to_path_buf(), 0)
            .await
            .unwrap();

        let bytes = vec![0x89u8, 0x50, 0x4e, 0x47]; // PNG magic bytes (short, fake)
        let stored = adapter
            .store(req(bytes.clone(), "image/png"))
            .await
            .unwrap();

        // Fetch via the URL exactly like a browser / LLM Files API would.
        let resp = reqwest::get(&stored.read_url).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .map(|h| h.to_str().unwrap()),
            Some("image/png")
        );
        let body = resp.bytes().await.unwrap();
        assert_eq!(body.as_ref(), &bytes[..]);
    }

    #[tokio::test]
    async fn http_server_returns_404_for_unknown_key() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalHttpStorageAdapter::new(tmp.path().to_path_buf(), 0)
            .await
            .unwrap();
        let url = format!("http://127.0.0.1:{}/files/nope.png", adapter.port());
        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn empty_bytes_rejected() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalHttpStorageAdapter::new(tmp.path().to_path_buf(), 0)
            .await
            .unwrap();
        let err = adapter.store(req(vec![], "image/png")).await.unwrap_err();
        assert!(matches!(err, StorageError::InvalidInput(_)));
    }
}
