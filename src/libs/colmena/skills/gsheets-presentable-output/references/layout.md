# Layout rules

- **Alignment:** text/labels left, numbers right (`horizontal_alignment`).
  Center only the header row.
- **Column widths:** the label column wider (~140-160px), numeric columns
  even (~90-110px). Set via `column_width_px` over the column's range.
- **Borders:** thin `SOLID` on the whole data block; use a `SOLID_THICK`
  bottom border under the header and a `SOLID_THICK` top border above the
  totals row to separate sections.
- **Totals row:** bold + light-gray background + top border. Keep numbers
  right-aligned and in the same number format as the data.
- **Zebra (optional, long tables):** subtle `#F3F3F3` background on odd data
  rows — only when the table is long enough to need row tracking.
- Don't over-format: one header color, optional zebra, one totals highlight.
