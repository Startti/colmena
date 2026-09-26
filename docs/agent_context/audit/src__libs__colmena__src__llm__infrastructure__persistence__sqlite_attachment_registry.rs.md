# src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs

**Layer:** infrastructure  
**Purpose:** SQLite-backed repository adapter for conversation attachment persistence, implementing upsert, lookup, refresh, and stale attachment detection for garbage collection.

## Symbols

- `SqliteAttachmentRegistry` (struct, pub) — wraps Arc<SqlitePool> for shared async pool access
- `SqliteAttachmentRegistry::new` (fn, pub async) — creates and connects a new registry, runs migrations
- `SqliteAttachmentRegistry::from_pool` (fn, pub) — constructs registry from an existing pool
- `AttachmentRegistry::upsert` (fn, pub async) — INSERT ON CONFLICT upsert of attachment metadata
- `AttachmentRegistry::lookup` (fn, pub async) — fetches attachment by (agent_session_id, document_id, provider)
- `AttachmentRegistry::refresh_provider_file_id` (fn, pub async) — updates provider_file_id, errors if row missing
- `AttachmentRegistry::list_for_session` (fn, pub async) — returns all attachments for a session ordered by registered_at
- `AttachmentRegistry::update_description` (fn, pub async) — updates description field, errors if row missing
- `AttachmentRegistry::lookup_by_document_id` (fn, pub async) — returns one attachment across providers: a row with neither storage_key nor origin (a lazy provider upload) last, then refreshed_at DESC, provider ASC for deterministic ties
- `AttachmentRegistry::touch_last_used` (fn, pub async) — updates last_used_at timestamp; idempotent (no-op if missing)
- `AttachmentRegistry::find_stale_attachments` (fn, pub async) — returns attachments older than cutoff (uses COALESCE(last_used_at, registered_at)), respects limit
- `AttachmentRegistry::delete_attachment` (fn, pub async) — deletes by (agent_session_id, document_id); idempotent
- `parse_ts` (fn, private) — parses SQLite default format "YYYY-MM-DD HH:MM:SS" to DateTime<Utc>
- `parse_ts_opt` (fn, private) — tolerant parser for nullable columns; handles SQLite default and RFC3339/ISO-8601 formats, returns None on unparseable or NULL
- `row_to_attachment` (fn, private) — maps SqliteRow to ConversationAttachment, converting provider string, source_kind/source_value pairs, and optional columns
- `tests::make_registry` (fn, private async) — test helper creating in-memory SQLite registry
- `tests::upsert_then_lookup_roundtrip` (test, #[tokio::test]) — roundtrip test for upsert/lookup
- `tests::list_returns_only_session_rows_ordered` (test, #[tokio::test]) — verifies list_for_session filters by session and maintains order
- `tests::refresh_updates_provider_file_id` (test, #[tokio::test]) — verifies provider_file_id update
- `tests::refresh_returns_not_found_for_missing_row` (test, #[tokio::test]) — verifies error on missing row
- `tests::update_description_persists_value` (test, #[tokio::test]) — verifies description update
- `tests::update_description_missing_row_returns_not_found` (test, #[tokio::test]) — verifies error on missing row
- `tests::upsert_persists_storage_key_and_origin` (test, #[tokio::test]) — verifies Plan A additive columns
- `tests::upsert_preserves_storage_key_when_subsequent_call_omits_it` (test, #[tokio::test]) — verifies COALESCE behavior in ON CONFLICT UPDATE
- `tests::lookup_by_document_id_returns_row_when_present` (test, #[tokio::test]) — basic existence check
- `tests::lookup_by_document_id_returns_none_when_absent` (test, #[tokio::test]) — None on missing
- `tests::lookup_by_document_id_returns_most_recent_when_multiple_providers` (test, #[tokio::test]) — verifies deterministic tie-breaking by provider ASC
- `tests::lookup_by_document_id_prefers_the_row_that_holds_the_bytes` (test, #[tokio::test]) — a `Generated` row with storage_key wins over a newer lazy-upload provider row without one
- `tests::lookup_by_document_id_a_newer_keyless_upload_wins_over_an_older_key` (test, #[tokio::test]) — a newer keyless `user_upload` row wins over an older keyed row of another provider
- `tests::touch_last_used_updates_timestamp` (test, #[tokio::test]) — verifies timestamp update
- `tests::touch_last_used_is_noop_when_missing` (test, #[tokio::test]) — verifies idempotency
- `tests::find_stale_attachments_returns_rows_older_than_cutoff_sqlite` (test, #[tokio::test]) — Plan C: find stale by cutoff
- `tests::find_stale_attachments_respects_limit_sqlite` (test, #[tokio::test]) — Plan C: limit enforcement
- `tests::delete_attachment_removes_row_and_is_idempotent_sqlite` (test, #[tokio::test]) — Plan C: delete idempotency

## File-level notes

- **Async & concurrency**: Correctly uses Arc<SqlitePool> for shared access; all trait methods are async
- **Error handling**: All database operations wrap sqlx errors in domain AttachmentError variants
- **Schema compatibility**: Additive nullable columns (storage_key, origin, last_used_at) handled gracefully via `.ok().flatten()` patterns
- **Timestamp handling**: SQLite stores timestamps as text "YYYY-MM-DD HH:MM:SS" in UTC; `parse_ts_opt` accepts both SQLite format and RFC3339 for robustness
- **Ordering**: `lookup_by_document_id` sorts rows with neither a storage_key nor an origin (lazy provider uploads) last, then `refreshed_at DESC`, then `provider ASC` to break ties (SQLite second-resolution accuracy)
- **Idempotency**: `touch_last_used` and `delete_attachment` are idempotent (no error if missing); `refresh_provider_file_id` and `update_description` error on missing
- **Test coverage**: 18 tests total, comprehensive coverage of happy-path, edge cases (ties, missing rows), and Plan C (stale attachment detection)
