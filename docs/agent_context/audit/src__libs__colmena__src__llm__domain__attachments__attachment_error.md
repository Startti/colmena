# src/libs/colmena/src/llm/domain/attachments/attachment_error.rs

**Layer:** domain  
**Purpose:** Defines error types for attachment-related domain operations, with standardized error messages for not-found, expiration, missing session, and repository failures.

## Symbols

- `AttachmentError` (pub enum) — Error type for attachment-related operations, derives Debug and thiserror::Error
  - `NotFound { document_id: String }` (variant) — Attachment not found in the session; renders document_id in message
  - `ExpiredUnrecoverable { document_id: String, reason: String }` (variant) — Attachment expired and cannot be re-uploaded; includes explanatory reason
  - `SessionMissing` (variant) — Agent session ID is missing from the run; indicates load_attachment requires stable agent session
  - `RepositoryFailed(String)` (variant) — Repository operation failed; wraps implementation error message
- `tests::not_found_renders_document_id_in_message` (test) — Verifies NotFound variant formats document_id into error message
- `tests::expired_unrecoverable_renders_reason` (test) — Verifies ExpiredUnrecoverable variant includes reason in error message

## File-level notes

- Clean, minimal error enum with no external dependencies beyond `thiserror`
- All variants have standardized Display messages via `#[error(...)]` attributes
- Test coverage limited to two message-rendering assertions; no variant construction or pattern-matching coverage
- No architectural complexity; pure domain-layer abstraction with no I/O or external coupling
