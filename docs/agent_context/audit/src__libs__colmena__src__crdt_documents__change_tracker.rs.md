# src/libs/colmena/src/crdt_documents/change_tracker.rs

**Layer:** application  
**Purpose:** Async facade over ChangeTrackerStore that preserves v1 public API for recording and querying change events; coordinates between callers and the underlying persistent store (in-memory or SQL).

## Symbols

- `ChangeEvent` (pub struct) — Event DTO with event_id, artifact_id, sheet_id, origin, summary, created_at; returned by `since()` queries
- `ChangeEvent::from` (impl From<StoredEvent>) — Converts StoredEvent to ChangeEvent for public API
- `ChangeTracker` (pub struct) — Thin async wrapper holding Arc<dyn ChangeTrackerStore>; forwards all operations to the store
- `ChangeTracker::new` (pub fn) — Constructs a ChangeTracker from a store reference; cheap facade
- `ChangeTracker::record` (pub async fn) — Records a change event; returns new event_id or 0 on error (best-effort); optional sheet_id for document-level events
- `ChangeTracker::since` (pub async fn) — Queries events after since_event_id with optional sheet_id and exclude_origin filters; respects limit cap
- `new_tracker` (fn in tests) — Helper creates an in-memory-backed ChangeTracker for tests
- `records_and_since_filters` (test) — Verifies record/since round-trip and cursor-based filtering work
- `empty_for_unknown_artifact` (test) — Verifies unknown artifact_id returns empty event list
- `exclude_origin_filters_out_self` (test) — Verifies exclude_origin parameter filters matching origin
- `sheet_filter_scopes_results` (test) — Verifies sheet_id parameter limits results to single sheet

## File-level notes

- Module doc clearly explains the migration from v1 in-memory HashMap to abstracted ChangeTrackerStore with SQL/in-memory pluggability
- Error handling is intentionally best-effort: record() returns 0 on insert failure, since() returns empty vec on query failure; callers use returned id only for high-water cursor advancement (B-T5/B-T7)
- v1 API shape preserved except timestamp_ms removed (store records ISO 8601 created_at instead) and artifact_id/sheet_id fields added for richer narration (B-T11)
- All public methods are async and forward to the store trait; no local state or caching
- Test coverage is comprehensive: happy path, unknown artifacts, origin filtering, sheet scoping
