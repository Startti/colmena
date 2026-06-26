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

## Writing a live FORMULA — reference columns by name with `{{Name}}`

To make `Importe` a formula instead of a computed value, put the **column name**
in double braces; the dispatcher resolves it to the real A1 of the **same row**:

```python
df = pd.DataFrame(sheet_data)
mask = df['CLIENT ID'] == 'TCIb1afd2...'
df.loc[mask, 'Importe'] = '={{Cantidad}}*{{Tarifa}}'   # → e.g. =S5*U5, =S6*U6 ...
output_sheets = {'Hoja 16': {'mode': 'update_by_position', 'df': df}}
```

- `{{Cantidad}}` → that column's cell in the row being written (current row only).
- **Never compute column letters yourself.** Deriving them from `df.columns`
  order is off-by-one whenever the sheet has an empty or duplicate header column
  (a very common case), and lands a broken `#VALUE!` formula in the wrong refs.
  Let the tool do it — it reads the real header positions.
- An unknown / misspelled column name returns a `FormulaUnknownColumn` error that
  lists the valid column names, so you can fix it and retry.
- Single braces (`={1,2;3,4}` array literals) are left untouched — only `{{ }}`
  is a column reference.
- Current-row references only. For an aggregate like `=SUM(Importe2:Importe100)`
  write the literal A1 range yourself.

### Filling a formula down many rows

`{{Name}}` resolves per row, so to fill a formula across a column / range /
subset, just assign it to those rows — the tool puts the correct row number in
each cell:

```python
df['Importe'] = '={{Cantidad}}*{{Tarifa}}'                        # whole column
df.loc[df['Categoria'] == 'Bebidas', 'Margen'] = '={{Venta}}-{{Costo}}'  # by condition — PREFERRED
df.loc[df.index[0:30], 'Subtotal'] = '={{Precio}}*{{Unidades}}'   # first 30 data rows
```

- **Prefer selecting rows by a condition** (`df.loc[df['X']=='Y', …]`) — no row
  math. If the user names literal sheet rows, map them: `df_index = sheet_row - 2`
  (row 1 is the header), so sheet rows 2–31 → `df.index[0:30]`.
- A fill over >50 cells returns `formula_cells_total` + `formula_cells_truncated`
  in the result. Report the real total ("applied to all 812 rows"), not the
  50-cell sample.
- **When you confirm to the user, quote the tool result's `formula_cells`**
  (real `cell → formula`, e.g. `{"V5": "=S5*U5"}`). Do NOT recompute column
  letters yourself to build the message — that hand math is off-by-one, so your
  confirmation would report the wrong cell/refs even though the write was right.

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
