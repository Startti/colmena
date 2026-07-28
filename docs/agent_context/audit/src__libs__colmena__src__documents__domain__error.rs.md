# src/libs/colmena/src/documents/domain/error.rs

**Layer:** domain  
**Purpose:** Defines error types for the documents module: storage, indexing, rendering, document versioning, and asset operations. All error variants use `thiserror` for display formatting and `serde` for JSON serialization.

## Symbols

- `StorageError` (enum, pub) — Storage operation errors with variants: NotFound, PermissionDenied, PreconditionFailed, Transient, Backend
- `IndexError` (enum, pub) — Index operation errors with variants: NotFound, Backend
- `RenderError` (enum, pub) — Render operation errors with single variant: Failed
- `ConflictDetail` (struct, pub) — Captures details of a version conflict: incoming_op, conflicting_with, in_version, reason
- `DocumentError` (enum, pub) — Primary domain error type for document operations; variants: ArtifactNotFound, VersionNotFound, VersionConflict (with ConflictDetail), IRValidationFailed, InvalidPatchOp, RenderFailed, Storage (from StorageError), Index (from IndexError), SessionIsolationViolation
- `impl From<RenderError> for DocumentError` — Converts RenderError to DocumentError::RenderFailed by calling `.to_string()` on the source error
- `AssetError` (enum, pub) — Asset operation errors with variants: NotFound, MimeNotAllowed, TooLarge, StillReferenced (with by_artifacts vector), Storage
- `tests::error_display` (test fn) — Verifies DocumentError::ArtifactNotFound displays correctly via to_string()
- `tests::storage_into_document_error` (test fn) — Verifies StorageError can be converted to DocumentError via Into trait

## File-level notes

- All public error enums use `#[derive(Debug, Error, Serialize)]` to support both display formatting and JSON serialization for API responses.
- `ConflictDetail` struct uses `serde_json::Value` for arbitrary operation payloads; appropriate for CRDT conflict representation.
- `DocumentError::VersionConflict` includes an `artifact` field that is not used in the error message template—may be intentional for programmatic access.
- `AssetError::StillReferenced` error message uses inline format string with `by_artifacts.len()` placeholder; displays count of referencing artifacts dynamically.
- Clean separation of concerns: StorageError and IndexError are specialized infrastructure errors wrapped into DocumentError via `#[from]`; RenderError has explicit From impl.
- Tests are minimal but cover happy paths (error display and error conversion); no edge cases tested.
