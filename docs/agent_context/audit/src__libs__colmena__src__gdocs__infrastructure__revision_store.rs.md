# src/libs/colmena/src/gdocs/infrastructure/revision_store.rs

**Layer:** infrastructure  
**Purpose:** Provides persistent storage (Postgres or in-memory) for tracking the last revision and optional snapshot of a Google Docs document per (agent_session_id, document_id) pair. Used by the co-edit guard to detect and show paragraph-level diffs of human edits intervening between agent writes.

## Symbols

- `DEFAULT_MAX_SNAPSHOT_BYTES` (const, pub) — Maximum serialized snapshot size before dropping (default 1 MiB, configurable via env var)
- `max_snapshot_bytes()` (fn, pub) — Read the effective cap from `COLMENA_GDOCS_MAX_SNAPSHOT_BYTES` env var or return default
- `RevisionStore` (trait, pub async) — Port for persistent revision+snapshot tracking; backward-compat `get` and `put` shims delegate to the new `*_with_snapshot` methods
  - `get_with_snapshot()` (async method) — Return both last revisionId and full snapshot if persisted
  - `put_with_snapshot()` (async method) — Persist the revision and optionally the snapshot (None clears previously stored snapshot)
  - `get()` (async method, default impl) — Legacy shim that delegates to `get_with_snapshot` and returns revision only
  - `put()` (async method, default impl) — Legacy shim that delegates to `put_with_snapshot` with `None` snapshot
- `PostgresRevisionStore` (struct, pub) — Production adapter backed by PgPool
  - `pool` (field) — PgPool connection to read/write `gdocs_session_state`
  - `has_snapshot_col` (field) — Cached flag indicating whether v1.1 snapshot columns exist (detected at init via `information_schema`)
- `PostgresRevisionStore::new()` (impl async fn, pub) — Construct and probe `information_schema` for `last_snapshot_json` column; degrades silently to v1 behavior if missing and logs warn
- `RevisionStore for PostgresRevisionStore` (impl, pub async) — Executes branching logic on `has_snapshot_col` to either fetch+store both revision and snapshot (v1.1) or revision-only (v1)
  - `get_with_snapshot()` — Conditionally selects from `gdocs_session_state` with or without snapshot columns
  - `put_with_snapshot()` — Upserts with size-cap enforcement (drops snapshot if exceeds `max_snapshot_bytes()`, logs warn, and stores NULL)
- `RevSnapshotEntry` (type alias, test-only) — `(RevisionId, Option<DocumentSnapshot>)`
- `RevStoreMap` (type alias, test-only) — `HashMap<(String, String), RevSnapshotEntry>`
- `InMemoryRevisionStore` (struct, pub, test-only) — In-memory adapter using `tokio::sync::RwLock<HashMap>` for unit tests
  - `map` (field) — Locked HashMap keyed on `(session_id, document_id)`
  - `Default::default()` (impl) — Delegates to `new()`
  - `new()` (impl fn, pub) — Construct with empty HashMap
- `RevisionStore for InMemoryRevisionStore` (impl, test-only async) — Hashmap get/put with snapshot support
  - `get_with_snapshot()` — Reads from map or returns `(None, None)`
  - `put_with_snapshot()` — Inserts or upserts in map; `None` snapshot clears prior entry
- `tests` (mod, cfg(test)) — Unit test module
  - `make_snapshot()` (fn) — Helper to construct a DocumentSnapshot with one tab, one paragraph
  - `in_memory_round_trip_legacy_api()` (test) — Verifies legacy `get`/`put` round-trip
  - `in_memory_round_trip_with_snapshot()` (test) — Verifies `get_with_snapshot`/`put_with_snapshot` round-trip
  - `in_memory_put_without_snapshot_clears_old_snapshot()` (test) — Verifies `put_with_snapshot` with `None` clears prior snapshot
  - `in_memory_scoped_by_session()` (test) — Verifies isolation by (session_id, document_id)
  - `in_memory_overwrite_same_key()` (test) — Verifies overwrite behavior
  - `max_snapshot_bytes_uses_env_override()` (test) — Verifies env var override works and defaults are respected

## File-level notes

- **v1/v1.1 compatibility**: Branching on `has_snapshot_col` is intentional; older databases without the migration applied automatically degrade to v1 behavior (revision-only, empty diffs) with a single warn at boot. No breaking change — seamless.
- **Size capping**: Snapshots exceeding the cap are dropped (stored as NULL) and a warn is logged. The co-edit guard then operates in v1 mode (no paragraph diffs) for that document.
- **Backward compatibility**: `get` and `put` methods remain on the trait as default impls for code still using the legacy API; all new code should call `get_with_snapshot`/`put_with_snapshot`.
- **Error handling**: All database errors are wrapped in `DocsError::Internal` with context. Errors in JSON serialization (to_value, to_string) are silently dropped (`.ok()`) rather than causing a panic — if a snapshot can't be serialized, it's stored as NULL.
- **Thread safety**: `PostgresRevisionStore` holds a `PgPool` (thread-safe, reusable). `InMemoryRevisionStore` uses `tokio::sync::RwLock` for test isolation.
- **Test coverage**: 6 unit tests on `InMemoryRevisionStore` covering round-trip, snapshots, clearing, isolation, overwrite, and env override — comprehensive.
