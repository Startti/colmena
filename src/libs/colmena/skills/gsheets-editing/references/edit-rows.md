# Editing existing rows

First know the schema: if you don't already know the real column names, read the
tab once with `gsheets_read`. Then edit with `update_by_position` — it works
whether or not any column is unique, and you never compute an A1 address.

## Primary way — `update_by_position` (no key, no A1 math)

Bind the **whole sheet** (no `range`), modify the bound DataFrame **in place**,
and return the **whole** df:

```python
import pandas as pd
df = pd.DataFrame(sheet_data)                 # `sheet_data` = a whole-sheet binding
mask = df['CLIENT ID'] == 'TCI28fa...'
df.loc[mask, 'Importe'] = df['Cantidad'] * df['Tarifa']   # in place
output_sheets = {'Hoja 16': {'mode': 'update_by_position', 'df': df}}   # WHOLE df
output = {'edited': int(mask.sum())}
```

The dispatcher diffs the returned df against what you loaded and writes only the
changed cells back to the correct rows/columns. You do **not** compute row
numbers or column letters, and the matching value does **not** need to be unique
(a repeated `CLIENT ID` is fine).

**Rules (the tool rejects violations with a clear message):**
- Return the **WHOLE** df — do NOT return a filtered subset (`df[mask]`). Modify
  in place; the filter is only for selecting *which cells to set*.
- Change ONLY the target cells (`df.loc[mask, 'col'] = ...`). Do NOT reassign a
  WHOLE column (e.g. `df['Tarifa'] = pd.to_numeric(df['Tarifa'])`) — that rewrites
  every cell that coercion changes, not just the ones you meant to edit. If you
  need a numeric version for a calculation, compute it into a LOCAL variable and
  assign only the masked target column.
- Do NOT `reset_index()`, `sort()` + `reset_index(drop=True)`, or `concat()` the
  bound df — those break the row mapping.
- Do NOT add rows with this mode (it edits existing rows only). Columns whose
  header name is empty or duplicated can't be addressed and are reported in
  `skipped_columns`.

## Alternative — `update_in_place` (only when a column is truly UNIQUE)

If the tab has a genuine primary key, you can patch by key instead of position:

```python
df.loc[df['product_id'] == 'P-100', 'price'] = 19.9
output_sheets = {'Sales': {'mode': 'update_in_place', 'df': df,
                           'key': 'product_id', 'columns': ['price']}}
```

`update_in_place` requires `key` values to be UNIQUE — it errors on duplicates.
When in doubt, use `update_by_position`.

## Never
- `overwrite` to change a few rows → it replaces the whole tab, destroying
  formatting, formulas, and untouched columns.
