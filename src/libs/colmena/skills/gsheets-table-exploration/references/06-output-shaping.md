# 06 — Output shaping

The result of your analysis can flow back in two ways. Pick based on
the size of the result and where the human is going to look.

## Small result → return via `output`

For top-10 lists, summary statistics, single-row answers — keep it in
the conversation. The LLM cap is 10 KB for `output`.

```python
import pandas as pd
df = pd.DataFrame(products)
top_5 = df.nlargest(5, 'price')[['product_id', 'name', 'price']]
output = {
    'top_5_by_price': top_5.to_dict('records'),
    'total_products': len(df),
}
```

## Tabular result → write back as a new tab

For full tables of results (top-100, all groups, anomaly lists), write
to a new tab in the spreadsheet. Use `output_sheets` (dict of
name→DataFrame) and pass `write_to_spreadsheet`:

```python
import pandas as pd
df = pd.DataFrame(sales)

by_region = df.groupby('region')['total'].sum().reset_index()
top10     = df.nlargest(10, 'total')
all_data  = df.assign(margin=df['total'] - df['cost'])

output_sheets = {
    'By Region':     by_region,
    'Top 10 Sales':  top10,
    'With Margin':   all_data,
}
output = {'tabs_written': 3}
```

Call shape:

```json
{
  "bindings": [{"var": "sales", "spreadsheet_id": "<id>", "sheet": "Sales"}],
  "code": "<script that sets output_sheets>",
  "write_to_spreadsheet": "<target_spreadsheet_id>"
}
```

The dispatcher creates 3 new tabs. The LLM only receives metadata
(`name`, `resolved_name`, `sheet_id`, `n_rows`, `n_cols`) per tab. The
row contents NEVER pass through your context.

## When `output_sheets` is misconfigured

- `write_to_spreadsheet` set but no `output_sheets` in code: dispatcher
  emits a `_warning`.
- `output_sheets` set but no `write_to_spreadsheet` arg: same warning.
  The result is discarded; no tabs created.

Read the warning and fix the call.

## Naming new tabs

If a tab named `By Region` already exists in the target, the dispatcher
auto-suffixes ` (2)`, ` (3)`, etc. (capped at 10). The response
includes both `name` (what you asked for) and `resolved_name` (what was
actually created).
