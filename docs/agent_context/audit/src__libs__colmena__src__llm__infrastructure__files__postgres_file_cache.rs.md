# src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs

**Layer:** infrastructure  
**Purpose:** Postgres adapter for file cache repository; manages cached file metadata with UPDATE...RETURNING optimization for cache hits and upsert semantics (always touches `last_used_at` on conflict).

## Symbols

- `PostgresFileCache` (struct, pub) — Wrapper holding Arc<PgPool> for Postgres file cache operations
- `parse_provider_from_row` (fn, private) — Converts provider string from DB row to ProviderKind enum; fails fast with detailed error (document_id + provider value) on corruption instead of silent fallback
- `PostgresFileCache::new` (async fn, pub) — Creates new PostgresFileCache by acquiring or creating a pool from PgPoolRegistry
- `FileCacheRepository::lookup` (async fn) — Updates `last_used_at` and returns cached file entry; uses UPDATE...RETURNING to combine update and read in one query; returns None on miss
- `FileCacheRepository::upsert` (async fn) — Inserts or updates cache entry via ON CONFLICT; always sets `last_used_at = NOW()` on update regardless of passed value to treat every upsert as a cache touch
- `FileCacheRepository::invalidate` (async fn) — Deletes a cache entry by document_id and provider
- `tests::with_cache` (async fn) — Test helper that creates a PostgresFileCache instance from TEST_DATABASE_URL; generates unique prefix per test for parallel-safe cleanup; runs migrations explicitly
- `tests::fixture` (fn) — Creates a sample CachedFileEntry with Anthropic provider and PDF metadata
- `tests::lookup_miss_returns_none` (async test) — Verifies lookup returns None for non-existent document
- `tests::upsert_then_lookup_returns_entry` (async test) — Round-trip: upsert an entry and verify lookup returns it
- `tests::upsert_twice_updates_and_touches_last_used_at` (async test) — Confirms that second upsert updates fields and overwrites passed `last_used_at` with NOW()
- `tests::lookup_advances_last_used_at_on_cache_hit` (async test) — Verifies that lookup updates `last_used_at` via UPDATE...RETURNING and does not mutate other fields
- `tests::parse_provider_from_row_accepts_known_kinds` (test) — Validates all known ProviderKind variants round-trip through string parsing
- `tests::parse_provider_from_row_fails_on_corrupted_string` (test) — Confirms corrupted provider string fails with LlmError::RequestFailed (not silent fallback) and includes provider + document_id in message
- `tests::invalidate_removes` (async test) — Verifies invalidate deletes cache entry and subsequent lookup returns None

## File-level notes

- **Good error propagation design:** `parse_provider_from_row` explicitly logs and returns detailed error on provider corruption, preventing silent failures that obscure invalid cache state.
- **Boilerplate opportunity (lines 84–128):** Eight consecutive field extractions from row (`document_id`, `provider`, `provider_file_id`, `mime_type`, `filename`, `size_bytes`, `uploaded_at`, `expires_at`, `last_used_at`) use identical error handling. Could be reduced with a helper macro or generic extract function to improve maintainability.
- **Query strategy rationale:** UPDATE...RETURNING used for cache hits (single query vs. SELECT + UPDATE) to combine touch and read atomically; trade-off (row lock vs. share lock) is acceptable due to low concurrency per (document_id, provider) tuple.
- **Upsert semantics clear:** ON CONFLICT always sets `last_used_at = NOW()` while preserving passed `last_used_at` on initial INSERT — test confirms this behavior (`upsert_twice_updates_and_touches_last_used_at`).
- **Test parallelization:** Unique prefix per test execution prevents race conditions in CI and local parallel runs; migrations run explicitly to ensure schema exists.
- **All tests properly marked `#[ignore]`** with TEST_DATABASE_URL requirement and run instructions documented in comment.
