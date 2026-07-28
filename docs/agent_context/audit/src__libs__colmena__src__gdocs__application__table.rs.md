# src/libs/colmena/src/gdocs/application/table.rs

**Layer:** application  **Purpose:** Surgical table editing use cases for Google Docs — read tables with per-cell previews, set cell text, insert/delete rows and columns, all via Docs API requests.

## Symbols

### Data Structures
- `TableListing` (struct, pub) — Return envelope for `read_tables`: wraps vec of `TableInfo`
- `TableInfo` (struct, pub) — Per-table metadata: index, tab_id, dimensions, cell list with previews
- `TableCellInfo` (struct, pub) — Single cell info: position (row/col), text preview (80 chars), row/col span

### Helpers
- `cell_location` (fn, pub(crate)) — Constructs Docs API `tableCellLocation` JSON from table start index, row, col, and optional tab_id
- `find_table` (fn, pub(crate)) — Locates a table by 0-based index within requested tab (or first tab), errors on not found or invalid args
- `find_cell` (fn, pub(crate)) — Locates a cell in a table by row/col, errors if out of range or covered by a merge

### Public Use Cases (read, set, insert, delete)
- `run_read_tables` (async fn, pub) — Fetches doc snapshot and lists all tables with per-cell previews (text truncated to 80 chars), optionally scoped to a tab
- `run_set_table_cell` (async fn, pub) — Replaces a single cell's plain text: runs co-edit guard, finds table/cell, builds delete+insert requests, finalizes
- `run_insert_table_row` (async fn, pub) — Inserts a row above/below `at_row` in the addressed table: guard → find table → build insert request → apply
- `run_delete_table_row` (async fn, pub) — Deletes a row from the addressed table: guard → find table → build delete request → apply
- `run_insert_table_column` (async fn, pub) — Inserts a column left/right of `at_col` in the addressed table: guard → find table → build insert request → apply
- `run_delete_table_column` (async fn, pub) — Deletes a column from the addressed table: guard → find table → build delete request → apply

### Request Builders (private)
- `build_set_cell_requests` (fn, private) — Builds two-request sequence (delete+insert) to replace cell text while preserving the mandatory trailing newline
- `build_insert_row_request` (fn, private) — Builds Docs API `insertTableRow` request JSON for the addressed row
- `build_delete_row_request` (fn, private) — Builds Docs API `deleteTableRow` request JSON for the addressed row
- `build_insert_column_request` (fn, private) — Builds Docs API `insertTableColumn` request JSON for the addressed column
- `build_delete_column_request` (fn, private) — Builds Docs API `deleteTableColumn` request JSON for the addressed column

### Tests
- `tests` (mod, cfg(test)) — Unit tests covering snapshot construction, cell/table lookup, request building, and end-to-end `run_set_table_cell` orchestration with guard + batch_update

## File-level notes
- Consistent pattern across all six `run_*` functions: guard → find → build request(s) → finalize via `apply_and_finalize`
- All domain errors properly propagated via `Result<T, DocsError>`; clear error messages for out-of-range / merged cells / missing tabs
- Request builders are private; public functions own the orchestration and co-edit safety
- Plain text only per v1 spec (rich/markdown cell content deferred to v1.1)
- No dead code, no unfinished stubs; all public operations tested
