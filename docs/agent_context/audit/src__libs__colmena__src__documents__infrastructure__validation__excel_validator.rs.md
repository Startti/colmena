# src/libs/colmena/src/documents/infrastructure/validation/excel_validator.rs

**Layer:** infrastructure  **Purpose:** Validates ExcelIR document structures for structural correctness: duplicate sheet/table IDs, dangling style references, and cell value type mismatches.

## Symbols

- `ExcelValidator` (pub struct) — Stateless validator that implements IRValidator trait for Excel IR documents
- `validate` (fn in IRValidator impl, pub) — Checks ExcelIR for duplicate sheet IDs, duplicate table IDs across workbook, valid style_ref references, and value-type consistency for all cells (lines 8–72)
- `base_ir` (fn in tests, private) — Constructs a minimal valid ExcelIR with one empty sheet for test setup (lines 81–92)
- `empty_ir_is_valid` (test, private) — Verifies that a minimal valid IR structure passes validation (lines 94–99)
- `duplicate_sheet_ids_fail` (test, private) — Verifies that duplicate sheet IDs are rejected with IRValidationFailed error (lines 101–115)
- `dangling_style_ref_fails` (test, private) — Verifies that a cell style_ref pointing to undefined named_styles is rejected (lines 117–133)
- `type_mismatch_fails` (test, private) — Verifies that cell values not matching their declared type are rejected (lines 135–151)

## File-level notes

- Validation logic is deterministic and exhaustive: checks traverse all sheets → all cells for style refs and type consistency; all sheets/tables for ID uniqueness.
- Table ID uniqueness is checked globally across the entire workbook (line 26 HashSet is not reset per sheet), ensuring IDs are unique regardless of sheet membership.
- Cell type validation (lines 54–60) correctly maps CellType variants: String/Formula → string JSON value, Number → numeric JSON value, Boolean → boolean JSON value, Date → string JSON value.
- Error paths are descriptive (e.g., `/workbook/sheets/{}/cells/{addr}/style_ref`), enabling callers to pinpoint failures.
- Test coverage is comprehensive: empty valid IR, duplicate sheet IDs, dangling style refs, and type mismatches.
- No unfinished work, dead code, or panics observed.

