---
name: crdt-doc-run-python
description: Use when working with the `crdt_doc_run_python` tool to analyze workbook sheets server-side with pandas/numpy/scipy.stats. Explains the DataFrame shape contract (row 1 of the Y.Doc becomes pandas columns), the output / output_sheet protocol, sandbox limits, and how to debug KeyError without looping. Activate as soon as you see `crdt_doc_run_python` in your tool list.
---

# crdt_doc_run_python — Operator Manual

The `crdt_doc_run_python` tool runs sandboxed Python (pandas + numpy + scipy.stats) against one or more sheets of the current CRDT workbook. The full data lives server-side; you analyze it without paying token cost for every row.

## The DataFrame shape contract — read this BEFORE writing code

For every `sheet_id` you pass in `sheet_ids`, the dispatcher projects the Y.Doc as a list of records and pandas builds `df = pd.DataFrame(records)`. The rule is:

**Y.Doc row 1 ALWAYS becomes the pandas column names.** Empty cells in that row are filled with `col_A`, `col_B`, … so the columns vector is never sparse.

The implication splits into two cases:

### Case A — "clean" sheet (headers in row 1)

If row 1 already contains the real headers (e.g. `Region | Sales | Qty`), you can use the DataFrame directly:

```python
df = dfs['<sheet_id>']
# df.columns = ['Region', 'Sales', 'Qty']  — already correct
totals = df.groupby('Region')['Sales'].sum()
```

### Case B — "title row" sheet (header in row 2, common in imported xlsx)

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

A `crdt_doc_read` call with range `A1:D5` is enough to tell which case you're in: if A1 is a string and B1/C1/D1 are empty, you're in Case B.

## Output protocol

Your code must define **at least one** of:

- `output` — any JSON-serializable value (dict, list, scalar). Returned to you in the response.
- `output_sheet` — a `pandas.DataFrame`. If you also passed `write_to_sheet: "<name>"`, this is persisted as a new sheet (name collisions auto-suffix `" (2)"`, `" (3)"` etc., max 31 chars).

Both, one, or neither — all valid. Neither is treated as "side-effect only" (e.g. just `print` statements).

`output_sheet` rules:

- Must be a `pandas.DataFrame` (anything else is silently ignored).
- All column names must be strings (pandas defaults are fine; `.reset_index()` first if grouping produced a MultiIndex).
- Max 100,000 rows per output sheet — beyond that the response includes `truncated_at`.

## Type quirks to remember

- **All numbers from Y.Doc arrive as `float64`** (Yjs has only one numeric type). `12` enters as `12.0`. If you need integer comparisons, cast: `df['Qty'].astype(int)`.
- **`Total`-style columns** in imported xlsx are often blank → arrive as `None`/`NaN`. Use `pd.to_numeric(..., errors='coerce')` before arithmetic.
- **Mixed-type columns** (e.g. a price column that has a stray text row) → also use `pd.to_numeric(..., errors='coerce')` and `.dropna()` if you want strict numeric behavior.

## Debugging — what to do when you hit an error

The response has a top-level `error` string when something went wrong (sandbox exception, write_to_sheet collision, timeout) AND a `loaded_sheet_columns` map showing the ACTUAL columns of every loaded sheet:

```json
{
  "error": "Python execution error: KeyError: 'Precio'",
  "loaded_sheet_columns": {
    "sh_01ABC…": ["Reporte Q3 2026", "col_B", "col_C", "col_D"]
  }
}
```

**Do not retry the same indexing assumption.** Read `loaded_sheet_columns` first:

- If it shows your expected column names → some other issue (typo, case mismatch, leading/trailing whitespace).
- If it shows a single string + `col_B/col_C/…` placeholders → you're in Case B above; apply the "promote real headers" pattern.

If you're still stuck, call `run_python` once with **just** `print(df.head(3)); print(list(df.columns))` and an empty `output`. The `stdout` field carries it back to you. That single call resolves 99% of shape confusion.

## Sandbox limits

- Imports whitelisted: `pandas`, `numpy`, `scipy`, plus `json`, `math`, `datetime`, `re`, `collections`. Other imports raise `ImportError` at AST-validation time.
- Timeout: **30 seconds** per call (hard cap).
- Output cap: `output` and `stdout` truncated at 10 KB each.
- Combined sheet load cap: 100 MB across all `sheet_ids` (you'll see `load_size_exceeded` if you ask for too many large sheets at once).

## Anti-patterns

- ❌ Reading the whole sheet with `crdt_doc_read` "to plan" your code. Use `crdt_doc_read A1:D5` for shape — let `run_python` see the rest.
- ❌ Retrying the same `df.iloc[N]` with different N values without reading `loaded_sheet_columns`.
- ❌ Hardcoding column positions (`df.iloc[:, 2]`) when you could use names (`df['Precio']`) — names are robust to column reorderings.
- ❌ Calling `run_python` once per row in a loop. Pass the whole sheet, do the loop in pandas.
- ❌ Returning the full DataFrame as `output` for inspection. Use `output_sheet` (persisted, fast) or `df.head().to_dict('records')` (small).
