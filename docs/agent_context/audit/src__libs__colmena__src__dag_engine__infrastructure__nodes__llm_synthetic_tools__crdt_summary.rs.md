# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_summary.rs

**Layer:** infrastructure  **Purpose:** Builds the "Recent changes since your last turn" block injected into the system message for CRDT document contexts. Exports an operating manual prelude that teaches agents how to handle CRDT workflows without explicit tool names.

## Symbols

- `CRDT_SPREADSHEET_PROTOCOL_PRELUDE` (const, pub) — Operating manual markdown text auto-injected into system_message when crdt_documents is configured; teaches naive-user-focused behavior (discovery, skill loading, clarification, persistence).
- `MAX_SHEETS_IN_SUMMARY` (const) — Limit of 10 sheets in change-event summary display; events beyond this are grouped into overflow marker.
- `MAX_EVENTS_TO_FETCH` (const) — Limit of 200 events fetched from backend per summary build.
- `build_recent_changes_block` (fn, pub, async) — Fetches recent change events from backend cursor, excludes self-origin events, returns formatted markdown block or None if no events.
- `format_block` (fn, private) — Aggregates StoredEvents by (sheet_id, origin) tuple, sorts by event count descending, renders markdown summary with peer/change counts and overflow marker.
- `tests::ev` (fn, test helper) — Constructs a StoredEvent fixture for testing.
- `tests::format_block_with_empty_events_shows_zero` (test) — Verifies empty event list renders "0 events" string.
- `tests::single_sheet_single_peer` (test) — Verifies single sheet with single peer origin aggregates and counts correctly.
- `tests::two_sheets_two_peers` (test) — Verifies multiple sheets and origins aggregate independently and render with correct peer count.
- `tests::workbook_level_when_sheet_unknown` (test) — Verifies sheet_id=None events render as "Workbook (sheet unknown)" label.
- `tests::caps_at_max_sheets_with_overflow_marker` (test) — Verifies output truncates at 10 sheet/peer groups and appends overflow marker for remainder.

## File-level notes

- All symbols are either public (two consts, one async fn) or test-scoped; no dead code.
- Error handling via `?` operator on async backend calls is appropriate and consistent.
- Test coverage is comprehensive: empty case, single and multi-peer/sheet, unknown sheet, and truncation edge case.
- Sorting by `std::cmp::Reverse(count)` for descending order is idiomatic; no improvement needed.
- No todo!(), unimplemented!(), or unfinished markers detected.
- The module is well-documented with module-level comments explaining purpose; public items lack individual doc comments but intent is clear from context.
