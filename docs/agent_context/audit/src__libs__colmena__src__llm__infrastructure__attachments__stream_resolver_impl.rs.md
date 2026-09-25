# src/libs/colmena/src/llm/infrastructure/attachments/stream_resolver_impl.rs

**Layer:** infrastructure  **Purpose:** Production implementation of `AttachmentStreamResolver` trait, composing an `AttachmentRegistry` (catalog of document metadata) and `OutputStorageRepository` (byte retrieval) to resolve `(agent_session_id, document_id)` tuples to readable byte streams; an id the session registry does not know is `NotFound` and never read as a raw storage_key.

## Symbols

- `AttachmentStreamResolverImpl` (struct, pub) — Production resolver composing registry + storage adapters; shared via Arc across nodes and concurrent DAG runs
- `AttachmentStreamResolverImpl::new()` (fn, pub) — Constructor accepting registry and storage as `Arc<dyn _>` to enable shared multi-node access
- `AttachmentStreamResolver` impl for `AttachmentStreamResolverImpl` (impl, pub) — Async trait implementation
  - `resolve()` (fn, async) — Looks up `(agent_session_id, document_id)` in the registry and streams the row's storage_key from storage; a registry miss is `NotFound` (the error tells the model to use a document_id from the attachments catalog) and storage is not called; non-fatal errors on `touch_last_used` are logged as warn
- `tests::make_stream()` (fn, private) — Helper to construct `StoredStream` from static bytes with mime/filename metadata for test fixtures
- `tests::base_upsert()` (fn, private) — Helper to construct `UpsertAttachmentInput` with common fields for test cases
- `tests::resolve_via_document_id_uses_storage_key_from_registry()` (test, async) — Verifies registry lookup path succeeds and triggers `touch_last_used` side effect
- `tests::resolve_never_reads_an_id_the_session_registry_does_not_know()` (test, async) — A raw key of this or another session, another session's document_id and an unknown id are all `NotFound`; storage is never called
- `tests::resolve_returns_storage_key_missing_when_row_has_no_storage_key()` (test, async) — Verifies error when registry row exists but storage_key is None (pre-Plan-A legacy case)

## File-level notes

- Code is clean and complete; no todos, unreachable, or stub implementations
- Error handling is defensive: checks for a missing storage_key before attempting the read
- `touch_last_used` failure is intentionally non-fatal and logged, preventing transient registry errors from blocking stream reads
- Test coverage: happy path (registry + storage), ids that are not the session's (never read), missing storage_key field
- Three-test suite uses `SqliteAttachmentRegistry` (in-memory) + `MockOutputStorageRepository` to keep tests hermetic
