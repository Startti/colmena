# src/libs/colmena/src/crdt_documents/runtime.rs

**Layer:** infrastructure  **Purpose:** Bundler that builds and owns long-lived services for CRDT documents v1 feature. Constructs runtime from JSON config, handles storage backend selection (localfs/gcs), database setup with migrations, and provides lifecycle management (shutdown with flush guarantee).

## Symbols

- `DEFAULT_STORAGE_ROOT` (const) — Default storage directory path (`.colmena/crdt_documents`).
- `CrdtDocumentsRuntime` (struct, pub) — Bundler struct holding registry, storage, tracker, and change-tracker store; one per process, shared between HTTP server, LLM tool dispatchers, and Python bindings.
- `RuntimeError` (enum, pub) — Error type for runtime configuration and storage failures.
- `RuntimeError::Config` (variant) — Configuration error with message.
- `RuntimeError::Storage` (variant) — Storage error wrapping `StorageError`.
- `CrdtDocumentsRuntime::from_config` (fn, pub async) — Constructs runtime from `serde_json::Value` config; selects storage backend (localfs with configurable root, or gcs with feature gate); wires database (Postgres or SQLite via `DATABASE_URL` env or config) and runs migrations; falls back to in-memory change tracker if no database; returns fully initialized runtime.
- `CrdtDocumentsRuntime::shutdown` (fn, pub async) — Drains all artifact snapshot writers to guarantee pending mutations reach disk before runtime teardown; documented as idempotent; MUST be called before tokio context dies if LLM tools or Y.Doc mutations occurred.
- `tests::default_localfs_runtime_builds` (test fn) — Verifies localfs runtime initializes with temp storage root and empty registry.
- `tests::rejects_unknown_backend` (test fn) — Confirms `from_config` rejects unknown storage backend name with `RuntimeError::Config`.
- `tests::from_config_rejects_gcs_without_feature` (test fn) — Validates GCS backend is rejected unless `gcs` feature is enabled.
- `tests::shutdown_persists_pending_mutations_across_runtime_teardown` (test fn) — Smoke test validating shutdown flushes pending mutations to disk across tokio runtime boundaries; uses separate thread with dedicated tokio runtime to reproduce real teardown scenario.

## File-level notes

- Feature gating for GCS backend is sound (compile-time branch, clear error message for missing feature).
- Error handling is comprehensive across storage initialization, database connection, and migrations.
- Test coverage includes edge cases (unknown backend, feature flag guard, persistence across runtime teardown).
- `from_config` at line 89 intentionally discards `load_from_disk()` return value while propagating errors via `?` operator — idiomatic for side-effect-only operations.
- Strong precondition documented on `shutdown` (must be called before tokio context dies) has no runtime enforcement (no `Drop` impl or double-call guard), but underlying operation is delegated to `registry.shutdown_all()` whose semantics would determine actual idempotency guarantee.
- No dead code, unfinished stubs, or TODOs detected.
