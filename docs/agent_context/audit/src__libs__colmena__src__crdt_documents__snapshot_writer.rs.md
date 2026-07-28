# src/libs/colmena/src/crdt_documents/snapshot_writer.rs

**Layer:** infrastructure  **Purpose:** Spawns and manages a background tokio task that periodically encodes and persists CRDT document state to storage on demand (dirty flag + 5-second tick).

## Symbols

- `TICK` (const) — 5-second interval duration for periodic snapshot polling
- `SnapshotHandle` (struct, pub) — public handle for controlling the snapshot writer task lifecycle
- `SnapshotHandle::notify` (field, pub) — Arc-wrapped async Notify primitive signaled on mutations
- `SnapshotHandle::dirty` (field, pub) — Arc-wrapped atomic bool flag tracking if document has unsaved mutations
- `SnapshotHandle::shutdown_tx` (field, private) — optional oneshot sender to trigger graceful task shutdown
- `SnapshotHandle::done_rx` (field, private) — optional oneshot receiver to await writer task's final flush completion
- `SnapshotHandle::mark_dirty` (method, pub) — sets dirty flag and notifies the writer task of a mutation
- `SnapshotHandle::shutdown` (method, pub async) — signals shutdown, awaits final flush, idempotent via take() pattern
- `spawn_writer` (fn, pub) — spawns background tokio task that listens for mutation notifications and 5s ticks, flushes dirty state, returns handle
- `flush` (fn, async) — encodes yrs Doc state as update_v1 and persists via ArtifactStorage, logs errors without propagating
- `tests::dirty_then_shutdown_persists_state` (test, async) — verifies that mutations are persisted to disk when dirty flag is set and shutdown is called

## File-level notes

- **Error handling by design:** `flush()` logs storage errors as warnings but does not propagate them; this is intentional for a best-effort background task (line 90).
- **Idempotent shutdown:** The `shutdown()` method uses `take()` to ensure calling it twice is a safe no-op (lines 36–40).
- **Synchronization:** Proper use of `Ordering::Release` (line 29) and `Ordering::AcqRel` (line 70) for atomic operations; `notify.notify_one()` wakes the task once per mutation.
- **Test coverage:** The single test exercises the full lifecycle (mutation → mark_dirty → shutdown → load & verify) via LocalFsStorage and yrs codec round-trip.
