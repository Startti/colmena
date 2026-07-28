# src/libs/colmena/src/gsheets/infrastructure/merge_fill.rs

**Layer:** infrastructure  **Purpose:** Forward-fills merged cells in Google Sheets reads by propagating the anchor cell value across the entire merge rectangle. Google's API returns merged region values only at the top-left anchor; this module makes reads reflect the visual merge.

## Symbols

- `MergeRect` (struct, pub) — represents a merge rectangle in absolute sheet coordinates (half-open: start_row..end_row, start_col..end_col)
- `MergeRect::start_row` (field, pub) — top row of the merge (inclusive)
- `MergeRect::end_row` (field, pub) — bottom row of the merge (exclusive)
- `MergeRect::start_col` (field, pub) — left column of the merge (inclusive)
- `MergeRect::end_col` (field, pub) — right column of the merge (exclusive)
- `forward_fill_merges` (fn, pub) — propagates anchor value across every cell in each merge rectangle, clamped to grid bounds; skips merges whose anchor falls outside the grid
- `grid` (fn, private) — test helper that wraps a Vec<Vec<Value>> in a grid
- `horizontal_merge_fills_to_the_right` (test) — verifies that horizontal merges fill from left anchor rightward
- `vertical_merge_fills_downward` (test) — verifies that vertical merges fill from top anchor downward
- `block_merge_fills_both_directions` (test) — verifies that rectangular merges fill both rows and columns
- `anchor_outside_grid_is_skipped_b1` (test) — verifies that merges whose anchor precedes the grid offset are skipped per spec B1
- `offset_applied_when_anchor_inside_grid` (test) — verifies correct offset subtraction when computing grid-relative anchor position
- `no_merges_is_noop` (test) — verifies that an empty merge list leaves the grid unchanged
- `merge_extending_past_grid_is_clamped` (test) — verifies that merges extending beyond grid bounds are clamped without panic
- `empty_anchor_fills_with_null_noop` (test) — verifies that null anchors propagate as nulls (no-op visually)

## File-level notes

- Comprehensive test coverage includes all major cases: horizontal, vertical, block merges; offset handling; boundary clamping; edge cases (no merges, null anchors, anchor outside grid).
- Guard clauses properly handle grid bounds: `grid.get()`, `grid.len()`, `saturating_sub()`, `.min()` prevent panics on malformed input.
- Spec reference: `docs/superpowers/specs/2026-06-14-gsheets-expand-merges-design.md`.
- No unfinished code, no obvious dead symbols, no missing error handling.
