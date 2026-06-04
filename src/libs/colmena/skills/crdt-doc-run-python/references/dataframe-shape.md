# DataFrame shape contract — read this BEFORE writing code

For every `sheet_id` you pass in `sheet_ids`, the dispatcher projects the Y.Doc as a list of records and pandas builds `df = pd.DataFrame(records)`. The rule is:

**Y.Doc row 1 ALWAYS becomes the pandas column names.** Empty cells in that row are filled with `col_A`, `col_B`, … so the columns vector is never sparse.

The implication splits into two cases.

## Case A — "Clean" sheet (headers in row 1)

If row 1 already contains the real headers (e.g. `Region | Sales | Qty`), the DataFrame is ready to use:

```python
df = dfs['<sheet_id>']
# df.columns = ['Region', 'Sales', 'Qty']  — already correct
totals = df.groupby('Region')['Sales'].sum()
```

## Case B — "Title row" sheet (header in row 2, common in imported xlsx)

If row 1 is a single title cell (e.g. `Reporte Q3 2026` in A1, B1/C1/D1 empty), then:

- `df.columns` is `['Reporte Q3 2026', 'col_B', 'col_C', 'col_D']` — garbage for analysis.
- `df.iloc[0]` is `{'Reporte Q3 2026': 'Producto', 'col_B': 'Cantidad', …}` — **these are the REAL headers**.
- `df.iloc[1]` is the first data row — `{'Reporte Q3 2026': 'SKU-0001', …}` — **NOT a header**.

The canonical "promote real headers" pattern:

```python
df = dfs['<sheet_id>']
df.columns = df.iloc[0].tolist()       # row that LOOKS like Producto/Cantidad/Precio/Total
df = df.iloc[1:].reset_index(drop=True) # drop the row you just promoted
```

## How to tell which case you're in

A `crdt_doc_read` call with range `A1:D5` is enough: if A1 is a string and B1/C1/D1 are empty, you're in **Case B**. Otherwise **Case A**.

When in doubt, do a quick probe with `print(list(df.columns))` and `print(df.head(3))` inside a `run_python` call with `output = ''` — the stdout will tell you exactly what you have.
