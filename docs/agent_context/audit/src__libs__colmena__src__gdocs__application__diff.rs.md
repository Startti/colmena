# src/libs/colmena/src/gdocs/application/diff.rs

**Layer:** application  **Purpose:** Computes paragraph-level diffs between Google Docs snapshots using Myers algorithm, emitting structured `HumanChange` records for the co-edit guard without extra API calls. Output is deterministically sorted by (tab_id, paragraph).

## Symbols

- `PREVIEW_MAX_CHARS` (const, private) — Maximum character limit (120) for `HumanChange.preview` field; full text preserved in `before_text`/`after_text`.
- `paragraph_diff` (fn, pub) — Pure, deterministic entry point; compares prior and current `DocumentSnapshot`s per tab using Myers diff, returns sorted `Vec<HumanChange>` keyed by (tab_id, paragraph).
- `diff_tab` (fn, private) — Processes one tab's Myers diff over paragraph texts; converts `DiffOp` variants (Insert/Delete/Replace) to `HumanChange` records with proper anchoring logic.
- `make_change` (fn, private) — Constructs a `HumanChange` with truncated preview, before/after text, current timestamp, and tab context.
- `truncate` (fn, private) — Truncates string to max characters with trailing ellipsis if needed.
- `tests::p` (fn, test helper) — Creates a `ParagraphSnapshot` with given paragraph number and text.
- `tests::snap_single_tab` (fn, test helper) — Creates a `DocumentSnapshot` with a single tab for test fixtures.
- `tests::no_changes` (test) — Verifies no diff when snapshots are identical.
- `tests::single_modify` (test) — Verifies single paragraph modification is detected with before/after text.
- `tests::single_insert_end` (test) — Verifies insertion at end-of-tab.
- `tests::single_insert_middle` (test) — Verifies insertion in middle-of-tab.
- `tests::single_delete` (test) — Verifies single paragraph deletion with anchoring.
- `tests::replace_shrink_net_effect_is_delete_plus_modify` (test) — Verifies shrink operation (net-delete scenario).
- `tests::multi_tab_isolated_change` (test) — Verifies changes in one tab don't bleed to others.
- `tests::preview_truncates_long_text` (test) — Verifies preview truncation at 120+ellipsis while full text retained.
- `tests::deterministic_order` (test) — Verifies output order stability across multiple calls (accounting for non-deterministic `modified_time`).

## File-level notes

- Well-documented module header links spec (`2026-06-09-gdocs-paragraph-diff-design.md`).
- Algorithm correctly maps Myers diff operations: `Equal` ignored, `Insert`/`Delete` produce one record per operation, `Replace` splits into min(old, new) Modify records plus excess Insert/Delete.
- Delete anchor logic sound: uses post-delete position in current snapshot or last+1 if at end, clamped to 0 for empty tabs.
- Edge-case handling robust: all `.get()` calls paired with safe `.unwrap_or()` defaults; `saturating_add(1)` prevents overflow.
- Comprehensive test coverage: 9 test cases exercise normal path, multi-tab isolation, boundary conditions (empty tabs), and output properties (determinism, truncation).
- No dead code, panics, TODOs, or unimplemented stubs.
