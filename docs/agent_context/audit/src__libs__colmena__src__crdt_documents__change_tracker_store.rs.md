# src/libs/colmena/src/crdt_documents/change_tracker_store.rs

**Layer:** infrastructure  **Purpose:** Storage adapter layer for CRDT change tracking. Defines the `ChangeTrackerStore` port and provides two implementations: in-memory (for tests/dev) and SQLx-based (for SQLite/Postgres production).

## Symbols

- `NewEvent` (struct, pub) — DTO for a new event to be recorded: artifact_id, sheet_id, origin (source identifier), and summary text
- `StoredEvent` (struct, pub) — DTO for a persisted event row: id, artifact_id, sheet_id, origin, summary, created_at timestamp
- `StoredArtifact` (struct, pub) — DTO for artifact metadata: artifact_id, name, created_at, last_accessed_at
- `StoreError` (enum, pub) — Error type wrapping SQL errors as `Sql(String)`
- `ChangeTrackerStore` (trait, pub) — Port abstraction defining the persistence contract for events and session cursors
  - `insert_event` — async method to record a new event and return its assigned id
  - `events_since` — async method to retrieve events for an artifact after a given event id, with optional sheet/origin filters and limit
  - `cursor_for` — async method to retrieve the last-seen event id for a session+artifact pair
  - `upsert_cursor` — async method to update the cursor position for a session+artifact pair
  - `touch_artifact` — async method to record or update an artifact's metadata in the session
  - `artifacts_for_session` — async method to list artifacts for a session ordered by last access, with limit
- `InMemoryChangeTrackerStore` (struct, pub) — In-memory implementation using Mutex-wrapped HashMap for events, cursors, and artifacts; used for tests and no-database dev
- `InMemoryState` (struct, private) — Internal state holder: events vec, next_id counter, cursors map, artifacts map
- `InMemoryChangeTrackerStore::new` (pub fn) — Constructor; initializes empty in-memory state
- `InMemoryChangeTrackerStore::default` (impl Default) — Returns `new()`
- `impl ChangeTrackerStore for InMemoryChangeTrackerStore` — Trait implementation with 6 async methods managing in-memory collections
- `chrono_now_iso` (fn, private) — Helper to get current UTC time formatted as RFC3339 string
- `ChangeTrackerStoreRef` (type alias, pub) — Convenience alias for `Arc<dyn ChangeTrackerStore>`
- `SqlxDialect` (enum, pub) — Enum for database dialect detection: `Sqlite` or `Postgres`
  - `from_url` — pub method to detect dialect from connection URL prefix (`postgres://`, `postgresql://`, or `sqlite:`)
- `SqlxChangeTrackerStore` (struct, pub) — SQLx-based implementation supporting both SQLite and Postgres via `AnyPool`
- `SqlxChangeTrackerStore::new` (pub fn) — Constructor taking pool and dialect
- `SqlxChangeTrackerStore::is_postgres` (private fn) — Helper to check if dialect is Postgres (used for SQL placeholder switching)
- `impl ChangeTrackerStore for SqlxChangeTrackerStore` — Trait implementation with 6 async methods using dialect-specific SQL (parameterized queries with `?` for SQLite, `$N` for Postgres)
- `tests::make_event` (fn, private) — Test helper to construct a NewEvent
- `tests::in_memory_records_and_lists_events_in_order` (test) — Verifies event insertion and retrieval maintains order
- `tests::in_memory_filters_by_origin` (test) — Verifies `exclude_origin` filter excludes specified source
- `tests::in_memory_filters_by_sheet` (test) — Verifies `sheet_id_filter` returns only matching sheet
- `tests::in_memory_caps_results_at_limit` (test) — Verifies `limit` parameter truncates results
- `tests::in_memory_cursor_upsert_and_lookup` (test) — Verifies cursor operations (insert, update, retrieve)
- `tests::in_memory_touch_artifact_then_list` (test) — Verifies artifact touch/update and listing sorted by recent access
- `tests::sqlx_sqlite_round_trip` (test) — Integration test: SQLite in-memory DB with migrations, full CRUD round-trip

## File-level notes

- **Missing doc comments on public items**: `NewEvent`, `StoredEvent`, `StoredArtifact`, `ChangeTrackerStore` trait (and its methods), `SqlxDialect`, and `from_url` lack `///` doc comments. Per CLAUDE.md convention ("All public items"), these should be documented to support IDE discoverability and generated docs.

- **Placeholder string interpolation (lines 347–361) is a clarity/efficiency concern**: The character-by-character `.chars().map(...).collect::<String>()` converts `?` to `$1`, `$2`, etc. for Postgres. While correct, it is performed at query-build time for every `events_since` call. Could be extracted to a dedicated function (`convert_placeholders_to_postgres`) for reuse and clarity, or replaced with a templating approach. Not a blocker (queries are short), but worth noting for maintainability.

- **SQLite RETURNING clause behavior** (lines 300–307): Good defensive comment explaining why `RETURNING` is used directly instead of the two-statement `INSERT` + `SELECT last_insert_rowid()` pattern. The note about multi-connection pools confirms the fix for the integration test `recent_changes_round_trip_via_ws_peer` (test B-T14). This is solid.

- **Error handling**: `StoreError` is a simple wrapper (`Sql(String)`). No `#[from]` trait impl for `sqlx::Error`, but this is acceptable given the storage layer's role (errors are re-wrapped). Could be enhanced with structured error variants (e.g., `Conflict`, `NotFound`) in future, but not critical for current usage.

- **Test coverage**: In-memory implementation has 6 focused tests covering core operations. SQLx test only covers SQLite; Postgres coverage would benefit from a CI integration test (not practical in local tests without external DB). Unit tests are robust and well-scoped.

- **InMemoryState is not `pub`**: Correctly private; only the store wrapper is public. Good encapsulation.
