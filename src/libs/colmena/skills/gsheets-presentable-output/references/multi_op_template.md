# Multi-op template — copy and adapt the ranges

A typical report: title row 1, header row 3, data rows 4-7, totals row 8,
columns A..F. Adapt the ranges to your sheet, then send as ONE call:

```json
{
  "spreadsheet_id": "<id>",
  "ops": [
    { "sheet": "<tab>", "range": "A1:F1",
      "format": { "text": { "bold": true, "font_size": 16 }, "horizontal_alignment": "CENTER" } },
    { "sheet": "<tab>", "range": "A3:F3",
      "format": { "text": { "bold": true, "color": "#FFFFFF" }, "background_color": "#1F4E78",
                  "horizontal_alignment": "CENTER",
                  "borders": { "bottom": { "style": "SOLID_THICK" } } } },
    { "sheet": "<tab>", "range": "B4:F8",
      "format": { "number_format": { "type": "CURRENCY", "pattern": "$#,##0" } } },
    { "sheet": "<tab>", "range": "A3:F8",
      "format": { "borders": { "top": {"style":"SOLID"}, "bottom": {"style":"SOLID"},
                  "left": {"style":"SOLID"}, "right": {"style":"SOLID"},
                  "inner_horizontal": {"style":"SOLID"}, "inner_vertical": {"style":"SOLID"} } } },
    { "sheet": "<tab>", "range": "A8:F8",
      "format": { "text": { "bold": true }, "background_color": "#D9D9D9",
                  "borders": { "top": { "style": "SOLID_THICK" } } } },
    { "sheet": "<tab>", "range": "A3:A8", "format": { "column_width_px": 150 } },
    { "sheet": "<tab>", "range": "B3:F8", "format": { "column_width_px": 100 } }
  ]
}
```

This is non-destructive: it does not touch values/formulas, and each op's
`fields` mask only changes the attributes you set.
