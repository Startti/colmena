# Recipe — a professional report, end to end

Order matters. Do these in sequence:

1. **Plan the layout.** Decide the columns and where the header row, data
   rows, and totals row will land (e.g. header row 1, data rows 2..N, totals
   row N+1).
2. **Write values + formulas** with `gsheets_set_range` (USER_ENTERED — a
   string starting with `=` becomes a formula). Use `=SUM(...)` for totals,
   not hardcoded numbers.
3. **Format LAST**, in ONE `gsheets_format_range` multi-op call, over the now-
   populated ranges:
   - Header row: bold, white text, dark background, centered, bottom border.
   - Numeric columns: a `number_format` (currency / percent / date as fits).
   - Whole table: thin borders on all cells.
   - Totals row: bold, light-gray background, top border to separate it.
   - Column widths: label column wider, numeric columns even.
4. **Report** the spreadsheet URL.

See `multi_op_template` for a copy-paste ops payload, `palettes` for colors,
`number_formats` for pattern strings.
