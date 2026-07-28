# src/libs/colmena/src/llm/domain/attachments/mod.rs

**Layer:** domain  
**Purpose:** Module aggregator and re-export hub for attachment domain types, registries, and utilities. Provides well-known origin constants for attachment source tracking.

## Symbols

### Public Modules (re-exports)
- `attachment_error` (mod, pub) — Error type definitions for attachment operations
- `attachment_registry` (mod, pub) — Registry traits and input types for attachment persistence
- `auto_id` (mod, pub) — Attachment ID generation utility
- `conversation_attachment` (mod, pub) — Core `ConversationAttachment` value object and `AttachmentSource` enum
- `stream_resolver` (mod, pub) — Stream resolution and error types for attachment content fetching
- `summary_generator` (mod, pub) — Summary generation logic and configuration for attachment content

### Public Re-exports
- `AttachmentError` (type, pub) — Re-export from attachment_error
- `AttachmentRegistry` (type, pub) — Re-export from attachment_registry
- `StaleAttachmentQuery` (type, pub) — Re-export from attachment_registry
- `UpsertAttachmentInput` (type, pub) — Re-export from attachment_registry
- `generate_attachment_id` (fn, pub) — Re-export from auto_id
- `AttachmentSource` (enum, pub) — Re-export from conversation_attachment
- `ConversationAttachment` (struct, pub) — Re-export from conversation_attachment
- `AttachmentResolveError` (type, pub) — Re-export from stream_resolver
- `AttachmentStreamResolver` (type, pub) — Re-export from stream_resolver
- `AttachmentSummaryGenerator` (type, pub) — Re-export from summary_generator
- `SummaryConfig` (type, pub) — Re-export from summary_generator
- `SummaryError` (type, pub) — Re-export from summary_generator
- `SummaryInput` (type, pub) — Re-export from summary_generator
- `SummaryOutcome` (type, pub) — Re-export from summary_generator
- `SummarySource` (type, pub) — Re-export from summary_generator

### Origin Constant Management Module
- `origin` (mod, pub) — Well-known constants and factory for attachment origin tracking
- `origin::USER_UPLOAD` (const, pub) — Constant for user-uploaded attachments (`"user_upload"`)
- `origin::generated_by(tool_name: &str)` (fn, pub) — Factory to format origin as `generated_by:<tool_name>` for tool-generated attachments

### Tests
- `origin::tests` (mod, cfg(test)) — Unit tests validating origin constant values and formatting

## File-level notes

- This is a clean, well-organized module aggregator with no executable code beyond re-exports.
- The `origin` module implements a grep-friendly constant registry pattern, preventing hardcoded origin strings at call sites.
- All tests pass (two assertions validate `USER_UPLOAD` and `generated_by` formatting for multiple tools).
- No architectural violations, dead code, or unfinished work detected.
- All public symbols are intentionally exported for use throughout the LLM domain and application layers.
