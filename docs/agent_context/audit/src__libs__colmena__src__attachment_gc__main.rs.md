# src/libs/colmena/src/attachment_gc/main.rs

**Layer:** infrastructure  
**Purpose:** Standalone binary that implements garbage collection for stale attachments. Deletes rows from `conversation_attachments` registry and their backing blobs from `OutputStorageRepository` once they exceed a configurable TTL (default 7 days). Designed to run as a Cloud Scheduler → Cloud Run Job; idempotent via blob-delete-before-registry-delete pattern.

## Symbols

- `Cli` (struct, pub) — Clap argument parser: `ttl_days`, `batch_size` overrides, and `dry_run` flag for preview-only execution
- `GcSummary` (struct, private) — Accumulates garbage collection statistics: `total_deleted` and `total_storage_errors` counts
- `main` (fn, async) — Entry point: loads env vars via dotenvy, initializes tracing, parses CLI, wires services, invokes `run_gc`, logs completion
- `build_services_from_env` (async fn) — Constructs `(AttachmentRegistry, OutputStorageRepository)` pair from environment; reuses `EngineConfig::from_env` for storage adapter, builds fresh `PgPoolRegistry` + `PostgresAttachmentRegistry`, runs migrations on internal pool
- `run_gc` (async fn) — Core GC loop: batches `find_stale_attachments` queries, deletes blob from storage (best-effort, retried on failure), then deletes registry row (error logged but continue on failure); breaks when batch size < configured size; includes 100ms sleep between batches to avoid DB hammering
- `tests::seed_attachment` (async fn, #[cfg(test)]) — Test helper: stores bytes to `LocalCacheStorageAdapter`, then upserts record to registry with metadata
- `tests::backdate` (async fn, #[cfg(test)]) — Test helper: backdates `registered_at` timestamp and nulls `last_used_at` via raw SQL to make attachment stale for test cutoff
- `tests::fresh_sqlite_registry` (async fn, #[cfg(test)]) — Test helper: creates temporary SQLite database, runs migrations, returns `(registry, pool_arc, tempdir_guard)` to prevent premature cleanup
- `tests::run_gc_deletes_stale_rows_and_blobs` (test) — Verifies GC deletes stale attachments and blobs while preserving fresh ones; uses batch_size=2 to exercise multi-batch loop
- `tests::run_gc_dry_run_deletes_nothing` (test) — Verifies `dry_run=true` skips all deletions and logs intentions
- `tests::run_gc_returns_immediately_when_no_stale_rows` (test) — Verifies early exit when query returns empty batch

## File-level notes

- **Architecture integration**: Reuses `EngineConfig::from_env()` for storage adapter selection (mirrors `dag_engine` binary), ensuring consistent behavior across bindings.
- **Idempotency**: Blob delete is best-effort; registry row preserved on storage failure so next run retries. Registry delete failure orphans the blob (logged, rare) but doesn't fail the binary.
- **Tested thoroughly**: All three test cases use in-memory SQLite + LocalCache adapter; cover happy path (mixed stale/fresh, batch boundary), dry-run mode, and no-stale-rows early exit.
- **Structured logging**: All tracing calls use `target: "colmena::attachment_gc"` and event-based field naming for Cloud Logging integration; startup/completion and per-batch events included.
