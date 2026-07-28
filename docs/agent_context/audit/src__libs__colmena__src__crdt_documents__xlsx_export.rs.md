# src/libs/colmena/src/crdt_documents/xlsx_export.rs

**Layer:** infrastructure  **Purpose:** Serializes a Yrs CRDT document projection to XLSX binary format via `rust_xlsxwriter`, handling sheet creation and cell value writing for v1 scope (strings, numbers, booleans only).

## Symbols

- `ExportError` (enum, pub) — Wrapper error type for `rust_xlsxwriter::XlsxError`
- `export_doc_to_xlsx` (fn, pub) — Converts a Yrs `Doc` to XLSX byte buffer via projection and workbook serialization
- `parse_a1` (fn, private) — Parses Excel A1 cell notation (e.g., "B3") into zero-indexed (row, col) tuple using base-26 decoding for columns
- `tests::exports_two_sheets_with_values` (test) — Verifies round-trip export/import of multiple sheets with string and numeric cells
- `tests::exports_empty_workbook_with_default_sheet` (test) — Verifies empty document creates default sheet without error

## File-level notes

- **V1 scope enforced:** Module comment explicitly declares cells-only support (no format, formulas, merged cells, charts). Line 49 silently drops non-scalar JSON values; documented as v1.1 follow-up.
- **Defensive sheet handling:** Creates default "Sheet1" if projection contains no sheets (lines 24-26); uses fallback sheet name "Sheet" if missing (line 28).
- **Robust A1 parser:** Handles multi-letter columns (A–ZZ–AAA, etc.) via correct base-26 arithmetic; edge cases covered (empty parts, row 0, lowercase, out-of-range column) via `Option` and `checked_sub`.
- **Test coverage:** Two tests verify export behavior directly and via round-trip through `xlsx_import::import_xlsx_into_doc`, confirming data survival across serialization boundary.
- **No external dependencies within domain:** Uses only projection, workbook, and serde_json; all I/O delegated to `rust_xlsxwriter` crate.
