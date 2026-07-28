# src/libs/colmena/src/crdt_documents/narration.rs

**Layer:** infrastructure  
**Purpose:** Decodes Yjs v1 update byte blobs and produces human-readable change summaries by replaying state diffs (added/deleted/changed sheets and cells).

## Symbols

- `narrate` (fn, pub) — Applies update_bytes to a `before` Doc clone, computes projection diffs, returns summary string for ChangeTracker.record(...)
- `summarize_diff` (fn, private) — Detects added/deleted/changed sheets and cells between before/after projections, returns formatted multi-line summary
- `sheets_by_id` (fn, private) — Extracts sheets array from projection JSON, builds HashMap<sheet_id, sheet_value>
- `tests` (mod, private cfg(test)) — Test module with 4 integration tests

### Test Functions
- `summarises_added_cell` (test, private) — Verifies narrate detects and formats newly added cells with addresses and values
- `summarises_added_sheet` (test, private) — Verifies narrate detects and formats newly added sheets by name
- `summarises_changed_cell` (test, private) — Verifies narrate detects and formats cell mutations with before → after values
- `empty_update_yields_no_detectable_change_message` (test, private) — Verifies narrate returns standard "no detectable change" message for state-only updates

## File-level notes

- **Strategy documented**: Lines 3–6 explain the v1 approach: replay before-state onto a clone, apply update, diff IR projections. Noted as "slow but simple" with v1.1 optimization deferred.
- **Error resilience**: Both `Update::decode_v1` calls (lines 21, 25) are guarded with `if let Ok(u)`, preventing panics from malformed input; silent fallback to diff (possibly "no detectable change") is intentional and acceptable for robustness.
- **Test coverage**: All four public behaviors tested (add sheet, add cell, change cell, no-op); uses helper functions from `tool_executor` and `projection` modules.
- **Performance note**: Lines 101–102 involve cloning the sheets array; scale is expected to be small (typical spreadsheet documents).
