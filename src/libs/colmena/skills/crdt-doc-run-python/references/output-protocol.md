# Output protocol

Your code must define **at least one** of two variables:

- `output` — any JSON-serializable value (dict, list, scalar). Returned to you in the response.
- `output_sheet` — a `pandas.DataFrame`. If you also passed `write_to_sheet: "<name>"`, this is persisted as a new sheet.

Both, one, or neither — all valid. Neither is treated as "side-effect only" (e.g. just `print` statements for debugging).

## `output` rules

- Any JSON-serializable value works. `to_dict()`, `to_dict('records')`, `value_counts().to_dict()` are common.
- Capped at **10 KB** after JSON serialization. Excess is truncated and the response includes `_output_truncated: true`.
- Do NOT return whole DataFrames here — they may exceed the cap. Use `df.head().to_dict('records')` or summaries.

## `output_sheet` rules

- Must be a `pandas.DataFrame` (anything else is silently ignored — your `write_to_sheet` will be a no-op).
- All column names must be strings. Default pandas types are fine.
- After `groupby(...).agg(...)` or similar, the index becomes a multi-level Index. Call `.reset_index()` so the result is a flat DataFrame before assigning to `output_sheet`.
- Max 100,000 rows per output sheet. Above that, the response includes `truncated_at: <row_count>`.

## `write_to_sheet` (string, optional)

- If `output_sheet` is a DataFrame AND `write_to_sheet` is set, the DataFrame is persisted as a new sheet in the CURRENT artifact with that name.
- Name collisions auto-suffix `" (2)"`, `" (3)"` … up to a 31-char limit.
- If you call run_python TWICE with the same `write_to_sheet` name, you'll get two sheets: `"X"` and `"X (2)"`. To overwrite intentionally, the user has to delete the previous one (not currently a tool — see BACKLOG).

## Example

```python
import pandas as pd
df = dfs[sid]
# ... promotion + analysis ...

agg = df.groupby('Region').agg({'Sales': 'sum', 'Qty': 'sum'}).reset_index()
output_sheet = agg                                  # persisted as sheet
output = f"Aggregated {len(agg)} regions, total Sales: {df['Sales'].sum()}"
```

Response shape (LLM-visible):
```json
{
  "output": "Aggregated 3 regions, total Sales: 12345.67",
  "wrote_sheet": {
    "sheet_id": "sh_01...",
    "name": "Summary by Region",
    "n_rows": 3,
    "n_cols": 3,
    "preview": [...first 5 rows...],
    "truncated_at": null
  },
  "stdout": "",
  "error": null
}
```
