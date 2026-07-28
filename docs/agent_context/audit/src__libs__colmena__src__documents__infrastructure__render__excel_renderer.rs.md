# src/libs/colmena/src/documents/infrastructure/render/excel_renderer.rs

**Layer:** infrastructure  
**Purpose:** Implements ExcelIR → XLSX binary serialization via rust_xlsxwriter; concrete adapter for the IRRenderer port.

## Symbols

- `ExcelRenderer` (struct, pub) — empty marker struct implementing the IRRenderer trait for Excel workbook rendering
- `ExcelRenderer::render_sync` (fn, private) — core synchronous logic: builds rust_xlsxwriter::Workbook from ExcelIR, applies sheets/columns/cells/tables/styles, returns binary buffer
- `impl IRRenderer for ExcelRenderer` (impl, pub) — trait implementation providing async `render()` wrapper, target file extension ("xlsx"), and MIME type
  - `render()` (method, pub) — deserializes JSON Value to ExcelIR, delegates to render_sync()
  - `target_extension()` (method, pub) — returns "xlsx"
  - `target_mime()` (method, pub) — returns "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
- `infer_type()` (fn, private) — heuristic CellType detection: number→Number, boolean→Boolean, string starting with '='→Formula, else→String
- `write_cell()` (fn, private) — dispatches cell value to rust_xlsxwriter based on CellType: String/Number/Boolean/Date as text, Formula via Formula::new()
- `parse_a1()` (fn, private) — parses Excel A1 notation (e.g., "B5", "AA1") to (row: u32, col: u16) tuple; rejects invalid addresses and row==0
- `parse_range()` (fn, private) — parses Excel range notation (e.g., "A1:B10") to pair of (row, col) tuples
- `tests::renders_minimal_xlsx()` (fn, test, private) — tokio integration test verifying ExcelIR renders to valid XLSX binary (PK magic bytes)
- `tests::parses_a1()` (fn, test, private) — unit tests for parse_a1() covering single/double columns and edge cases
- `tests::calamine_can_reopen_rendered_xlsx()` (fn, test, private) — tokio integration test: renders IR, reopens with calamine reader, verifies cell content round-trip

## File-level notes

- **Error propagation**: All fallible operations return `Result<_, RenderError>` with descriptive context (parse IR, set_name, set_column_width, add_table, save_to_buffer). No silent failures.
- **Hex color parsing (lines 45–50, 53)**: Accepts `#RRGGBB` format, trims `#` prefix, uses `u32::from_str_radix(..., 16)` with error silencing (silently omits color if parse fails). Acceptable fallback.
- **Type conversion fallbacks (lines 113, 117, 122–123)**: Non-matching JSON types use defaults (0.0 for missing numbers, false for missing booleans, empty string for missing dates). Data loss is silent but reasonable given external JSON input.
- **CellType::Date handling (lines 121–124)**: Writes dates as text strings; does not convert ISO date strings to Excel's internal date number (epoch-relative). If IR includes date strings + date format string in `cell.format`, the format is applied correctly. If numeric date conversion is needed, it would happen upstream in IR generation, not here.
- **Style resolution pattern (lines 32–58)**: Multi-level optional chaining (cell.style_ref → named_styles[sref] → style.font/fill) uses idiomatic Option unwrapping; skips style application if any link is None.
- **Sorting determinism**: Sheets are sorted by `order` field before iteration (line 11–12), ensuring stable output order.
- **Test coverage**: 3 tokio/standard tests cover rendering, A1 parsing, and full round-trip via external calamine reader; no panics in tests (only unwrap on known-good values).
- **No dead code detected**: All private functions (render_sync, infer_type, write_cell, parse_a1, parse_range) are invoked from render_sync or trait methods.
