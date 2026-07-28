# src/libs/colmena/src/gdocs/application/table_format.rs

**Layer:** application  **Purpose:** Implements table-cell formatting use case for Google Docs: validates cell ranges, constructs Docs API batchUpdate requests for styling (background, borders, text, paragraph alignment), and applies all ops in one atomic call.

## Symbols

### Public API
- `CellRange` (struct, pub) — Value object: 0-based, end-exclusive rectangle of cells (row_start, row_end, col_start, col_end)
- `TableFormatOp` (struct, pub) — Value object: one formatting operation (table_index, cell_range, format attributes)
- `CellFormat` (struct, pub) — Value object: all-optional formatting attributes (background_color, vertical_alignment, borders, text, horizontal_alignment)
- `CellTextFormat` (struct, pub) — Value object: text styling attributes (bold, italic, underline, strikethrough, font_size, color)
- `CellBorders` (struct, pub) — Value object: optional per-side borders (top, bottom, left, right)
- `CellBorderSide` (struct, pub) — Value object: one border side (style, color, width_pt)
- `hex_to_rgb` (fn, pub(crate)) — Parse `#RRGGBB` hex string to normalized RgbColor (0.0..=1.0); rejects non-6-digit hex
- `validate_range` (fn, pub(crate)) — Validate cell range is non-empty and within table bounds; checked arithmetic, never panics on adversarial input
- `build_format_table_requests` (fn, pub(crate)) — Build Docs API batchUpdate requests for one op: one `updateTableCellStyle` over rectangle + per-cell `updateTextStyle`/`updateParagraphStyle` requests
- `run_format_table` (async fn, pub) — Main use case: run co-edit guard, validate all ops' ranges, accumulate batchUpdate requests, finalize in single atomic call

### Private Helpers
- `rgb_to_color` (fn, private) — Convert RgbColor to Docs API OptionalColor JSON shape `{ "color": { "rgbColor": {r, g, b} } }`
- `build_text_style` (fn, private) — Build textStyle map and fields list from optional CellTextFormat; silently skips invalid hex colors (intentional v1 simplification to avoid Result plumbing)

### Tests (in `#[cfg(test)]` module)
- `cell` (helper fn) — Construct CellSnapshot for test fixtures
- `table_2x2` (helper fn) — Construct 2x2 TableSnapshot with cells at known content indices
- `hex_parses_and_rejects` (test) — hex_to_rgb happy/sad paths (valid `#FF8000`, reject `#FFF`, reject garbage)
- `validate_range_rejects_empty_and_oob` (test) — Range validation: reject row_start==row_end, reject out-of-bounds
- `cell_level_only_emits_one_table_cell_style` (test) — Single updateTableCellStyle for background-color-only format
- `text_only_emits_text_and_no_cell_style` (test) — Per-cell updateTextStyle requests, no cell-level style when format has only text attrs
- `horizontal_alignment_emits_paragraph_style_per_cell` (test) — One updateParagraphStyle per cell in range
- `borders_set_all_requested_sides_on_cell_style` (test) — Border field construction, defaults (1pt, black), OptionalColor shape
- `snap_with_one_table` (helper fn) — DocumentSnapshot with 1x2 table at known indices for integration tests
- `run_format_table_emits_one_batch_update` (integration test, tokio::test) — End-to-end: runs guard, accumulates cell-style + text-style requests, verifies exactly one batch_update call
- `run_format_table_rejects_empty_ops` (test, tokio::test) — Rejects empty ops array with InvalidArgs error

## File-level notes

- **Hexagonal compliance:** Clean separation — domain types (CellFormat, RgbColor) at the boundary, `DocsClient` calls delegated to `apply_and_finalize` (infrastructure). Use case `run_format_table` is pure application.
- **Error handling:** Checked arithmetic (`range.row_end - range.row_start`), never panics on adversarial input. Validation errors are descriptive DocsError::InvalidArgs.
- **Intentional asymmetry (line 224):** Text-color hex failures are silently skipped (`if let Ok`), whereas cell-level background/border colors fail the entire op (`?`). Documented as v1 simplification to avoid Result plumbing in `build_text_style`. Not a bug, but a known limitation for future unification.
- **Request accumulation:** Per-op iteration in `run_format_table` (line 294) collects all requests into a single batchUpdate, ensuring atomicity. Empty ops and empty-format-only ops both rejected with clear messages.
- **Test coverage:** Unit tests cover hex parsing, range validation, cell-style, text-style, paragraph-style, and border construction; integration test verifies the full `run_format_table` flow with MockDocsClient.
