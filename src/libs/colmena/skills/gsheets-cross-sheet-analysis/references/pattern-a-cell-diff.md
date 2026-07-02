# Pattern A — Cell-by-cell diff

> **Use `data_run_python` for any analysis over >50 rows** — pass each sheet as a binding; the rows never load into LLM context. Use `gsheets_read` only for inspection / small reads / `value_render: "FORMULA"`. The code in this reference is the body of `data_run_python`s `code` argument; bind each sheet under the records-list name you pick (e.g. `records_a`, `records_b`).

> Same pattern as the crdt-doc equivalent — see `crdt-doc-cross-sheet-analysis` if you need the local-CRDT variant.

**When:** two versions of the same report that should have identical layout (same rows, same columns). You want to spot which cells changed value.

## Data flow

1. Call `data_run_python` binding each sheet directly (rows never enter context):
   `data_run_python({ bindings: [ {var: "records_a", spreadsheet_id: <id_a>, sheet: <tab_a>}, {var: "records_b", spreadsheet_id: <id_b>, sheet: <tab_b>} ], code: <below>, write_to_spreadsheet: <id_out> })`
2. The `code` computes the cell-level diff and assigns the `output_sheets` sink to write the diff table back; `output` carries a short text summary.

## Script

```python
import pandas as pd
a = pd.DataFrame(records_a)
b = pd.DataFrame(records_b)

common_cols = [c for c in a.columns if c in b.columns]
diff = a[common_cols].compare(
    b[common_cols].reindex(a.index),
    align_axis=1,
)
# DataFrame.compare returns a MultiIndex on columns — flatten for sheet storage
diff.columns = [f"{c}_{side}" for c, side in diff.columns]
diff_df = diff.reset_index(names='row_index')

# Write the diff table back into a "Diff" tab of the target spreadsheet, in the same call.
output_sheets = {"Diff": {"mode": "replace", "df": diff_df}}
output = f"{len(diff_df)} cells changed across {len(common_cols)} columns"
```

## Output

- The `output_sheets` write creates a `Diff` tab whose columns are `row_index, <colname>_self, <colname>_other, ...` for each column with at least one differing value.
- The write happens INSIDE the `data_run_python` call via the `output_sheets` sink (the DataFrame goes in the `df` field). You get back sink metadata (`{name, sheet_id, n_rows, n_cols}`) — never the row contents.
- `output` carries only the short summary string (e.g. `"12 cells changed across 5 columns"`).

**Anti-tip:** if shapes differ (one has extra rows), `DataFrame.compare` raises. In that case use Pattern B (row-diff by key) instead.
