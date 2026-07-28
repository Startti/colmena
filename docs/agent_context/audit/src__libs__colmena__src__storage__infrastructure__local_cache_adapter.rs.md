# src/libs/colmena/src/storage/infrastructure/local_cache_adapter.rs

**Layer:** infrastructure  **Purpose:** In-process storage adapter backed by a DashMap for local testing and CLI runs. Implements OutputStorageRepository to store, retrieve, and delete bytes without filesystem or network I/O.

## Symbols

- `LocalCacheStorageAdapter` (struct, pub) — In-process storage adapter retaining bytes in an Arc<DashMap> keyed by storage_key.
- `cache` (field, private) — Thread-safe in-process cache: Arc<DashMap<String, StoredBytes>>.
- `LocalCacheStorageAdapter::new()` (fn, pub) — Constructor; initializes adapter with empty DashMap.
- `LocalCacheStorageAdapter::default()` (fn, pub) — Default trait impl; delegates to new().
- `OutputStorageRepository::store()` (async fn, pub) — Validates input (non-empty bytes, non-empty mime_type), generates UUID-based storage_key, caches StoredBytes, returns StoredOutput with read_url == storage_key.
- `OutputStorageRepository::read()` (async fn, pub) — Retrieves StoredBytes by key from cache; returns InvalidInput error if not found.
- `OutputStorageRepository::read_stream()` (async fn, pub) — Wraps cached StoredBytes in a single-chunk async stream; explicitly drops cache guard before await point to avoid lock contention.
- `OutputStorageRepository::delete()` (async fn, pub) — Idempotent removal of entry from cache; no-op on missing keys.
- `req()` (fn, private test helper) — Constructs a StoreRequest with test values (bytes, mime_type, filename, no session_id).
- `store_returns_short_handle_as_url_not_data_uri()` (test) — Verifies storage_key is used as read_url and does not start with "data:".
- `rejects_empty_bytes()` (test) — Verifies store() rejects zero-length byte payload.
- `rejects_empty_mime()` (test) — Verifies store() rejects whitespace-only mime_type.
- `distinct_calls_yield_distinct_storage_keys()` (test) — Verifies each store() call generates a unique UUID-based key.
- `read_returns_stored_bytes_for_known_key()` (test) — Verifies read() retrieves correct bytes and metadata.
- `read_unknown_key_errors()` (test) — Verifies read() returns InvalidInput error for missing keys.
- `read_stream_returns_single_chunk_with_correct_metadata()` (test) — Verifies read_stream() returns stream with correct size_bytes, mime_type, filename and single chunk.
- `read_stream_unknown_key_errors()` (test) — Verifies read_stream() returns InvalidInput error for missing keys.
- `delete_removes_stored_blob_and_is_idempotent()` (test) — Verifies delete() removes entries and subsequent delete() is a no-op.
- `delete_unknown_key_is_noop()` (test) — Verifies delete() succeeds on non-existent keys.

## File-level notes

- Clean, straightforward implementation with no complexity debt.
- All public methods are trait implementations for OutputStorageRepository; no orphaned public functions.
- Error handling is appropriate: explicit validation of input constraints (empty bytes, empty mime) with descriptive messages.
- Explicit `drop(entry)` in `read_stream()` (line 113) is a deliberate guard to release DashMap guard before await points — good practice, not a code smell.
- Test coverage is comprehensive: validations, happy paths, not-found paths, idempotency, distinct keys, and metadata correctness.
- No todos, unimplemented!(), or stub implementations.
- Intentional design choice to return `storage_key` as `read_url` (not a data: URI) to keep LLM tool-results compact — documented in file header and test assertions.
