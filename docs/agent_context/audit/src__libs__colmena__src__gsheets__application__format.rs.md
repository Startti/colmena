# src/libs/colmena/src/gsheets/application/format.rs

**Layer:** application  **Purpose:** Pure mapping from declarative `FormatSpec` to Google Sheets `spreadsheets.batchUpdate` requests. Includes A1 range parsing, hex color conversion, and JSON serialization of formatting directives. No I/O or infrastructure dependencies.

## Symbols

- `GridRange` (struct, pub) — 0-based, end-exclusive grid rectangle (sheet_id, start_row, end_row, start_col, end_col)
- `GridRange.sheet_id` (field, pub) — Google Sheets sheet ID
- `GridRange.start_row` (field, pub) — Start row index (inclusive)
- `GridRange.end_row` (field, pub) — End row index (exclusive)
- `GridRange.start_col` (field, pub) — Start column index (inclusive)
- `GridRange.end_col` (field, pub) — End column index (exclusive)
- `FormatSpec` (struct, pub) — Declarative format specification with optional text, background, alignment, number format, borders, and sizing
- `FormatSpec.text` (field, pub) — Text styling (bold, italic, underline, strikethrough, font, color)
- `FormatSpec.background_color` (field, pub) — Hex #RRGGBB cell background color
- `FormatSpec.horizontal_alignment` (field, pub) — Horizontal alignment (LEFT | CENTER | RIGHT)
- `FormatSpec.vertical_alignment` (field, pub) — Vertical alignment (TOP | MIDDLE | BOTTOM)
- `FormatSpec.number_format` (field, pub) — Number format type and pattern
- `FormatSpec.wrap` (field, pub) — Wrap strategy (OVERFLOW | CLIP | WRAP)
- `FormatSpec.borders` (field, pub) — Border specifications for all sides and inner grid lines
- `FormatSpec.column_width_px` (field, pub) — Column width in pixels (applied to all columns in range)
- `FormatSpec.row_height_px` (field, pub) — Row height in pixels (applied to all rows in range)
- `TextFormat` (struct, pub) — Text styling with optional bold, italic, underline, strikethrough, font size, family, and color
- `TextFormat.bold` (field, pub) — Bold flag
- `TextFormat.italic` (field, pub) — Italic flag
- `TextFormat.underline` (field, pub) — Underline flag
- `TextFormat.strikethrough` (field, pub) — Strikethrough flag
- `TextFormat.font_size` (field, pub) — Font size in points
- `TextFormat.font_family` (field, pub) — Font family name
- `TextFormat.color` (field, pub) — Hex #RRGGBB text (foreground) color
- `NumberFormat` (struct, pub) — Number format type and pattern
- `NumberFormat.r#type` (field, pub) — Number format type (NUMBER | CURRENCY | PERCENT | DATE | TIME | DATE_TIME | TEXT | SCIENTIFIC)
- `NumberFormat.pattern` (field, pub) — Optional format pattern (e.g. "$#,##0.00" for currency)
- `Borders` (struct, pub) — Border specifications for top, bottom, left, right, inner horizontal, and inner vertical
- `Borders.top` (field, pub) — Top border specification
- `Borders.bottom` (field, pub) — Bottom border specification
- `Borders.left` (field, pub) — Left border specification
- `Borders.right` (field, pub) — Right border specification
- `Borders.inner_horizontal` (field, pub) — Inner horizontal grid line specification
- `Borders.inner_vertical` (field, pub) — Inner vertical grid line specification
- `BorderSide` (struct, pub) — Single border specification with style and optional color
- `BorderSide.style` (field, pub) — Border style (SOLID | SOLID_MEDIUM | SOLID_THICK | DASHED | DOTTED | DOUBLE)
- `BorderSide.color` (field, pub) — Hex #RRGGBB border color (defaults to black when omitted)
- `FormatError` (struct, pub) — Error type for format mapping failures (wraps String)
- `hex_to_rgb` (fn, pub) — Parse #RRGGBB (or RRGGBB) to Sheets RgbColor JSON with 0.0–1.0 floats; validates 6-digit hex and returns error on invalid format
- `a1_to_grid_range` (fn, pub) — Parse A1 range notation ("A1", "A1:D5", "B:D", "2:5") to 0-based, end-exclusive GridRange; supports single cells, ranges, whole columns, and whole rows
- `parse_a1_cell` (fn, private) — Parse one A1 token (e.g. "A", "1", "B12") to (col_index?, row_index?) 0-based; col reference uses base-26 overflow-checked arithmetic
- `grid_json` (fn, private) — Convert GridRange to Sheets gridRange JSON object; omits row/column indices when start==end (whole-row/column case)
- `border_json` (fn, private) — Convert BorderSide to Sheets border JSON with RGB color (defaults to black); uses hex_to_rgb
- `build_format_requests` (fn, pub) — Map FormatSpec over GridRange to Vec of Sheets batchUpdate request objects; includes repeatCell (for format), updateBorders, and updateDimensionProperties (for sizing); errors if spec has no attributes; builds partial-update fieldMasks to avoid overwriting unset fields
- `tests` (mod, private) — Unit test suite covering hex_to_rgb (with/without #), a1_to_grid_range (single cell, range, whole columns/rows, multi-letter columns, overflow), and build_format_requests (composite formatting, border + width separation, empty spec error, multi-attribute fan-out)

## File-level notes

- Well-structured pure-mapping module with comprehensive test coverage (7 tests spanning color parsing, A1 parsing, and request generation)
- A1 parser handles edge cases correctly: column references use checked overflow arithmetic; empty ranges/tokens error; whole-row/column ranges emit 0..0 for the omitted axis
- build_format_requests uses partial-update fieldMasks to avoid unintended overwrites on the Sheets API side
- No infrastructure dependencies; all errors are deterministic and fail-closed with descriptive messages
- Referenced by design spec: `docs/superpowers/specs/2026-06-22-gsheets-cell-formatting-design.md`
