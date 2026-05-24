# HTTP Multipart Streaming Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add multipart/form-data support to the `http_request` node, sourcing parts from either signed URLs or `$attachment:<storage_key>` references, with full streaming (no in-memory buffering) so concurrent uploads scale on Cloud Run without OOMs.

**Architecture:** The node detects multipart mode by `Content-Type: multipart/*` header. Parts come from three value shapes inside `body`: bare strings (URL / `$attachment:` / text), arrays (expand to N parts under the same field name), or explicit objects (override filename/content_type). URLs are HEAD-validated then streamed in via `bytes_stream()`. Attachments stream through a new `OutputStorageRepository::read_stream` method. The downstream multipart request is sent in chunks — no full payload sits in worker RAM.

**Tech Stack:** Rust, `reqwest` (with `multipart` + `stream` features, already enabled), `bytes`, `futures` for stream composition, `async_trait`, `wiremock` for integration tests, `mockall` for unit mocks. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-24-http-multipart-streaming-design.md`

---

## File Structure

**Modify:**
- `src/libs/colmena/src/storage/domain/output_storage_repository.rs` — add `StoredStream` + `read_stream` trait method
- `src/libs/colmena/src/storage/domain/mod.rs` — re-export `StoredStream` if needed
- `src/libs/colmena/src/storage/infrastructure/local_cache_adapter.rs` — `read_stream` impl
- `src/libs/colmena/src/storage/infrastructure/local_http_adapter.rs` — `read_stream` impl
- `src/libs/colmena/src/storage/infrastructure/http_callback_adapter.rs` — `read_stream` impl (true streaming via `bytes_stream()`)
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs` — multipart branch + body parser
- `docs/node_configurations.json` — update `http_request` entry
- `docs/developer_guide/25_web_nodes.md` — new "Multipart uploads" section
- `docs/agent_context/node_ports_reference.md` — note multipart behavior

**Create:**
- `tests/multipart_http_test.rs` — wiremock end-to-end integration test
- `tests/graphs/external/multipart_upload.json` — CLI-runnable smoke graph

---

## Task 1: Extend `OutputStorageRepository` with `read_stream` (stub all 3 adapters)

**Goal:** Land the new trait surface and stub impls so the codebase compiles, before any real streaming logic.

**Files:**
- Modify: `src/libs/colmena/src/storage/domain/output_storage_repository.rs`
- Modify: `src/libs/colmena/src/storage/domain/mod.rs`
- Modify: `src/libs/colmena/src/storage/infrastructure/local_cache_adapter.rs`
- Modify: `src/libs/colmena/src/storage/infrastructure/local_http_adapter.rs`
- Modify: `src/libs/colmena/src/storage/infrastructure/http_callback_adapter.rs`

- [ ] **Step 1: Define `StoredStream` and add `read_stream` to the trait**

Open `src/libs/colmena/src/storage/domain/output_storage_repository.rs` and replace the file contents with:

```rust
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
```

- [ ] **Step 2: Re-export `StoredStream` from `storage::domain`**

Open `src/libs/colmena/src/storage/domain/mod.rs`. Find the line that re-exports `StoredBytes, StoredOutput, StoreRequest` and add `StoredStream` to it. If the existing line is:

```rust
pub use output_storage_repository::{OutputStorageRepository, StoreRequest, StoredBytes, StoredOutput};
```

Change it to:

```rust
pub use output_storage_repository::{
    OutputStorageRepository, StoreRequest, StoredBytes, StoredOutput, StoredStream,
};
```

If the existing structure differs, add `StoredStream` to whichever `pub use` line exposes the other types.

- [ ] **Step 3: Add stub `read_stream` to `LocalCacheStorageAdapter`**

Open `src/libs/colmena/src/storage/infrastructure/local_cache_adapter.rs`. Inside `impl OutputStorageRepository for LocalCacheStorageAdapter { ... }`, after the existing `read` method, add:

```rust
    async fn read_stream(
        &self,
        _storage_key: &str,
    ) -> Result<crate::storage::domain::StoredStream, StorageError> {
        Err(StorageError::InvalidInput(
            "read_stream not yet implemented for LocalCacheStorageAdapter".to_string(),
        ))
    }
```

- [ ] **Step 4: Add stub `read_stream` to `LocalHttpStorageAdapter`**

Open `src/libs/colmena/src/storage/infrastructure/local_http_adapter.rs`. Inside `impl OutputStorageRepository for LocalHttpStorageAdapter { ... }`, after the existing `read` method, add the same stub:

```rust
    async fn read_stream(
        &self,
        _storage_key: &str,
    ) -> Result<crate::storage::domain::StoredStream, StorageError> {
        Err(StorageError::InvalidInput(
            "read_stream not yet implemented for LocalHttpStorageAdapter".to_string(),
        ))
    }
```

- [ ] **Step 5: Add stub `read_stream` to `HttpCallbackStorageAdapter`**

Open `src/libs/colmena/src/storage/infrastructure/http_callback_adapter.rs`. Inside `impl OutputStorageRepository for HttpCallbackStorageAdapter { ... }`, after the existing `read` method, add the same stub.

- [ ] **Step 6: Verify the project compiles**

Run: `cargo check -p colmena_dag_engine`
Expected: compiles with no errors. Warnings about unused `_storage_key` are fine — they'll go away when we implement.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/storage/
git commit -m "feat(storage): add read_stream trait method + stubs

Stubs in all 3 adapters return InvalidInput. Next commits add real
implementations per adapter."
```

---

## Task 2: Implement `LocalCacheStorageAdapter::read_stream`

**Goal:** Single-chunk stream wrapping the in-memory `Vec<u8>`. No real RAM reduction (bytes are already alive in the cache), but a uniform API for callers.

**Files:**
- Modify: `src/libs/colmena/src/storage/infrastructure/local_cache_adapter.rs`

- [ ] **Step 1: Write the failing test**

Add this test to the `mod tests` block at the bottom of `local_cache_adapter.rs`:

```rust
    #[tokio::test]
    async fn read_stream_returns_single_chunk_with_correct_metadata() {
        use futures::StreamExt;

        let adapter = LocalCacheStorageAdapter::new();
        let bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let stored = adapter
            .store(req(bytes.clone(), "image/png"))
            .await
            .unwrap();

        let mut s = adapter.read_stream(&stored.storage_key).await.unwrap();
        assert_eq!(s.size_bytes, 4);
        assert_eq!(s.mime_type, "image/png");
        assert_eq!(s.filename, "x.bin"); // matches `req()` helper above

        // Drain the stream into a Vec<u8>
        let mut collected = Vec::new();
        while let Some(chunk) = s.stream.next().await {
            collected.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(collected, bytes);
    }

    #[tokio::test]
    async fn read_stream_unknown_key_errors() {
        let adapter = LocalCacheStorageAdapter::new();
        let err = adapter
            .read_stream("local://does-not-exist")
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::InvalidInput(_)));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p colmena_dag_engine local_cache_adapter::tests::read_stream_returns_single_chunk_with_correct_metadata`
Expected: FAIL — assertion `s.size_bytes == 4` fails (stub returns `Err`) OR test panics on `.unwrap()` of the stub error.

- [ ] **Step 3: Replace the stub with a real implementation**

In `local_cache_adapter.rs`, replace the stub `read_stream` body with:

```rust
    async fn read_stream(
        &self,
        storage_key: &str,
    ) -> Result<crate::storage::domain::StoredStream, StorageError> {
        use bytes::Bytes;
        use futures::stream;

        let entry = self.cache.get(storage_key).ok_or_else(|| {
            StorageError::InvalidInput(format!(
                "storage_key '{storage_key}' not found in LocalCache (was it stored in this process?)"
            ))
        })?;
        let stored = entry.value().clone();
        drop(entry); // release the DashMap guard before await points

        let size_bytes = stored.bytes.len() as u64;
        let mime_type = stored.mime_type.clone();
        let filename = stored.filename.clone();
        let chunk: Result<Bytes, StorageError> = Ok(Bytes::from(stored.bytes));
        let stream = stream::once(async move { chunk });

        Ok(crate::storage::domain::StoredStream {
            stream: Box::pin(stream),
            size_bytes,
            mime_type,
            filename,
        })
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -p colmena_dag_engine local_cache_adapter`
Expected: PASS — both new tests + all existing local_cache_adapter tests.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/storage/infrastructure/local_cache_adapter.rs
git commit -m "feat(storage): LocalCache read_stream impl (single-chunk)"
```

---

## Task 3: Implement `LocalHttpStorageAdapter::read_stream`

**Goal:** Read bytes from disk and wrap as a single-chunk stream. True chunked streaming for files would need `tokio-util::io::ReaderStream` (new dep) and provides no production benefit since this adapter is dev-only — single-chunk is the right call.

**Files:**
- Modify: `src/libs/colmena/src/storage/infrastructure/local_http_adapter.rs`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block at the bottom:

```rust
    #[tokio::test]
    async fn read_stream_returns_disk_bytes_with_metadata() {
        use futures::StreamExt;

        let tmp = TempDir::new().unwrap();
        let adapter = LocalHttpStorageAdapter::new(tmp.path().to_path_buf(), 0)
            .await
            .unwrap();
        let bytes = vec![0x01, 0x02, 0x03, 0x04, 0x05];
        let stored = adapter
            .store(req(bytes.clone(), "image/png"))
            .await
            .unwrap();

        let mut s = adapter.read_stream(&stored.storage_key).await.unwrap();
        assert_eq!(s.size_bytes, 5);
        assert_eq!(s.mime_type, "image/png");
        assert_eq!(s.filename, stored.storage_key);

        let mut collected = Vec::new();
        while let Some(chunk) = s.stream.next().await {
            collected.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(collected, bytes);
    }

    #[tokio::test]
    async fn read_stream_rejects_path_traversal() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalHttpStorageAdapter::new(tmp.path().to_path_buf(), 0)
            .await
            .unwrap();
        for bad in ["../etc/passwd", "/tmp/foo", "sub/dir/key"] {
            let err = adapter.read_stream(bad).await.unwrap_err();
            assert!(
                matches!(err, StorageError::InvalidInput(_)),
                "expected InvalidInput for '{bad}', got {err:?}"
            );
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p colmena_dag_engine local_http_adapter::tests::read_stream`
Expected: FAIL — stub returns InvalidInput so even the happy-path test sees the wrong error.

- [ ] **Step 3: Replace the stub with a real implementation**

In `local_http_adapter.rs`, replace the stub `read_stream` body with:

```rust
    async fn read_stream(
        &self,
        storage_key: &str,
    ) -> Result<crate::storage::domain::StoredStream, StorageError> {
        use bytes::Bytes;
        use futures::stream;

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

        let size_bytes = bytes.len() as u64;
        let mime_type = mime_from_filename(storage_key).to_string();
        let filename = storage_key.to_string();
        let chunk: Result<Bytes, StorageError> = Ok(Bytes::from(bytes));
        let stream = stream::once(async move { chunk });

        Ok(crate::storage::domain::StoredStream {
            stream: Box::pin(stream),
            size_bytes,
            mime_type,
            filename,
        })
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -p colmena_dag_engine local_http_adapter`
Expected: PASS — both new tests + all existing local_http_adapter tests.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/storage/infrastructure/local_http_adapter.rs
git commit -m "feat(storage): LocalHttp read_stream impl (single-chunk from disk)"
```

---

## Task 4: Implement `HttpCallbackStorageAdapter::read_stream` (true streaming)

**Goal:** This is the production adapter. Replace the current `bytes()`-buffered GET with `bytes_stream()` to actually save RAM. Sources `size_bytes` from the response's `Content-Length`.

**Files:**
- Modify: `src/libs/colmena/src/storage/infrastructure/http_callback_adapter.rs`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block at the bottom (the helpers `req`, `MockServer`, etc. are already imported in scope):

```rust
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
        let stored = adapter
            .store(req(vec![1, 2], "image/png"))
            .await
            .unwrap();
        let err = adapter.read_stream(&stored.storage_key).await.unwrap_err();
        assert!(matches!(err, StorageError::UploadFailed(_)));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p colmena_dag_engine http_callback_adapter::tests::read_stream`
Expected: FAIL — stub returns InvalidInput on all three.

- [ ] **Step 3: Replace the stub with a real streaming implementation**

In `http_callback_adapter.rs`, replace the stub `read_stream` body with:

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -p colmena_dag_engine http_callback_adapter`
Expected: PASS — all new tests + all existing http_callback_adapter tests.

- [ ] **Step 5: Verify the full storage module still passes**

Run: `cargo test --lib -p colmena_dag_engine storage`
Expected: PASS — entire `storage::` module green.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/storage/infrastructure/http_callback_adapter.rs
git commit -m "feat(storage): HttpCallback read_stream true streaming via bytes_stream"
```

---

## Task 5: Add Content-Type detection helper to `http.rs`

**Goal:** A single pure function that decides whether the request is multipart, given the merged headers map.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`

- [ ] **Step 1: Write the failing test**

At the bottom of `http.rs`, add a new test module before the closing `}` of the file (after the existing `attachment_placeholder_tests` module):

```rust
#[cfg(test)]
mod multipart_detection_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_multipart_from_form_data_header_lowercase() {
        let headers = json!({ "content-type": "multipart/form-data" });
        assert!(HttpNode::is_multipart_mode(headers.as_object().unwrap()));
    }

    #[test]
    fn detects_multipart_with_boundary_param() {
        let headers = json!({ "Content-Type": "multipart/form-data; boundary=foo" });
        assert!(HttpNode::is_multipart_mode(headers.as_object().unwrap()));
    }

    #[test]
    fn detects_other_multipart_subtypes() {
        let headers = json!({ "Content-Type": "multipart/mixed" });
        assert!(HttpNode::is_multipart_mode(headers.as_object().unwrap()));
    }

    #[test]
    fn rejects_json_content_type() {
        let headers = json!({ "Content-Type": "application/json" });
        assert!(!HttpNode::is_multipart_mode(headers.as_object().unwrap()));
    }

    #[test]
    fn rejects_missing_content_type() {
        let headers = json!({ "X-Custom": "yes" });
        assert!(!HttpNode::is_multipart_mode(headers.as_object().unwrap()));
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let headers = json!({ "CONTENT-TYPE": "multipart/form-data" });
        assert!(HttpNode::is_multipart_mode(headers.as_object().unwrap()));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p colmena_dag_engine multipart_detection_tests`
Expected: FAIL — function `is_multipart_mode` is not defined.

- [ ] **Step 3: Implement the helper**

In `http.rs`, inside `impl HttpNode { ... }`, after the `resolve_env_vars_in_value` method, add:

```rust
    /// Returns `true` when the merged headers map contains a Content-Type
    /// whose MIME type begins with `multipart/`. Header lookup is
    /// case-insensitive per RFC 9110 §5.1.
    pub(crate) fn is_multipart_mode(headers: &serde_json::Map<String, Value>) -> bool {
        for (k, v) in headers {
            if k.eq_ignore_ascii_case("content-type") {
                if let Some(s) = v.as_str() {
                    return s.trim_start().to_ascii_lowercase().starts_with("multipart/");
                }
            }
        }
        false
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -p colmena_dag_engine multipart_detection_tests`
Expected: PASS — all 6 tests.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs
git commit -m "feat(http_request): add is_multipart_mode header detection helper"
```

---

## Task 6: Define `PartSpec` and the body parser

**Goal:** Convert the `body` Value into a flat `Vec<PartSpec>` ready for resolution. Pure logic — no networking, no storage access. Tests cover every shape from the spec D2 table.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`

- [ ] **Step 1: Write the failing test**

Add a new module at the bottom of `http.rs`:

```rust
#[cfg(test)]
mod multipart_body_parser_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_url_becomes_url_part() {
        let body = json!({ "files": "https://example.com/a.pdf" });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartSpec::Url { field, url, filename_override, content_type_override } => {
                assert_eq!(field, "files");
                assert_eq!(url, "https://example.com/a.pdf");
                assert!(filename_override.is_none());
                assert!(content_type_override.is_none());
            }
            other => panic!("expected Url, got {other:?}"),
        }
    }

    #[test]
    fn string_attachment_becomes_attachment_part() {
        let body = json!({ "files": "$attachment:abc123" });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartSpec::Attachment { field, storage_key, filename_override, content_type_override } => {
                assert_eq!(field, "files");
                assert_eq!(storage_key, "abc123");
                assert!(filename_override.is_none());
                assert!(content_type_override.is_none());
            }
            other => panic!("expected Attachment, got {other:?}"),
        }
    }

    #[test]
    fn plain_string_becomes_text_part() {
        let body = json!({ "metadata": "uploaded by agent" });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartSpec::Text { field, value, content_type_override } => {
                assert_eq!(field, "metadata");
                assert_eq!(value, "uploaded by agent");
                assert!(content_type_override.is_none());
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn array_expands_to_multiple_parts_under_same_field() {
        let body = json!({ "files": ["https://a/1", "https://a/2", "$attachment:k"] });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 3);
        assert!(matches!(&parts[0], PartSpec::Url { field, .. } if field == "files"));
        assert!(matches!(&parts[1], PartSpec::Url { field, .. } if field == "files"));
        assert!(matches!(&parts[2], PartSpec::Attachment { field, .. } if field == "files"));
    }

    #[test]
    fn explicit_url_object_with_overrides() {
        let body = json!({
            "files": [{
                "url": "https://example.com/x",
                "filename": "report.pdf",
                "content_type": "application/pdf"
            }]
        });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartSpec::Url { url, filename_override, content_type_override, .. } => {
                assert_eq!(url, "https://example.com/x");
                assert_eq!(filename_override.as_deref(), Some("report.pdf"));
                assert_eq!(content_type_override.as_deref(), Some("application/pdf"));
            }
            other => panic!("expected Url, got {other:?}"),
        }
    }

    #[test]
    fn explicit_attachment_object_with_overrides() {
        let body = json!({
            "files": [{
                "attachment": "key-1",
                "filename": "x.png",
                "content_type": "image/png"
            }]
        });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            PartSpec::Attachment { storage_key, filename_override, content_type_override, .. } => {
                assert_eq!(storage_key, "key-1");
                assert_eq!(filename_override.as_deref(), Some("x.png"));
                assert_eq!(content_type_override.as_deref(), Some("image/png"));
            }
            other => panic!("expected Attachment, got {other:?}"),
        }
    }

    #[test]
    fn explicit_text_object_with_content_type() {
        let body = json!({
            "metadata": { "value": "hello", "content_type": "text/csv" }
        });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        match &parts[0] {
            PartSpec::Text { value, content_type_override, .. } => {
                assert_eq!(value, "hello");
                assert_eq!(content_type_override.as_deref(), Some("text/csv"));
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn number_and_boolean_become_text_parts() {
        let body = json!({ "count": 5, "flag": true });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 2);
        let has_count = parts.iter().any(|p| matches!(p, PartSpec::Text { field, value, .. } if field == "count" && value == "5"));
        let has_flag = parts.iter().any(|p| matches!(p, PartSpec::Text { field, value, .. } if field == "flag" && value == "true"));
        assert!(has_count && has_flag);
    }

    #[test]
    fn null_value_omits_field() {
        let body = json!({ "ignored": null, "kept": "yes" });
        let parts = HttpNode::parse_multipart_body(&body).unwrap();
        assert_eq!(parts.len(), 1);
        assert!(matches!(&parts[0], PartSpec::Text { field, .. } if field == "kept"));
    }

    #[test]
    fn malformed_object_errors() {
        let body = json!({ "files": [{ "unknown_field": "x" }] });
        let err = HttpNode::parse_multipart_body(&body).unwrap_err();
        assert!(err.to_string().contains("MultipartConfigError") || err.to_string().contains("unrecognized"));
    }

    #[test]
    fn body_must_be_object() {
        let body = json!("just a string");
        let err = HttpNode::parse_multipart_body(&body).unwrap_err();
        assert!(err.to_string().contains("object"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p colmena_dag_engine multipart_body_parser_tests`
Expected: FAIL — `PartSpec` and `parse_multipart_body` are not defined.

- [ ] **Step 3: Define `PartSpec` and the parser**

In `http.rs`, near the top of the file (after the `ATTACHMENT_PLACEHOLDER_PREFIX` constant), add:

```rust
const URL_HTTP_PREFIX: &str = "http://";
const URL_HTTPS_PREFIX: &str = "https://";

/// A single resolved multipart form part, prior to network I/O. Built by
/// [`HttpNode::parse_multipart_body`] and consumed by the form assembler.
#[derive(Debug, Clone)]
pub(crate) enum PartSpec {
    Url {
        field: String,
        url: String,
        filename_override: Option<String>,
        content_type_override: Option<String>,
    },
    Attachment {
        field: String,
        storage_key: String,
        filename_override: Option<String>,
        content_type_override: Option<String>,
    },
    Text {
        field: String,
        value: String,
        content_type_override: Option<String>,
    },
}
```

Then add inside `impl HttpNode { ... }` (after `is_multipart_mode`):

```rust
    /// Parse a `body` JSON object into a flat list of `PartSpec`s. Pure logic,
    /// no I/O. The rules match the design spec D2 table.
    ///
    /// Returns an error for malformed bodies (non-object root, unrecognized
    /// explicit object shape, etc.).
    pub(crate) fn parse_multipart_body(
        body: &Value,
    ) -> Result<Vec<PartSpec>, Box<dyn StdError + Send + Sync>> {
        let map = body.as_object().ok_or_else(|| -> Box<dyn StdError + Send + Sync> {
            "MultipartConfigError: body must be a JSON object in multipart mode".into()
        })?;

        let mut parts = Vec::new();
        for (field, value) in map {
            Self::push_parts_for_value(field, value, &mut parts)?;
        }
        Ok(parts)
    }

    fn push_parts_for_value(
        field: &str,
        value: &Value,
        out: &mut Vec<PartSpec>,
    ) -> Result<(), Box<dyn StdError + Send + Sync>> {
        match value {
            Value::Null => Ok(()),
            Value::String(s) => {
                out.push(Self::classify_string_part(field, s));
                Ok(())
            }
            Value::Number(n) => {
                out.push(PartSpec::Text {
                    field: field.to_string(),
                    value: n.to_string(),
                    content_type_override: None,
                });
                Ok(())
            }
            Value::Bool(b) => {
                out.push(PartSpec::Text {
                    field: field.to_string(),
                    value: b.to_string(),
                    content_type_override: None,
                });
                Ok(())
            }
            Value::Array(arr) => {
                for item in arr {
                    Self::push_parts_for_value(field, item, out)?;
                }
                Ok(())
            }
            Value::Object(obj) => {
                let filename_override = obj
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let content_type_override = obj
                    .get("content_type")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
                    out.push(PartSpec::Url {
                        field: field.to_string(),
                        url: url.to_string(),
                        filename_override,
                        content_type_override,
                    });
                    Ok(())
                } else if let Some(key) = obj.get("attachment").and_then(|v| v.as_str()) {
                    out.push(PartSpec::Attachment {
                        field: field.to_string(),
                        storage_key: key.to_string(),
                        filename_override,
                        content_type_override,
                    });
                    Ok(())
                } else if let Some(value_s) = obj.get("value").and_then(|v| v.as_str()) {
                    out.push(PartSpec::Text {
                        field: field.to_string(),
                        value: value_s.to_string(),
                        content_type_override,
                    });
                    Ok(())
                } else {
                    Err(format!(
                        "MultipartConfigError: object under field '{field}' has none of \
                         'url', 'attachment', 'value' (unrecognized shape)"
                    )
                    .into())
                }
            }
        }
    }

    fn classify_string_part(field: &str, s: &str) -> PartSpec {
        if let Some(rest) = s.strip_prefix(ATTACHMENT_PLACEHOLDER_PREFIX) {
            PartSpec::Attachment {
                field: field.to_string(),
                storage_key: rest.to_string(),
                filename_override: None,
                content_type_override: None,
            }
        } else if s.starts_with(URL_HTTPS_PREFIX) || s.starts_with(URL_HTTP_PREFIX) {
            PartSpec::Url {
                field: field.to_string(),
                url: s.to_string(),
                filename_override: None,
                content_type_override: None,
            }
        } else {
            PartSpec::Text {
                field: field.to_string(),
                value: s.to_string(),
                content_type_override: None,
            }
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -p colmena_dag_engine multipart_body_parser_tests`
Expected: PASS — all 11 tests.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs
git commit -m "feat(http_request): PartSpec enum + multipart body parser"
```

---

## Task 7: URL part resolution — HEAD pre-flight + streaming GET

**Goal:** Take a `PartSpec::Url` and produce a `(stream, size_bytes, mime_type, filename)` tuple suitable for `Part::stream_with_length`. Enforces `https://` by default, `max_file_size_bytes`, and `url_download_timeout_secs`.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`

- [ ] **Step 1: Write the failing test**

Add a new module to `http.rs`:

```rust
#[cfg(test)]
mod multipart_url_resolution_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn happy_path_resolves_size_and_mime_from_head() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/file"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", "4")
                    .insert_header("Content-Type", "application/pdf"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/file"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1, 2, 3, 4]))
            .mount(&server)
            .await;

        let resolver = MultipartUrlResolver {
            max_file_size_bytes: 100_000,
            timeout_secs: 5,
            allow_http_urls: true, // wiremock serves http://
        };
        let url = format!("{}/file", server.uri());
        let resolved = resolver.resolve(&url).await.unwrap();
        assert_eq!(resolved.size_bytes, 4);
        assert_eq!(resolved.content_type, "application/pdf");
        assert_eq!(resolved.filename, "file");
    }

    #[tokio::test]
    async fn rejects_http_when_not_allowed() {
        let resolver = MultipartUrlResolver {
            max_file_size_bytes: 100_000,
            timeout_secs: 5,
            allow_http_urls: false,
        };
        let err = resolver.resolve("http://example.com/x").await.unwrap_err();
        assert!(err.to_string().contains("http://"), "got {err}");
    }

    #[tokio::test]
    async fn rejects_unknown_scheme() {
        let resolver = MultipartUrlResolver {
            max_file_size_bytes: 100_000,
            timeout_secs: 5,
            allow_http_urls: true,
        };
        let err = resolver.resolve("ftp://example.com/x").await.unwrap_err();
        assert!(err.to_string().contains("scheme"));
    }

    #[tokio::test]
    async fn rejects_when_content_length_missing() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/file"))
            .respond_with(ResponseTemplate::new(200).insert_header("Content-Type", "application/pdf"))
            .mount(&server)
            .await;

        let resolver = MultipartUrlResolver {
            max_file_size_bytes: 100_000,
            timeout_secs: 5,
            allow_http_urls: true,
        };
        let url = format!("{}/file", server.uri());
        let err = resolver.resolve(&url).await.unwrap_err();
        assert!(err.to_string().contains("Content-Length"), "got {err}");
    }

    #[tokio::test]
    async fn rejects_when_oversized() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/big"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", "999999")
                    .insert_header("Content-Type", "application/octet-stream"),
            )
            .mount(&server)
            .await;

        let resolver = MultipartUrlResolver {
            max_file_size_bytes: 100,
            timeout_secs: 5,
            allow_http_urls: true,
        };
        let url = format!("{}/big", server.uri());
        let err = resolver.resolve(&url).await.unwrap_err();
        assert!(err.to_string().contains("too large") || err.to_string().contains("FileTooLarge"));
    }

    #[tokio::test]
    async fn filename_from_content_disposition_overrides_url_path() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/file"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", "10")
                    .insert_header("Content-Type", "application/pdf")
                    .insert_header("Content-Disposition", "attachment; filename=\"report.pdf\""),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/file"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 10]))
            .mount(&server)
            .await;

        let resolver = MultipartUrlResolver {
            max_file_size_bytes: 100_000,
            timeout_secs: 5,
            allow_http_urls: true,
        };
        let url = format!("{}/file", server.uri());
        let resolved = resolver.resolve(&url).await.unwrap();
        assert_eq!(resolved.filename, "report.pdf");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p colmena_dag_engine multipart_url_resolution_tests`
Expected: FAIL — `MultipartUrlResolver` is not defined.

- [ ] **Step 3: Implement `MultipartUrlResolver`**

In `http.rs`, add these to the existing `use` block at the top of the file (right after the existing `use std::sync::Arc;`):

```rust
use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;
```

Then, after the `PartSpec` enum near the top of the file, add the resolver type:

```rust
/// Resolution result for a single URL part: a streaming reader + the metadata
/// we'll forward to the downstream multipart form.
pub(crate) struct ResolvedUrlPart {
    pub stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    pub size_bytes: u64,
    pub content_type: String,
    pub filename: String,
}

pub(crate) struct MultipartUrlResolver {
    pub max_file_size_bytes: u64,
    pub timeout_secs: u64,
    pub allow_http_urls: bool,
}

impl MultipartUrlResolver {
    pub(crate) async fn resolve(
        &self,
        url: &str,
    ) -> Result<ResolvedUrlPart, Box<dyn StdError + Send + Sync>> {
        let parsed = Url::parse(url)
            .map_err(|e| format!("UrlValidationFailed: cannot parse '{url}': {e}"))?;
        match parsed.scheme() {
            "https" => {}
            "http" if self.allow_http_urls => {}
            "http" => {
                return Err(format!(
                    "UrlValidationFailed: plain http:// URL '{url}' rejected (set allow_http_urls=true to permit)"
                )
                .into());
            }
            other => {
                return Err(format!(
                    "UrlValidationFailed: scheme '{other}' not supported (only http/https)"
                )
                .into());
            }
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .http1_only()
            .build()?;

        // HEAD pre-flight
        let head = client.head(parsed.clone()).send().await.map_err(|e| {
            format!("UrlValidationFailed: HEAD for '{url}' failed: {e}")
        })?;
        if !head.status().is_success() {
            return Err(format!(
                "UrlValidationFailed: HEAD for '{url}' returned status {}",
                head.status()
            )
            .into());
        }
        let size_bytes = head
            .content_length()
            .ok_or_else(|| format!(
                "UrlValidationFailed: HEAD for '{url}' returned no Content-Length"
            ))?;
        if size_bytes > self.max_file_size_bytes {
            return Err(format!(
                "FileTooLarge: '{url}' declared {size_bytes} bytes, max is {}",
                self.max_file_size_bytes
            )
            .into());
        }
        let content_type = head
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let filename = filename_from_disposition(
            head.headers().get(reqwest::header::CONTENT_DISPOSITION),
        )
        .unwrap_or_else(|| filename_from_url_path(&parsed));

        // Streaming GET — the body is not buffered.
        let resp = client.get(parsed).send().await.map_err(|e| {
            format!("UrlValidationFailed: GET for '{url}' failed: {e}")
        })?;
        if !resp.status().is_success() {
            return Err(format!(
                "UrlValidationFailed: GET for '{url}' returned status {}",
                resp.status()
            )
            .into());
        }
        let stream = resp.bytes_stream();

        Ok(ResolvedUrlPart {
            stream: Box::pin(stream),
            size_bytes,
            content_type,
            filename,
        })
    }
}

/// Parse `Content-Disposition: attachment; filename="report.pdf"` (or unquoted)
/// into the bare filename. Returns None for unrecognized shapes or absent
/// header. RFC 5987 (`filename*=`) is intentionally not handled in v1.
fn filename_from_disposition(header: Option<&reqwest::header::HeaderValue>) -> Option<String> {
    let v = header?.to_str().ok()?;
    let after = v.split(';').find_map(|chunk| {
        let chunk = chunk.trim();
        let lower = chunk.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("filename=") {
            let _ = rest;
            Some(&chunk[("filename=".len())..])
        } else {
            None
        }
    })?;
    let unquoted = after.trim().trim_matches('"').to_string();
    if unquoted.is_empty() {
        None
    } else {
        Some(unquoted)
    }
}

/// Last path segment of the URL (after the final `/`), URL-decoded. Falls back
/// to `"file"` for URLs with no usable path component.
fn filename_from_url_path(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut s| s.next_back().filter(|seg| !seg.is_empty()).map(|s| s.to_string()))
        .unwrap_or_else(|| "file".to_string())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -p colmena_dag_engine multipart_url_resolution_tests`
Expected: PASS — all 6 tests.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs
git commit -m "feat(http_request): MultipartUrlResolver with HEAD pre-flight"
```

---

## Task 8: Wire the multipart branch into `execute()`

**Goal:** Pull it all together. When `is_multipart_mode` is true, route through a new `execute_multipart` method that builds and sends the form. The existing JSON path is untouched.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs`

- [ ] **Step 1: Write the failing test (end-to-end happy path)**

Add a new test module to `http.rs`:

```rust
#[cfg(test)]
mod multipart_execute_tests {
    use super::*;
    use crate::storage::domain::{MockOutputStorageRepository, StoredStream};
    use bytes::Bytes;
    use futures::stream;
    use std::collections::HashMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    /// Build a multipart-routed config that POSTs to `<server>/upload`.
    fn mk_config(server_uri: &str, body: Value) -> Value {
        serde_json::json!({
            "base_url": server_uri,
            "endpoint": "/upload",
            "method": "POST",
            "headers": { "Content-Type": "multipart/form-data" },
            "allow_http_urls": true, // wiremock is http://
            "body": body,
        })
    }

    #[tokio::test]
    async fn multipart_with_two_url_parts_sends_form() {
        let server = MockServer::start().await;

        // Two upstream files
        Mock::given(method("HEAD"))
            .and(path("/u1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", "4")
                    .insert_header("Content-Type", "application/pdf"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/u1"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1, 2, 3, 4]))
            .mount(&server)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/u2"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", "3")
                    .insert_header("Content-Type", "application/pdf"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/u2"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![5, 6, 7]))
            .mount(&server)
            .await;

        // Downstream upload — capture the multipart body and assert basic shape
        Mock::given(method("POST"))
            .and(path("/upload"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })))
            .mount(&server)
            .await;

        let url1 = format!("{}/u1", server.uri());
        let url2 = format!("{}/u2", server.uri());
        let body = serde_json::json!({ "files": [url1, url2] });
        let config = mk_config(&server.uri(), body);

        let node = HttpNode::new();
        let out = node
            .execute(
                &HashMap::<String, Value>::new(),
                &config,
                &mut serde_json::json!({}),
                None,
            )
            .await
            .expect("execute ok");
        assert_eq!(out["status"], 200);

        // Verify the downstream actually got multipart by inspecting recorded requests
        let received = server.received_requests().await.unwrap();
        let upload_req = received
            .iter()
            .find(|r| r.url.path() == "/upload")
            .expect("upload request received");
        let ct = upload_req
            .headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.starts_with("multipart/form-data"), "got {ct}");
        assert!(ct.contains("boundary="), "boundary should be present in {ct}");
    }

    #[tokio::test]
    async fn multipart_with_attachment_part_streams_via_storage() {
        use futures::StreamExt;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/upload"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })))
            .mount(&server)
            .await;

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_read_stream()
            .times(1)
            .withf(|key: &str| key == "k-abc")
            .returning(|_| {
                let chunk: Result<Bytes, crate::storage::domain::StorageError> =
                    Ok(Bytes::from_static(b"hello"));
                Ok(StoredStream {
                    stream: Box::pin(stream::once(async move { chunk })),
                    size_bytes: 5,
                    mime_type: "text/plain".to_string(),
                    filename: "hello.txt".to_string(),
                })
            });

        let node = HttpNode::new().with_storage(std::sync::Arc::new(storage));
        let body = serde_json::json!({ "files": "$attachment:k-abc" });
        let config = mk_config(&server.uri(), body);
        let out = node
            .execute(
                &HashMap::<String, Value>::new(),
                &config,
                &mut serde_json::json!({}),
                None,
            )
            .await
            .expect("execute ok");
        assert_eq!(out["status"], 200);
    }

    #[tokio::test]
    async fn multipart_with_too_many_parts_errors_before_upload() {
        let server = MockServer::start().await;
        // No upstream mocks needed — should fail before any HEAD.
        let body = serde_json::json!({
            "files": [
                "https://a/1", "https://a/2", "https://a/3", "https://a/4",
                "https://a/5", "https://a/6", "https://a/7", "https://a/8",
                "https://a/9", "https://a/10", "https://a/11"
            ]
        });
        let mut config = mk_config(&server.uri(), body);
        config["max_parts"] = serde_json::json!(10);

        let node = HttpNode::new();
        let err = node
            .execute(
                &HashMap::<String, Value>::new(),
                &config,
                &mut serde_json::json!({}),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("TooManyParts"), "got {err}");
    }

    #[tokio::test]
    async fn existing_json_path_unaffected_without_multipart_header() {
        let server = MockServer::start().await;
        use wiremock::matchers::body_json;
        Mock::given(method("POST"))
            .and(path("/json"))
            .and(body_json(serde_json::json!({ "hi": "there" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })))
            .mount(&server)
            .await;
        let config = serde_json::json!({
            "base_url": server.uri(),
            "endpoint": "/json",
            "method": "POST",
            "body": { "hi": "there" }
        });
        let node = HttpNode::new();
        let out = node
            .execute(
                &HashMap::<String, Value>::new(),
                &config,
                &mut serde_json::json!({}),
                None,
            )
            .await
            .expect("ok");
        assert_eq!(out["status"], 200);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p colmena_dag_engine multipart_execute_tests`
Expected: FAIL — the multipart branch doesn't exist in `execute()` yet.

- [ ] **Step 3: Refactor `execute()` to route into a new `execute_multipart` method**

Inside `impl ExecutableNode for HttpNode { async fn execute(...) ... }`, find the body-handling section (around line 339):

```rust
        // 6. Body (Inputs or Config)
        let body_val = inputs.get("body").or_else(|| config.get("body"));
```

Right BEFORE that section, insert a multipart-branch check. The full replacement of section 6 becomes:

```rust
        // 6. Body (Inputs or Config) — branch on multipart vs JSON/string
        // Build a merged headers map for the multipart detector
        let mut merged_headers = serde_json::Map::new();
        if let Some(h) = config.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in h {
                merged_headers.insert(k.clone(), v.clone());
            }
        }
        if let Some(h) = inputs.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in h {
                merged_headers.insert(k.clone(), v.clone());
            }
        }

        if Self::is_multipart_mode(&merged_headers) {
            return self
                .execute_multipart(&full_url_str, method_str, &merged_headers, inputs, config)
                .await;
        }

        let body_val = inputs.get("body").or_else(|| config.get("body"));
```

Keep the rest of the `execute` method unchanged after this point — the JSON / string path stays in place.

- [ ] **Step 4: Implement `execute_multipart`**

Inside `impl HttpNode { ... }`, after the URL resolver code from Task 7, add:

```rust
    const DEFAULT_MAX_FILE_SIZE_BYTES: u64 = 104_857_600; // 100 MiB
    const DEFAULT_MAX_PARTS: usize = 10;
    const DEFAULT_URL_DOWNLOAD_TIMEOUT_SECS: u64 = 30;

    fn limit_u64(config: &Value, key: &str, default: u64) -> u64 {
        config
            .get(key)
            .and_then(|v| v.as_u64())
            .unwrap_or(default)
    }
    fn limit_usize(config: &Value, key: &str, default: usize) -> usize {
        config
            .get(key)
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(default)
    }
    fn limit_bool(config: &Value, key: &str, default: bool) -> bool {
        config
            .get(key)
            .and_then(|v| v.as_bool())
            .unwrap_or(default)
    }

    async fn execute_multipart(
        &self,
        full_url: &str,
        method_str: &str,
        merged_headers: &serde_json::Map<String, Value>,
        inputs: &NodeInputs,
        config: &Value,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let body_val = inputs
            .get("body")
            .or_else(|| config.get("body"))
            .ok_or("MultipartConfigError: body is required in multipart mode")?;

        // Apply env-var resolution to string leaves before parsing so that
        // `${VAR}` works inside URLs and text values, matching JSON path.
        let body_resolved = Self::resolve_env_vars_in_value(body_val);

        let parts = Self::parse_multipart_body(&body_resolved)?;

        let max_parts = Self::limit_usize(config, "max_parts", Self::DEFAULT_MAX_PARTS);
        if parts.len() > max_parts {
            return Err(format!(
                "TooManyParts: body produced {} parts, max is {}",
                parts.len(),
                max_parts
            )
            .into());
        }

        let max_file_size_bytes =
            Self::limit_u64(config, "max_file_size_bytes", Self::DEFAULT_MAX_FILE_SIZE_BYTES);
        let timeout_secs = Self::limit_u64(
            config,
            "url_download_timeout_secs",
            Self::DEFAULT_URL_DOWNLOAD_TIMEOUT_SECS,
        );
        let allow_http_urls = Self::limit_bool(config, "allow_http_urls", false);

        let resolver = MultipartUrlResolver {
            max_file_size_bytes,
            timeout_secs,
            allow_http_urls,
        };

        let parts_count = parts.len();
        let mut form = reqwest::multipart::Form::new();
        for spec in parts {
            form = self
                .add_part_to_form(form, spec, &resolver, max_file_size_bytes)
                .await?;
        }

        // Build the outbound request — same client tuning as JSON path
        let client = reqwest::Client::builder().http1_only().build()?;
        let url = Url::parse(full_url).map_err(|e| format!("Invalid URL '{full_url}': {e}"))?;
        let method = reqwest::Method::from_str(method_str)
            .map_err(|e| format!("Invalid HTTP method '{method_str}': {e}"))?;
        let mut req = client.request(method, url);
        req = req.header("User-Agent", "colmena-http-node/0.1");

        // Forward all headers EXCEPT Content-Type — reqwest will set
        // multipart/form-data; boundary=... itself.
        for (k, v) in merged_headers {
            if k.eq_ignore_ascii_case("content-type") {
                continue;
            }
            if let Some(v_str) = v.as_str() {
                let v_resolved = Self::resolve_env_vars(v_str).map_err(|e| {
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                        as Box<dyn StdError + Send + Sync>
                })?;
                req = req.header(k, v_resolved);
            }
        }
        if let Some(token) = inputs.get("bearer_token").and_then(|v| v.as_str()) {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(auth) = inputs.get("authorization").and_then(|v| v.as_str()) {
            req = req.header("Authorization", auth);
        }

        println!("[HttpNode] → {method_str} {full_url} (multipart, {parts_count} parts)");

        let response = req.multipart(form).send().await?;
        let status = response.status().as_u16();
        println!("[HttpNode] ← {status} ({full_url})");

        let response_body: Value = match response.json::<Value>().await {
            Ok(json) => json,
            Err(_) => Value::Null,
        };

        Ok(serde_json::json!({
            "status": status,
            "body": response_body
        }))
    }

    async fn add_part_to_form(
        &self,
        form: reqwest::multipart::Form,
        spec: PartSpec,
        resolver: &MultipartUrlResolver,
        max_file_size_bytes: u64,
    ) -> Result<reqwest::multipart::Form, Box<dyn StdError + Send + Sync>> {
        use futures::StreamExt;
        match spec {
            PartSpec::Text {
                field,
                value,
                content_type_override,
            } => {
                let mut part = reqwest::multipart::Part::text(value);
                if let Some(ct) = content_type_override {
                    part = part.mime_str(&ct)?;
                }
                Ok(form.part(field, part))
            }
            PartSpec::Url {
                field,
                url,
                filename_override,
                content_type_override,
            } => {
                let resolved = resolver.resolve(&url).await?;
                let filename = filename_override.unwrap_or(resolved.filename);
                let content_type = content_type_override.unwrap_or(resolved.content_type);
                let mapped = resolved
                    .stream
                    .map(|chunk| chunk.map_err(std::io::Error::other));
                let body = reqwest::Body::wrap_stream(mapped);
                let part = reqwest::multipart::Part::stream_with_length(body, resolved.size_bytes)
                    .file_name(filename)
                    .mime_str(&content_type)?;
                Ok(form.part(field, part))
            }
            PartSpec::Attachment {
                field,
                storage_key,
                filename_override,
                content_type_override,
            } => {
                let storage = self.storage.as_ref().ok_or_else(|| -> Box<dyn StdError + Send + Sync> {
                    format!("AttachmentNotFound: body references '$attachment:{storage_key}' but no OutputStorageRepository is wired").into()
                })?;
                let stored = storage
                    .read_stream(&storage_key)
                    .await
                    .map_err(|e| -> Box<dyn StdError + Send + Sync> { Box::new(e) })?;
                if stored.size_bytes > max_file_size_bytes {
                    return Err(format!(
                        "FileTooLarge: attachment '{storage_key}' is {} bytes, max is {max_file_size_bytes}",
                        stored.size_bytes
                    )
                    .into());
                }
                let filename = filename_override.unwrap_or(stored.filename);
                let content_type = content_type_override.unwrap_or(stored.mime_type);
                let mapped = stored
                    .stream
                    .map(|chunk| chunk.map_err(std::io::Error::other));
                let body = reqwest::Body::wrap_stream(mapped);
                let part = reqwest::multipart::Part::stream_with_length(body, stored.size_bytes)
                    .file_name(filename)
                    .mime_str(&content_type)?;
                Ok(form.part(field, part))
            }
        }
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib -p colmena_dag_engine multipart_execute_tests`
Expected: PASS — all 4 tests.

- [ ] **Step 6: Run the full http.rs module to verify backwards compat**

Run: `cargo test --lib -p colmena_dag_engine nodes::http`
Expected: PASS — multipart tests + existing `attachment_placeholder_tests` + the rest.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs
git commit -m "feat(http_request): wire multipart branch into execute()

- New execute_multipart routes URL/attachment/text parts into a
  reqwest::multipart::Form using streaming bodies.
- JSON/string path unchanged when Content-Type is not multipart/*."
```

---

## Task 9: Integration test with wiremock end-to-end

**Goal:** A single integration test covering the realistic flow: 3 signed URLs from a mock upstream, multipart POST to a mock downstream, verifying boundary presence + part count + chunked transfer.

**Files:**
- Create: `tests/multipart_http_test.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/multipart_http_test.rs`:

```rust
//! End-to-end integration test for http_request multipart streaming.
//! Spins up two wiremock servers — one upstream (sources) and one downstream
//! (target) — and runs the node through HttpNode::execute.

use colmena_dag_engine::dag_engine::domain::node::ExecutableNode;
use colmena_dag_engine::dag_engine::infrastructure::nodes::http::HttpNode;
use serde_json::{json, Value};
use std::collections::HashMap;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn end_to_end_three_url_parts_multipart_upload() {
    let upstream = MockServer::start().await;
    let downstream = MockServer::start().await;

    let payloads: Vec<(&str, Vec<u8>, &str)> = vec![
        ("/a", vec![0xAAu8; 100], "application/pdf"),
        ("/b", vec![0xBBu8; 500], "application/pdf"),
        ("/c", vec![0xCCu8; 2_000], "application/pdf"),
    ];
    for (p, body, ct) in &payloads {
        Mock::given(method("HEAD"))
            .and(path(*p))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", body.len().to_string())
                    .insert_header("Content-Type", *ct),
            )
            .mount(&upstream)
            .await;
        Mock::given(method("GET"))
            .and(path(*p))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&upstream)
            .await;
    }

    Mock::given(method("POST"))
        .and(path("/kb/upload"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "uploaded": 3 })))
        .mount(&downstream)
        .await;

    let body = json!({
        "files": [
            format!("{}/a", upstream.uri()),
            format!("{}/b", upstream.uri()),
            format!("{}/c", upstream.uri()),
        ],
        "description": "uploaded by agent"
    });

    let config = json!({
        "base_url": downstream.uri(),
        "endpoint": "/kb/upload",
        "method": "POST",
        "headers": { "Content-Type": "multipart/form-data" },
        "allow_http_urls": true,
        "body": body
    });

    let node = HttpNode::new();
    let out = node
        .execute(
            &HashMap::<String, Value>::new(),
            &config,
            &mut json!({}),
            None,
        )
        .await
        .expect("multipart end-to-end ok");
    assert_eq!(out["status"], 200);
    assert_eq!(out["body"]["uploaded"], 3);

    // Inspect the downstream request: must be multipart with boundary
    let received = downstream.received_requests().await.unwrap();
    let req = received
        .iter()
        .find(|r| r.url.path() == "/kb/upload")
        .expect("downstream POST received");
    let ct = req
        .headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.starts_with("multipart/form-data"), "got {ct}");
    assert!(ct.contains("boundary="), "boundary missing in {ct}");

    // The body should contain all three payloads (sanity — full multipart
    // parsing is overkill; we just confirm bytes survived end-to-end).
    let body_bytes = &req.body;
    assert!(body_bytes.windows(100).any(|w| w == &[0xAAu8; 100][..]));
    assert!(body_bytes.windows(500).any(|w| w == &[0xBBu8; 500][..]));
    // 2_000 byte window check is O(n*m) but n is small here
    assert!(body_bytes
        .windows(2_000)
        .any(|w| w == &vec![0xCCu8; 2_000][..]));
    // Text part should be inline (uppercase value to avoid colliding with payload bytes)
    assert!(body_bytes
        .windows(b"uploaded by agent".len())
        .any(|w| w == b"uploaded by agent"));
}

#[tokio::test]
async fn end_to_end_oversized_upstream_aborts_before_downstream_post() {
    let upstream = MockServer::start().await;
    let downstream = MockServer::start().await;

    Mock::given(method("HEAD"))
        .and(path("/big"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", "1000000")
                .insert_header("Content-Type", "application/octet-stream"),
        )
        .mount(&upstream)
        .await;

    Mock::given(method("POST"))
        .and(path("/kb/upload"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&downstream)
        .await;

    let config = json!({
        "base_url": downstream.uri(),
        "endpoint": "/kb/upload",
        "method": "POST",
        "headers": { "Content-Type": "multipart/form-data" },
        "allow_http_urls": true,
        "max_file_size_bytes": 100,
        "body": { "files": format!("{}/big", upstream.uri()) }
    });

    let node = HttpNode::new();
    let err = node
        .execute(
            &HashMap::<String, Value>::new(),
            &config,
            &mut json!({}),
            None,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("FileTooLarge"), "got {err}");

    // Downstream must NOT have received any POST.
    let received = downstream.received_requests().await.unwrap();
    assert!(
        received.iter().all(|r| r.url.path() != "/kb/upload"),
        "downstream received an upload despite upstream being oversized"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail or pass**

Run: `cargo test --test multipart_http_test`
Expected: PASS — these are end-to-end checks against code already implemented in Tasks 5-8. If they fail, the failure pinpoints a bug in the wiring; fix before committing.

If `HttpNode` is not in the public path expected by the test, adjust the `use` line at the top. Verify with: `cargo check --tests -p colmena_dag_engine`.

- [ ] **Step 3: Commit**

```bash
git add tests/multipart_http_test.rs
git commit -m "test(http_request): wiremock end-to-end multipart streaming"
```

---

## Task 10: Test graph + documentation

**Goal:** A CLI-runnable smoke graph + canonical docs so a human (or another agent) discovers how to use multipart mode.

**Files:**
- Create: `tests/graphs/external/multipart_upload.json`
- Modify: `docs/node_configurations.json`
- Modify: `docs/developer_guide/25_web_nodes.md`
- Modify: `docs/agent_context/node_ports_reference.md`

- [ ] **Step 1: Create the test graph**

Create `tests/graphs/external/multipart_upload.json` with content:

```json
{
  "name": "multipart_upload_smoke",
  "description": "Posts two signed URLs and a text field as multipart/form-data to httpbin's /post endpoint, which echoes back the parts received. Tweak base_url+endpoint for ADP smoke testing.",
  "nodes": [
    {
      "id": "trigger",
      "node_type": "trigger",
      "config": {
        "default": { "files": ["https://httpbin.org/image/png"], "metadata": "smoke" }
      }
    },
    {
      "id": "upload",
      "node_type": "http_request",
      "config": {
        "base_url": "https://httpbin.org",
        "endpoint": "/post",
        "method": "POST",
        "headers": { "Content-Type": "multipart/form-data" },
        "max_file_size_bytes": 5242880,
        "url_download_timeout_secs": 20,
        "body": {
          "files": ["https://httpbin.org/image/png"],
          "metadata": "smoke"
        }
      }
    }
  ],
  "edges": [
    { "from": "trigger", "to": "upload" }
  ]
}
```

- [ ] **Step 2: Smoke-run it locally**

Run:
```bash
source .env 2>/dev/null
cargo run --bin dag_engine -- run tests/graphs/external/multipart_upload.json
```
Expected: exits 0, logs show `200` response from httpbin echoing the multipart form fields.

- [ ] **Step 3: Update `docs/node_configurations.json`**

Open `docs/node_configurations.json`. Find the `http_request` entry under `node_types`. Make these specific changes:

- In `description`, append (after the existing `$attachment:` paragraph):
  ```
  MULTIPART MODE: When the resolved Content-Type header starts with 'multipart/', the body is interpreted as a multipart form. Each top-level field's value is treated as: a URL string (HEAD-validated then streamed in), a $attachment:<storage_key> string (streamed from OutputStorageRepository), a plain string (text part), an array (expanded to N parts under the same field), or an explicit object with url/attachment/value + optional filename/content_type. Number and boolean values are coerced to text. Null omits the field. Limits are configurable per request.
  ```

- In `config_fields`, add these four new entries (sibling to `body`, `bearer_token`, etc.):
  ```json
  "max_file_size_bytes": {
    "type": "integer",
    "required": false,
    "default": 104857600,
    "description": "Hard cap per file part in multipart mode. Validated against Content-Length (URL parts) or size_bytes (attachment parts).",
    "example": 52428800
  },
  "max_parts": {
    "type": "integer",
    "required": false,
    "default": 10,
    "description": "Maximum total parts (file + text) per request in multipart mode.",
    "example": 5
  },
  "url_download_timeout_secs": {
    "type": "integer",
    "required": false,
    "default": 30,
    "description": "Timeout (seconds) for HEAD pre-flight and GET connect/headers phase of URL parts in multipart mode. Does NOT bound body transfer duration.",
    "example": 60
  },
  "allow_http_urls": {
    "type": "boolean",
    "required": false,
    "default": false,
    "description": "When false, plain http:// URLs in multipart parts are rejected (SSRF mitigation). Set true to permit insecure URLs.",
    "example": true
  }
  ```

- [ ] **Step 4: Add a "Multipart uploads" section to `docs/developer_guide/25_web_nodes.md`**

Open `docs/developer_guide/25_web_nodes.md` and append a new section. Match the existing heading style (look at adjacent sections first; use `##` or `###` consistently). The new section's content:

````markdown
## Subida de archivos por multipart (`http_request`)

Cuando el header `Content-Type` empieza con `multipart/`, el nodo `http_request` cambia al modo multipart: cada campo del `body` se interpreta como una parte del form. Los archivos se transmiten por **streaming** (URLs vía `bytes_stream`, attachments vía `OutputStorageRepository::read_stream`), sin bufferizar el payload completo en RAM del worker — clave para escalar en Cloud Run.

### Interpretación de cada campo del `body`

| Forma del valor | Resultado |
|---|---|
| String `$attachment:<storage_key>` | Parte de archivo, bytes streameados desde el storage. |
| String que empieza con `https://` (o `http://` si `allow_http_urls=true`) | Parte de archivo, HEAD para validar tamaño + GET streaming. |
| Cualquier otro string | Text part (campo no-archivo). |
| Number o boolean | Coerced a su representación string como text part. |
| `null` | El campo se omite. |
| Array | Se expande a N partes con el mismo `field_name`. |
| Objeto `{ "url": "...", "filename": "...", "content_type": "..." }` | Parte de archivo con overrides explícitos. |
| Objeto `{ "attachment": "<key>", "filename": "...", "content_type": "..." }` | Parte de archivo desde storage con overrides. |
| Objeto `{ "value": "...", "content_type": "..." }` | Text part con content-type custom. |

### Límites configurables

| `config_field` | Default | Descripción |
|---|---|---|
| `max_file_size_bytes` | 100 MiB | Cap por parte de archivo. |
| `max_parts` | 10 | Cap total de partes por request. |
| `url_download_timeout_secs` | 30 | Timeout HEAD + connect/headers del GET. |
| `allow_http_urls` | false | Permite `http://` plano (off por seguridad). |

### Ejemplo — Subir archivos al KB de ADP (como LLM tool)

```json
"tool_configurations": {
  "upload_to_kb": {
    "node_type": "http_request",
    "node_schema": {
      "base_url":      { "fixed": "${ADP_API_BASE_URL}" },
      "endpoint":      { "type": "string", "required": true, "description": "/knowledge-bases/<kb_id>/documents" },
      "method":        { "fixed": "POST" },
      "headers":       { "fixed": { "Content-Type": "multipart/form-data" } },
      "authorization": { "fixed": "Bearer ${ADP_SESSION_TOKEN}" },
      "body": {
        "files": {
          "type": "array",
          "items": { "type": "string", "description": "Signed URL o $attachment:<storage_key>" },
          "required": true,
          "description": "Archivos a subir"
        }
      }
    }
  }
}
```

El LLM solo ve `endpoint` y `files`. Todo lo demás (método, auth, content-type) es invisible para el modelo.

### Política de errores

- All-or-nothing: si una parte falla validación (oversize, HEAD error, scheme no permitido, attachment no encontrado, max_parts excedido), **no se envía nada al downstream**.
- Stream interrumpido mid-flight: el downstream recibe una request incompleta y la rechaza; el nodo retorna `StreamInterrupted`. Sin reintentos automáticos en v1 — el loop del LLM puede reintentar.

### Ver también

- Spec de diseño: [`docs/superpowers/specs/2026-05-24-http-multipart-streaming-design.md`](../superpowers/specs/2026-05-24-http-multipart-streaming-design.md)
- Test graph runnable: `tests/graphs/external/multipart_upload.json`
````

- [ ] **Step 5: Update `docs/agent_context/node_ports_reference.md`**

Open `docs/agent_context/node_ports_reference.md`. Find the `http_request` entry. After its existing ports/outputs documentation, add:

```markdown

**Multipart mode** (activado por header `Content-Type: multipart/*`):
- El campo `body` se interpreta como un map de parts (string URL/`$attachment:`/texto, array, o objeto explícito con `url`/`attachment`/`value` + overrides opcionales `filename`/`content_type`).
- Config extra: `max_file_size_bytes` (def. 100MB), `max_parts` (def. 10), `url_download_timeout_secs` (def. 30), `allow_http_urls` (def. false).
- Output port `body` y `status` igual que en JSON mode.
```

- [ ] **Step 6: Link from the DEVELOPER_GUIDE index**

Open `docs/DEVELOPER_GUIDE.md`. Find the line that references `25_web_nodes.md`. If the index lists section subheadings, add a note that section 25 now covers "Subida de archivos por multipart". If the index only lists section titles, no change needed.

- [ ] **Step 7: Verify docs are consistent**

Run a quick grep:
```bash
grep -n "multipart" docs/node_configurations.json docs/developer_guide/25_web_nodes.md docs/agent_context/node_ports_reference.md
```
Expected: every file has at least one mention.

- [ ] **Step 8: Commit**

```bash
git add tests/graphs/external/multipart_upload.json docs/node_configurations.json docs/developer_guide/25_web_nodes.md docs/agent_context/node_ports_reference.md docs/DEVELOPER_GUIDE.md
git commit -m "docs(http_request): document multipart mode + canonical KB upload example

Adds runnable test graph at tests/graphs/external/multipart_upload.json"
```

---

## Task 11: Final verification

**Goal:** Make sure nothing else in the workspace broke (especially other nodes that consume `OutputStorageRepository`).

- [ ] **Step 1: Run the full test suite**

Run: `cargo test --verbose -p colmena_dag_engine`
Expected: ALL PASS — unit + integration + doctests.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p colmena_dag_engine --all-targets -- -D warnings`
Expected: 0 warnings. (Project has `[lints.rust] warnings = "deny"`, so any warning would block.)

- [ ] **Step 3: Format**

Run: `cargo fmt -p colmena_dag_engine`
Expected: no diff, or apply diff and commit it.

- [ ] **Step 4: Sweep against ADP worker (breaking-change discipline)**

Per `CLAUDE.md`, anything that changes colmena's public API needs a sweep against the ADP worker. The `read_stream` method is additive (existing `read` is untouched), so existing ADP usage compiles. Still:

```bash
ls /Users/danielgarcia/startti/adp/apps/service/ia/platform/{worker,api}/src/ 2>/dev/null
grep -rn "OutputStorageRepository" /Users/danielgarcia/startti/adp/apps/service/ia/platform/ 2>/dev/null
```

Expected: ADP doesn't directly impl the trait (it consumes colmena via Cargo), so no source changes needed. If grep finds direct impls, sweep them with a stub `read_stream` impl in the same style as Task 1.

- [ ] **Step 5: Update `CLAUDE.md` status line**

Open `CLAUDE.md`. Find the `## Current Status` section. Append a note:

```markdown
- **HTTP multipart streaming shipped 2026-05-24** — `http_request` node now supports `Content-Type: multipart/form-data` with URL-sourced and `$attachment:<key>`-sourced parts, both streamed end-to-end. `OutputStorageRepository` extended with additive `read_stream` method. See [`docs/developer_guide/25_web_nodes.md`](docs/developer_guide/25_web_nodes.md#subida-de-archivos-por-multipart-http_request) and the spec at [`docs/superpowers/specs/2026-05-24-http-multipart-streaming-design.md`](docs/superpowers/specs/2026-05-24-http-multipart-streaming-design.md).
```

Commit:
```bash
git add CLAUDE.md
git commit -m "docs(CLAUDE): note multipart streaming shipped"
```

- [ ] **Step 6: Manual smoke against ADP dev (optional, recommended)**

If you have an ADP dev session token + an actual KB id, create a tiny graph (similar to `tests/graphs/external/multipart_upload.json` but pointing at `${ADP_API_BASE_URL}/knowledge-bases/<kb_id>/documents`) and run it:

```bash
source .env
cargo run --bin dag_engine -- run /tmp/kb_upload_smoke.json --agent-session-id smoke_multipart_001
```

Expected: HTTP 200, KB returns the documents array. Verify documents appear via `GET /knowledge-bases/<kb_id>/documents`.

---

## Out of scope reminders (from spec)

These will NOT be done in this plan:

- Mid-stream retries on connection drops
- IP allowlists / private network blocking
- `allow_unverified_size` opt-in for providers that don't support HEAD
- Response body streaming (this plan covers REQUEST streaming only)
- Per-tool override of limits via input (only via config)
- RFC 5987 (`filename*=UTF-8''...`) Content-Disposition parsing
