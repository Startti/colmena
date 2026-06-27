# Creating containers and filling them

Three different "new" — pick by what you're creating.

## New TAB with data → `gsheets_run_python` (one call creates AND fills)

Name a sheet that does NOT exist yet in `output_sheets`. The dispatcher creates
the tab and writes the rows in one shot — no separate `add_sheet` + `set_range`.

```python
import pandas as pd
src = pd.DataFrame(sales)
summary = src.groupby('region')['total'].sum().reset_index()
output_sheets = {
    'Resumen por Región': summary,        # new tab name → created + populated
}
output = {'created_tab': 'Resumen por Región', 'rows': len(summary)}
```

You can mix several new tabs in one call:

```python
output_sheets = {
    'Detalle': enriched,
    'Resumen': enriched.groupby('category')['total'].sum().reset_index(),
}
```

The model only gets metadata back (`{name, resolved_name, sheet_id, n_rows,
n_cols}`), never the row contents. If a name collides with an existing tab the
default policy fails with a `SheetExists` error — pick a different name (or use
`update_in_place`/`overwrite` deliberately).

## New EMPTY tab → `gsheets_add_sheet`

When you just need a blank tab (you'll fill it later with set_cell/set_range or a
later run_python):

`gsheets_add_sheet` with `spreadsheet_id` + the new tab `name`.

## New SPREADSHEET FILE → `gsheets_create_spreadsheet`

Only when there is no workbook yet (prefer asking the user to share an existing
one). It returns the new `spreadsheet_id`; then populate it with run_python
(`output_sheets`) or set_range. To import an existing `.xlsx`, use
`gsheets_create_from_xlsx` instead (uploads the attachment as a new Sheet).
