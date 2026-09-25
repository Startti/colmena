# src/libs/colmena/src/storage/domain/output_storage_repository.rs

**Layer:** domain  **Purpose:** Defines the storage port trait (`OutputStorageRepository`) and value objects for persisting and retrieving generated output media (images, audio files, etc.) with session context and streaming support.

## Symbols

- `StoreRequest` (struct, pub) — Request to persist generated artifact with bytes, MIME type, filename, and session identifiers for path derivation in adapters
- `StoredOutput` (struct, pub) — Handle returned after successful store operation, containing canonical storage_key, read_url, MIME type, filename, and size
- `StoredBytes` (struct, pub) — Bytes plus minimal metadata returned by read() method, used for cross-provider lazy upload and `$attachment:<id>` placeholder resolution
- `StoredStream` (struct, pub) — Streaming counterpart to StoredBytes for multipart uploads, yielding Bytes + error from async stream without buffering full payload
- `Debug` impl for `StoredStream` (impl, pub) — Manual Debug formatter that elides the Pin<Box<dyn Stream>> field to render as readable `<async stream>` placeholder
- `OutputStorageRepository` (trait, pub) — Port for persisting and retrieving generated output media; two shipping adapters are `LocalCacheStorageAdapter` and `HttpCallbackStorageAdapter`
- `store()` (async fn, pub trait method) — Persist request bytes and return stable handle with fetchable read URL
- `read()` (async fn, pub trait method) — Retrieve bytes for previously-stored output, supporting cross-provider lazy upload and `$attachment:<id>` attachment resolution; returns `StorageError::InvalidInput` for unknown keys
- `read_stream()` (async fn, pub trait method) — Streaming variant of read() for multipart mode; implementations must yield bytes in order with accurate size_bytes metadata; returns `StorageError::InvalidInput` (unknown key) or `StorageError::BackendUnavailable` (source unreachable)
- `delete()` (async fn, pub trait method) — Idempotent blob deletion (returns Ok whether blob existed or not); used by attachment_gc garbage collection; bubbles `StorageError::BackendUnavailable` on backend failure for retry
- `read_url()` (async fn, pub trait method, DEFAULT-provided since v0.20.0 / Feature C part 1) — On-demand read URL for an existing `storage_key`, hinted to last `ttl_seconds`; default `Ok(None)` so a pre-existing implementer compiles and runs unchanged. Signing never lives in this library — a host overrides this to call its own signing protocol.
- `supports_read_url()` (fn, pub trait method, DEFAULT-provided since v0.20.0 / Feature C part 1) — Capability hint for the `$attachment_url:` teaching gate (part 2, not yet implemented); default `false`, kept in sync with `read_url`'s default.

## File-level notes

- Clean domain trait file with zero infrastructure dependencies
- No unfinished code, dead symbols, or obvious improvements
- All trait methods are properly async and bounded Send + Sync, except `supports_read_url` which is a plain sync fn — `#[async_trait]` leaves non-`async fn` trait methods untouched, and mixing the two on one trait is an existing pattern elsewhere in this codebase (e.g. `SkillRepository`)
- Error handling via StorageError is consistent across all methods
- Documentation is comprehensive and links use cases accurately
- `read_url` / `supports_read_url` are additive (default-provided): `LocalCacheStorageAdapter` and `HttpCallbackStorageAdapter` do not override either and keep the default; only `LocalHttpStorageAdapter` overrides both
