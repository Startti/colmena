# src/libs/colmena/src/llm/infrastructure/attachments/stream_resolver_impl.rs

**Layer:** infrastructure  **Purpose:** Production implementation of `AttachmentStreamResolver` trait, composing an `AttachmentRegistry` (catalog of document metadata) and `OutputStorageRepository` (byte retrieval) to resolve `(agent_session_id, document_id)` tuples to readable byte streams with two-path fallback strategy.

## Symbols

- `AttachmentStreamResolverImpl` (struct, pub) — Production resolver composing registry + storage adapters; shared via Arc across nodes and concurrent DAG runs
- `AttachmentStreamResolverImpl::new()` (fn, pub) — Constructor accepting registry and storage as `Arc<dyn _>` to enable shared multi-node access
- `AttachmentStreamResolver` impl for `AttachmentStreamResolverImpl` (impl, pub) — Async trait implementation
  - `resolve()` (fn, async) — Two-path resolution: (1) lookup document_id in registry and stream from storage if storage_key present; (2) fallback to treating document_id as raw storage_key for backward compat; returns `NotFound` if both paths fail; non-fatal errors on `touch_last_used` are logged as warn
- `tests::make_stream()` (fn, private) — Helper to construct `StoredStream` from static bytes with mime/filename metadata for test fixtures
- `tests::base_upsert()` (fn, private) — Helper to construct `UpsertAttachmentInput` with common fields for test cases
- `tests::resolve_via_document_id_uses_storage_key_from_registry()` (test, async) — Verifies registry lookup path succeeds and triggers `touch_last_used` side effect
- `tests::resolve_falls_back_to_raw_storage_key_when_lookup_misses()` (test, async) — Verifies backward-compat fallback to raw storage_key when registry returns None
- `tests::resolve_returns_not_found_when_both_paths_miss()` (test, async) — Verifies `StorageError::InvalidInput` is mapped to `AttachmentResolveError::NotFound`
- `tests::resolve_returns_storage_key_missing_when_row_has_no_storage_key()` (test, async) — Verifies error when registry row exists but storage_key is None (pre-Plan-A legacy case)

## File-level notes

- Code is clean and complete; no todos, unreachable, or stub implementations
- Error handling is defensive: Path 1 checks for missing storage_key before attempting read; Path 2 distinguishes `InvalidInput` (unknown key) from other storage errors
- `touch_last_used` failure is intentionally non-fatal and logged, preventing transient registry errors from blocking stream reads
- Test coverage is comprehensive, covering happy path (registry + storage), fallback (raw key), and both error conditions (missing key, missing storage_key field)
- Four-test suite uses `SqliteAttachmentRegistry` (in-memory) + `MockOutputStorageRepository` to keep tests hermetic
