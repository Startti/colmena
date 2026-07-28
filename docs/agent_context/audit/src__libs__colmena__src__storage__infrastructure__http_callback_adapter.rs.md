# src/libs/colmena/src/storage/infrastructure/http_callback_adapter.rs

**Layer:** infrastructure  **Purpose:** HTTP callback adapter implementing the OutputStorageRepository port. Orchestrates signed-URL upload/download flow with the ADP worker API and maintains an in-process cache to satisfy reads without requiring a separate sign-get endpoint.

## Symbols

- `CachedMeta` (struct, private) — Cached blob metadata (read_url, mime_type, filename) stored in-process during store() to enable later read() calls without round-tripping through the API
- `HttpCallbackStorageAdapter` (struct, pub) — Main adapter struct holding the reqwest client, callback endpoint URL, shared secret, and in-process metadata cache
- `HttpCallbackStorageAdapter::new` (fn, pub) — Constructor creating a new adapter with callback URL and shared secret; initializes reqwest client and DashMap cache
- `HttpCallbackStorageAdapter::with_client` (fn, pub) — Test-only constructor allowing injection of a mock reqwest client (used with wiremock)
- `HttpCallbackStorageAdapter::delete_url` (fn, private) — Derives the delete-endpoint sibling URL by replacing `/sign-put` suffix with `/delete` on the callback URL
- `SignResponse` (struct, private) — Deserialized JSON response from the sign callback containing put_url, read_url, and storage_key
- `OutputStorageRepository::store` (async fn, impl) — Two-stage store: POST to callback for signed PUT URL, then PUT bytes to that URL; caches metadata for later reads  [FLAG: improvement — semantic mismatch (line 212, 258)]
- `OutputStorageRepository::read` (async fn, impl) — Fetches bytes from cached read_url; fails fast with InvalidInput if key not in cache (no cross-process support)  [FLAG: improvement — code duplication (line 190-200 vs 236-246)]
- `OutputStorageRepository::read_stream` (async fn, impl) — Streams bytes from cached read_url without buffering; mirrors read() metadata lookup and error handling  [FLAG: improvement — code duplication (line 190-200 vs 236-246)]
- `OutputStorageRepository::delete` (async fn, impl) — POST storage_key to sibling /delete endpoint; treats 404 as idempotent success and evicts the key from meta_cache
- `tests` module — Comprehensive test suite covering: happy path store→read, error mapping (callback 401, PUT 500), empty bytes validation, cache eviction on delete, streaming, and URL derivation fallback logic

## File-level notes

- **Unbounded in-process cache:** The `meta_cache` DashMap is never evicted except on explicit delete(), so it accumulates entries over the process lifetime. Explicitly acknowledged as acceptable (comment line 44-47: "Persists for the lifetime of the engine process. Acceptable because the read URL is what the API would re-sign anyway.") but worth monitoring in production for long-running instances with many unique storage keys.

- **Semantic error variant mismatch:** Lines 212 and 258 map GET request failures (reads) to `StorageError::UploadFailed`, which is semantically misleading despite the error message being accurate. Consider using `StorageError::BackendUnavailable` or a dedicated read error variant for clarity.

- **Code duplication:** Both `read()` and `read_stream()` contain identical meta-cache lookup logic (lines 190-200 vs 236-246). Could be extracted into a private helper method `fn get_meta(&self, storage_key: &str) -> Result<CachedMeta, StorageError>`.

- **Cross-process read limitation:** The `read()` and `read_stream()` methods explicitly document (lines 196-198, 242-244) that cross-process reads are not supported — they require the key to be cached during the current process. This is a current design limitation, not a bug; a server-side sign-get endpoint would be needed to lift it.

- **Test coverage:** 11 well-structured async tests using wiremock, covering both success and error paths, cache behavior, and URL derivation edge cases.
