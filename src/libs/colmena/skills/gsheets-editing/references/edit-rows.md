# Editing existing rows

First know the schema: if you don't already know the real column names, read the
tab once with `gsheets_read`. Then choose by whether the matching column is unique.

## Case A — the key column is UNIQUE (e.g. `product_id`)

Patch only the changed cells with `update_in_place`:

```python
import pandas as pd
df = pd.DataFrame(sales)                 # `sales` = a sheet binding
df.loc[df['product_id'] == 'P-100', 'price'] = 19.9
output_sheets = {
    'Sales': {
        'mode': 'update_in_place',
        'df': df,
        'key': 'product_id',             # MUST be unique across rows
        'columns': ['price'],            # optional: only patch these columns
    }
}
output = {'updated': 'P-100'}
```

`update_in_place` diffs the new DataFrame against the live tab by `key` and writes
only the changed cells in one batchUpdate. It needs the key to be unique so each
new row maps to exactly one existing row.

## Case B — the matching value REPEATS (no unique single column)

`update_in_place` would fail here with a duplicate-key error (it can't tell which
of the N identical rows maps to which). Instead, find the ROW NUMBERS in code,
then write each cell with `gsheets_set_cell`.

Step 1 — locate the rows and the column (run_python; rows never leave the model):

```python
import pandas as pd
df = pd.DataFrame(data)                  # `data` = the sheet binding
mask = df['CLIENT ID'] == 'TCI28fa...'
# DataFrame index excludes the header row → sheet row = index + 2
rows = [int(i) + 2 for i in df.index[mask]]
col_idx = list(df.columns).index('Cantidad')   # 0-based position in the header
def col_letter(n):                       # 0 -> A, 18 -> S
    s = ''
    n += 1
    while n:
        n, r = divmod(n - 1, 26)
        s = chr(65 + r) + s
    return s
col = col_letter(col_idx)
output = {'cells': [f'{col}{r}' for r in rows]}   # e.g. ["S20","S21","S22",...]
```

Step 2 — for each address in `output['cells']`, call `gsheets_set_cell` with that
`addr` and the new `value` (e.g. 555). One call per cell. This is surgical: it
touches only those cells and preserves everything else (formatting, formulas,
other columns).

### Why not the alternatives here
- `update_in_place` → duplicate-key failure (the value isn't unique).
- `overwrite` → replaces the WHOLE tab, destroying formatting/formulas/untouched
  columns. Never acceptable just to change a few rows.
