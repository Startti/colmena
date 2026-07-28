# src/libs/colmena/src/documents/application/apply_excel_ops.rs

**Layer:** application  **Purpose:** Implements the `ExcelOpApplier` service that applies document patch operations (cell edits, row/column shifts, table/sheet management) to Excel spreadsheet representations.

## Symbols

- `ExcelOpApplier<'a>` (pub struct) — Application service holding a reference to an `IdGenerator` for allocating new sheet and table IDs
- `ExcelOpApplier::ids` (pub field) — Reference to ID generator for creating new resource identifiers
- `ExcelOpApplier::apply` (pub method) — Main entry point; applies a single `PatchOp` to an `ExcelIR` document and returns assigned IDs or error
- `invalid` (private fn) — Helper that constructs `DocumentError::InvalidPatchOp` with serialized op (fallback to Null)
- `parse_cell_type` (private fn) — Parses string ("string"|"number"|"boolean"|"date"|"formula") to `CellType` enum
- `parse_a1` (private fn) — Parses Excel A1 notation (e.g., "B5") into 0-indexed (row, col) tuple
- `parse_range` (private fn) — Parses Excel range notation (e.g., "A1:B3") into two (row, col) tuples
- `to_a1` (private fn) — Converts 0-indexed (row, col) to Excel A1 notation string
- `cell_row` (private fn) — Extracts 1-indexed row number from an A1 address string
- `cell_col` (private fn) — Extracts 0-indexed column number from an A1 address string
- `in_range` (private fn) — Checks if an A1 address falls within a range (inclusive)
- `shift_rows` (private fn) — Shifts all cells in a range by delta rows (positive = down, negative = up), skipping rows before threshold
- `shift_cols` (private fn) — Shifts all cells in a range by delta columns (positive = right, negative = left), skipping columns before threshold
- `tests` (test module) — Unit tests for `SetCell`, `InsertRow`, `AddSheet`, `CreateTable`, `to_a1` roundtrip

## File-level notes

- **API indexing asymmetry**: Row parameters in patch ops (e.g., `before_row`, `row_index`) are 1-indexed; column parameters (e.g., `before_col`, `col_index`) are 0-indexed. Internal A1 functions work with 0-indexed (row, col). This is intentional and consistent but worth noting.
- **Comprehensive variant coverage**: The `apply` match statement explicitly lists 29 non-applicable (Word/HTML/Presentation) `PatchOp` variants at the end, rejecting them with a clear error. This is defensive programming — new variants added to `PatchOp` that don't apply to Excel will not compile without updating this match arm.
- **BTreeMap mutation pattern**: `shift_rows` and `shift_cols` avoid iterator invalidation by collecting moves into a vector, removing old keys, then inserting new entries — this is correct.
- **No test infrastructure flags**: Test module has no `#[ignore]` attributes because tests do not depend on environment variables like `DATABASE_URL` or `TAVILY_API_KEY`.
