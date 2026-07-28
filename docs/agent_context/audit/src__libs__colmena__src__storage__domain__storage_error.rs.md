# src/libs/colmena/src/storage/domain/storage_error.rs

**Layer:** domain  
**Purpose:** Defines domain-level error types for storage operations (multimedia artifact persistence). Used by `OutputStorageRepository` implementations and callers.

## Symbols

- `StorageError` (enum, pub) — Enum representing all possible storage operation failures; implements `thiserror::Error` for automatic Display/Error impl.
- `StorageError::BackendUnavailable(String)` (variant) — Transport-level backend unreachability (DNS, TCP, TLS, or connection failure).
- `StorageError::InvalidInput(String)` (variant) — Malformed request before backend contact (empty bytes, missing metadata).
- `StorageError::UploadFailed(String)` (variant) — Upload step failure (non-2xx HTTP PUT to signed URL or equivalent).
- `StorageError::CallbackFailed { status: u16, body: String }` (variant) — Callback endpoint returned non-success HTTP status; includes parsed status code (0 if JSON parse failed) and response body.

## File-level notes

- Clean, minimal error enum with four distinct failure modes, each with specific use-case documentation.
- No infrastructure dependencies; uses only `thiserror` which is appropriate for domain errors per project conventions.
- All variants are tuple or named-field enums with descriptive error messages for user-facing output.
- No unfinished code, dead code, or clarity issues detected.
