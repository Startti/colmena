# Output protocol

Your code must define **at least one** of two variables:

- `output` — any JSON-serializable value (dict, list, scalar). Returned to you in the response.
- `output_sheets` — a dict of `{tab_name: pandas.DataFrame}`. Each entry is persisted as a new (or updated) tab in the current artifact.

Both, one, or neither — all valid. Neither is treated as "side-effect only" (e.g. just `print` statements for debugging).

## `output` rules

- Any JSON-serializable value works. `to_dict()`, `to_dict('records')`, `value_counts().to_dict()` are common.
- Capped at **10 KB** after JSON serialization. Excess is truncated and the response includes `_output_truncated: true`.
- Do NOT return whole DataFrames here — they may exceed the cap. Use `df.head().to_dict('records')` or summaries.

## `output_sheets` rules

Each entry can be a bare DataFrame (mode defaults to `replace`) or a spec dict:

```python
# Mode 1 — replace (default; tab must not exist or collision policy applies)
output_sheets = {'Summary': summary_df}

# Mode 2 — update_in_place (patch specific cells, never overwrites a full tab)
output_sheets = {
    'Sales': {
        'mode': 'update_in_place',
        'df': df_modified,
        'key': 'product_id',    # column identifying rows (must be unique)
        'columns': ['price'],   # optional — only patch these columns
    }
}

# Mode 3 — overwrite (replace existing tab; explicit consent)
output_sheets = {'Sales': {'mode': 'overwrite', 'df': df}}
```

**Collision policy.** By default, if a tab name already exists in the artifact, the tool fails with a `SheetExists` error returning metadata and three suggested next moves. Use `update_in_place` or `overwrite` to proceed deliberately.

All column names must be strings. Default pandas types are fine.

After `groupby(...).agg(...)` or similar, the index becomes a multi-level Index. Call `.reset_index()` so the result is a flat DataFrame before assigning to `output_sheets`.

Max 100,000 rows per output sheet. Above that, the response includes `truncated_at: <row_count>`.

## Example

```python
import pandas as pd
df = dfs[sid]
# ... promotion + analysis ...

agg = df.groupby('Region').agg({'Sales': 'sum', 'Qty': 'sum'}).reset_index()
output_sheets = {'Summary by Region': agg}
output = f"Aggregated {len(agg)} regions, total Sales: {df['Sales'].sum()}"
```

Response shape (LLM-visible):
```json
{
  "output": "Aggregated 3 regions, total Sales: 12345.67",
  "written_sheets": [
    {
      "name": "Summary by Region",
      "resolved_name": "Summary by Region",
      "sheet_id": "sh_01...",
      "n_rows": 3,
      "n_cols": 3
    }
  ],
  "stdout": "",
  "error": null
}
```
