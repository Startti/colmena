# src/libs/colmena/src/storage/infrastructure/local_http_adapter.rs

**Layer:** infrastructure  **Purpose:** Dev-mode storage adapter that writes bytes to disk and serves them via an embedded axum HTTP server on 127.0.0.1. Returns HTTP URLs (not `data:` URIs) to mirror production behavior in development.

## Symbols

- `mime_from_filename` (fn, private) — maps file extensions to MIME types for Content-Type header
- `ext_from_mime` (fn, private) — reverse mapping: MIME type to file extension for filename synthesis
- `ServerState` (struct, private) — holds directory path for axum route handler state
- `serve_file` (fn, private async) — axum route handler for GET /files/:key with path-traversal guards and dynamic MIME type
- `LocalHttpStorageAdapter` (struct, pub) — main public struct wrapping server lifecycle and storage directory
- `LocalHttpStorageAdapter::new` (pub async fn) — constructor: creates directory, binds axum server, spawns Tokio task, returns adapter with bound port
- `LocalHttpStorageAdapter::port` (pub fn) — returns the actual bound port (useful when 0 was requested)
- `LocalHttpStorageAdapter::dir` (pub fn) — returns reference to storage directory path
- `Drop` impl for `LocalHttpStorageAdapter` (private) — graceful shutdown trigger via oneshot channel take()
- `OutputStorageRepository::store` (pub async fn) — validates input, writes to disk, synthesizes UUID+ext storage_key, returns StoredOutput with HTTP URL
- `OutputStorageRepository::read` (pub async fn) — reads file from disk with path-traversal validation
- `OutputStorageRepository::read_stream` (pub async fn) — reads file as async stream with metadata (size, mime, filename)
- `OutputStorageRepository::delete` (pub async fn) — deletes file, idempotent (no-op if missing)
- `tests` (mod, private) — 11 comprehensive tests covering store/read/read_stream/delete, HTTP server behavior, error cases, path traversal

## File-level notes

- **Duplication**: Path-traversal validation (`contains('/')`, `contains("..")`, `is_empty()`) repeated identically in 4 methods (serve_file line 95, read line 228, read_stream line 254, delete line 285). Should extract to private helper `validate_storage_key(key: &str) -> Result<(), StorageError>` to reduce duplication and ensure consistency.

- **Type ergonomics**: `dir()` method returns `&PathBuf` (line 175); should be `&Path` for better caller ergonomics per Rust conventions.

- **Whitespace handling inconsistency**: `store()` validates `mime_type` by trimming (line 198: `if req.mime_type.trim().is_empty()`), but the untrimmed `req.mime_type` is passed directly to `ext_from_mime()` (line 207). If caller passes whitespace-padded MIME like `" image/png "`, validation passes but `ext_from_mime()` exact-matches fail and defaults to `".bin"`. Should either trim at input or document expectation. Recommend trimming `req.mime_type` during validation and storing the trimmed version.

- **Test coverage**: Excellent — happy path (store/read/read_stream/delete), HTTP server behavior, 404 handling, path traversal attacks, empty input, idempotent delete, all covered with `#[tokio::test]` async tests using `tempfile::TempDir`.

- **Error semantics**: `read()` uses `StorageError::InvalidInput` for both malformed keys and missing files (line 235). Error messages distinguish them clearly ("invalid storage_key" vs "not readable at <path>"), so callers can infer the root cause, but a stricter audit might suggest separate error variants. Not flagged — acceptable for current error model.

- **Shutdown pattern**: Uses `Mutex<Option<oneshot::Sender<()>>>` in `Drop` to safely signal graceful shutdown without `&mut self`. Pattern is sound; `lock()` failure silently ignored (line 182), acceptable given no other code holds lock.
