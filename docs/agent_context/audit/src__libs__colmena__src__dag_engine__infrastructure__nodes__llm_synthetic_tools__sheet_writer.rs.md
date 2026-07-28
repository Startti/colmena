# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/sheet_writer.rs

**Layer:** infrastructure  
**Purpose:** Dispatches Google Sheets write-back operations for `gsheets_run_python` (and future tools like `data_run_python`) across four modes (replace, overwrite, update_in_place, update_by_position). Provides A1 addressing, formula `{{Column}}` resolution, tab metadata fetching, and new-column planning.

## Symbols

- `LoadedSnapshot` (struct, pub) — snapshot of a sheet binding loaded this run; holds records, range-flag, and ambiguity-flag for positional write-back safety
- `write_output_sheets` (async fn, pub) — main dispatcher; routes each `output_sheets` entry by mode and returns result metadata
- `do_replace` (async fn, private) — mode `replace`: create tab and write full DataFrame; applies collision policy
- `do_overwrite` (async fn, private) — mode `overwrite`: replace existing tab contents; includes schema-change guard
- `do_update_in_place` (async fn, private) — mode `update_in_place`: diff and apply only changed cells using a key field
- `do_update_by_position` (async fn, private) — mode `update_by_position`: positional diff-write (by row index, no key needed)
- `write_full_df` (async fn, private) — common DataFrame write helper; used by replace/overwrite/auto_suffix paths
- `validate_full_index` (fn, private) — validates that returned df index is exactly `{0..n-1}` (catches subset/reset_index/concat footguns)
- `addressable_columns` (fn, private) — filters header columns to addressable ones (unique, non-empty); excludes duplicates and blanks
- `PlannedCell` (struct, private) — single cell for a new column; holds column index, row, and raw value
- `NewColumnPlan` (struct, private) — plan for columns present in returned df but absent from sheet header; pairs added columns with their planned cells
- `plan_new_columns` (fn, private) — assigns next-free column indices for new columns and emits header + body cells; skips all-null columns
- `columns_match` (fn, private) — checks if two column lists name the same set (order-insensitive, duplicate-insensitive)
- `fetch_tab_meta` (async fn, private) — fetches lightweight tab metadata (row/col count, header names, last modified)
- `col_letter` (fn, private) — converts 0-based column index to A1 letter(s) (e.g., 0→"A", 26→"AA")
- `a1_addr` (fn, pub(super)) — converts column index + 1-based row to A1 address; used for batch_update_cells targeting
- `FormulaResolveError` (struct, private) — error wrapper for unknown column references in formula placeholders; includes valid column list
- `resolve_formula_placeholders` (fn, private) — resolves `{{ColumnName}}` placeholders in formulas to real A1 refs; aborts write on unknown column
- `FORMULA_CELLS_SAMPLE_CAP` (const, private) — max resolved-formula cells echoed back (50); larger writes signal total + truncation flag
- `FormulaCellLog` (struct, private) — accumulates formulas a diff-write lands; tracks bounded sample and total count
- `position_tests` (mod, test) — comprehensive test suite covering index validation, addressable columns, A1 mapping, formula resolution, and column planning

## File-level notes

- **Error response inconsistency**: Different write modes return different field names in error responses (`"tab"` vs `"name"`), creating downstream ambiguity. Lines 91, 214, 259 show the inconsistency.
- **Duplicated header-read pattern**: `do_update_in_place` (lines 276–308) and `do_update_by_position` (lines 605–634) perform nearly identical full-row header fetches. Could extract to a shared helper to reduce duplication.
- **Formula placeholder safety**: The resolver (`resolve_formula_placeholders`, lines 980–1025) manually indexes bytes while iterating UTF-8 chars. Works correctly (Rust guarantees UTF-8), but the pattern is subtle and relies on the caller passing valid UTF-8.
- **Graceful JSON parsing**: All required fields (`df_records`, `df_cols`, `key`, `df_index`) are parsed with `.and_then()` chains that return structured errors rather than panicking. This is appropriate for untrusted LLM output.
- **Positional safety guards**: `validate_full_index` and `ambiguous` flag in `LoadedSnapshot` prevent silent wrong-row writes — critical for a non-key-based write mode.
