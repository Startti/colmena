# src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs

**Layer:** domain  
**Purpose:** Defines the `AttachmentRegistry` port trait for persistent attachment management across sessions, plus input DTOs for registry upsert and stale-attachment queries. Coordinates Plans A (storage keying) and C (garbage collection).

## Symbols

### Types & Structs
- `UpsertAttachmentInput` (struct, pub) — Input DTO mirrors `conversation_attachments` table columns 1:1 for insert/update operations.
  - `agent_session_id` (pub field, String) — Session identifier
  - `document_id` (pub field, String) — Unique document reference
  - `provider` (pub field, ProviderKind) — LLM provider kind
  - `provider_file_id` (pub field, String) — External provider's file ID
  - `mime_type` (pub field, String) — Content MIME type
  - `filename` (pub field, String) — Original filename
  - `size_bytes` (pub field, Option<u64>) — File size in bytes
  - `label` (pub field, Option<String>) — User-facing label
  - `description` (pub field, Option<String>) — Attachment description (updated after auto-summary)
  - `source` (pub field, AttachmentSource) — Attachment source variant
  - `storage_key` (pub field, Option<String>) — Plan A: reference to `OutputStorageRepository` storage key when caller persisted bytes
  - `origin` (pub field, Option<String>) — Provenance: `user_upload` or `generated_by:<tool>`

- `StaleAttachmentQuery` (struct, pub) — Query parameters for finding stale attachments in garbage collection.
  - `cutoff` (pub field, chrono::DateTime<chrono::Utc>) — Rows older than this timestamp are considered stale
  - `limit` (pub field, u32) — Maximum batch size; GC binary loops until query returns fewer rows

### Traits
- `AttachmentRegistry` (trait, pub) — Port trait for attachment registry persistence; mocked in tests via `#[cfg_attr(test, mockall::automock)]`.
  - `upsert(&self, input: UpsertAttachmentInput) -> Result<(), AttachmentError>` — Insert or update attachment entry, idempotent on (agent_session_id, document_id, provider).
  - `lookup(&self, agent_session_id: &str, document_id: &str, provider: ProviderKind) -> Result<Option<ConversationAttachment>, AttachmentError>` — Fetch single attachment entry; returns `Ok(None)` when not found.
  - `refresh_provider_file_id(&self, agent_session_id: &str, document_id: &str, provider: ProviderKind, new_provider_file_id: &str) -> Result<(), AttachmentError>` — Update provider file ID and refreshed_at timestamp; returns `Err(NotFound)` if row missing.
  - `update_description(&self, agent_session_id: &str, document_id: &str, provider: ProviderKind, description: &str) -> Result<(), AttachmentError>` — Replace description (used after auto-summary); returns `Err(NotFound)` if row missing.
  - `list_for_session(&self, agent_session_id: &str) -> Result<Vec<ConversationAttachment>, AttachmentError>` — List all attachments for a session (filtering by provider handled by caller).
  - `lookup_by_document_id(&self, agent_session_id: &str, document_id: &str) -> Result<Option<ConversationAttachment>, AttachmentError>` — Plan A: cross-provider lookup by (session, document); returns most recently refreshed row if multiple providers.
  - `touch_last_used(&self, agent_session_id: &str, document_id: &str) -> Result<(), AttachmentError>` — Plan A: update `last_used_at = now()` for matching rows; called on every successful attachment resolution.
  - `find_stale_attachments(&self, query: StaleAttachmentQuery) -> Result<Vec<ConversationAttachment>, AttachmentError>` — Plan C: find rows older than cutoff (up to limit); ordered by age ASC for deterministic batching.
  - `delete_attachment(&self, agent_session_id: &str, document_id: &str) -> Result<(), AttachmentError>` — Plan C: idempotent row deletion; caller must delete blob from `OutputStorageRepository` first.

## File-level notes
- **Well-documented contracts:** each method includes detailed comments explaining idempotency, error cases, and operational flow (Plans A/C).
- **No todos/unfished stubs:** all trait methods are complete specifications.
- **Operational notes on `last_used_at`:** explicitly clarifies that it is populated only by `touch_last_used`, not by SQL trigger; GC binary uses `COALESCE(last_used_at, registered_at)` to handle legacy rows without a touch.
- **Cross-provider semantics:** `lookup_by_document_id` and `touch_last_used` operate across all providers for a given document, supporting multi-provider lazy upload.
- **Idempotency contracts:** `upsert` and `delete_attachment` are explicitly idempotent; `refresh_provider_file_id` and `update_description` fail-closed (return `NotFound`).
