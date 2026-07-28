# src/libs/colmena/src/crdt_documents/formula_engine_yrs_resolver.rs

**Layer:** infrastructure  **Purpose:** Production `CellResolver` impl that reads spreadsheet cells from a `yrs::Doc` CRDT. Decouples the formula engine from yrs so it remains independently testable.

## Symbols

- `YrsResolver<'a>` (struct, pub) — wrapper around a yrs::Doc reference; holds the ephemeral context for cell lookups
- `YrsResolver::new` (pub fn) — constructor that borrows a Doc reference
- `YrsResolver::get_formula` (pub fn) — returns the formula text string from a cell's `f` field if present; used by df_writer to detect formula overwrites
- `CellResolver` impl (trait impl) — implements the abstract CellResolver port for yrs::Doc
- `CellResolver::get` (impl method) — fetches CellSnapshot { v: json, t: u8 } from workbook→sheets[i]→cells→{addr}; returns None if any key missing
- `CellResolver::sheet_exists` (impl method) — checks whether a sheet by id exists in the workbook
- `CellResolver::iter_formulas_in_sheet` (impl method) — returns a boxed iterator of (cell_address, formula_text) tuples for all formula cells in a sheet
- `any_to_json` (fn, private) — converts a yrs::Any scalar to serde_json::Value; treats Buffer/Array/Map as null
- `yrs_resolver_reads_literal_cell` (test) — verifies that numeric literals round-trip as f64 through yrs storage
- `yrs_resolver_reports_sheet_existence` (test) — verifies sheet_exists returns true for existing sheets and false for missing ones
- `yrs_resolver_iter_formulas_returns_empty_when_none` (test) — confirms empty iteration when a sheet has no formulas
- `yrs_resolver_returns_none_for_missing_cell` (test) — verifies None return for cells and sheets that don't exist
- `yrs_resolver_get_formula_returns_text_for_formula_cell` (test) — verifies get_formula retrieves formula text and returns None for literals/missing cells/sheets

## File-level notes

- **Code location:** The Y.Doc cell schema is documented at the top: `workbook.sheets[i].cells.<A1> = {v, t, f?, fs?}`. The `f` and `fs` fields are optional and only populated once D-T5 starts persisting formula text; until then cells are `{v, t}` only.
- **Duplication:** `get_formula` (lines 27–56) and `get` (lines 60–95) both traverse the same sheet lookup path (workbook→sheets[i] with id match). This duplicated navigation is refactorable into a shared helper but is not a blocking issue.
- **Error handling:** All operations gracefully return None/false on missing keys rather than panicking; no error type needed because cell/sheet absence is a valid query result.
- **Lifetime management:** 'a is correctly borrowed from the Doc; transaction (`txn`) is created fresh per operation.
- **Test coverage:** Five tests cover the happy path (literal cells, sheet existence, formula retrieval) and edge cases (missing cells, missing sheets, empty formula lists).
