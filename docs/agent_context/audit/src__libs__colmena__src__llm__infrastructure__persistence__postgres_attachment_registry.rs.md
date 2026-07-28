# src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs

**Layer:** infrastructure  
**Purpose:** PostgreSQL adapter implementing AttachmentRegistry trait for persistent storage of conversation attachment metadata (document IDs, provider file IDs, MIME types, timestamps, storage keys, and lifecycle tracking).

## Symbols

- `PostgresAttachmentRegistry` (struct, pub) — Wraps an Arc<PgPool> to provide trait impl for attachment CRUD operations
- `PostgresAttachmentRegistry::new` (fn, pub async) — Acquires or creates a pooled connection via PgPoolRegistry and constructs Self
- `row_to_attachment` (fn, private async) — Converts a PostgreSQL row to ConversationAttachment by parsing provider enum, source kind/value, optional Plan A columns (storage_key, origin, last_used_at), and timestamps
- `AttachmentRegistry` (impl) — Trait impl for PostgresAttachmentRegistry providing all trait methods
- `upsert` (fn, pub async) — INSERT with ON CONFLICT DO UPDATE, merging new provider_file_id/metadata while preserving existing label/description/storage_key/origin if not overridden
- `lookup` (fn, pub async) — SELECT by (agent_session_id, document_id, provider) exact match, returns Option
- `refresh_provider_file_id` (fn, pub async) — UPDATE provider_file_id and refreshed_at where (agent_session_id, document_id, provider), errors NotFound if no rows affected
- `list_for_session` (fn, pub async) — SELECT all attachments for session_id, ordered ascending by registered_at
- `update_description` (fn, pub async) — UPDATE description field by (agent_session_id, document_id, provider), errors NotFound if no rows affected
- `lookup_by_document_id` (fn, pub async) — SELECT most recent by (agent_session_id, document_id) ignoring provider, ordered DESC refreshed_at then ASC provider for determinism
- `touch_last_used` (fn, pub async) — UPDATE last_used_at=NOW() for ALL rows matching (agent_session_id, document_id) across all providers, no-op if none exist
- `find_stale_attachments` (fn, pub async) — SELECT rows where COALESCE(last_used_at, registered_at) < cutoff, ordered ASC, limited to query.limit
- `delete_attachment` (fn, pub async) — DELETE all rows for (agent_session_id, document_id), idempotent (no error if none exist)
- `tests` (mod, cfg(test)) — 11 integration tests covering upsert, lookup, refresh, update_description, lookup_by_document_id, touch_last_used, find_stale_attachments, delete_attachment; all marked #[ignore] for database-dependent gating

## File-level notes

- **Defensive optional-column handling** (lines 87–96): storage_key, origin, and last_used_at use `.ok().flatten()` to gracefully handle NULL or missing columns on legacy rows (Plan A migration safety).
- **Deterministic ordering** in `lookup_by_document_id` (line 269): secondary sort by provider ASC ensures stable winner when multiple rows share same refreshed_at.
- **Idempotent delete** (lines 330–340): does not error if no rows match, consistent with attachment GC lifecycle.
- **Multi-provider lifecycle**: `touch_last_used` updates all provider copies of a document in one call, reflecting the semantic "user accessed this doc" across all integrations.
- **Test coverage**: 11 tests (all #[ignore]-gated) cover core paths; tests use UUID-based session IDs to avoid concurrency conflicts in shared test DB.
- **No blocking issues**: SQL queries are straightforward sqlx patterns, error handling is uniform via AttachmentError domain type, no unimplemented stubs or TODO comments.
