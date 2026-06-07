# 06 — Output shaping

The result of your analysis can flow back in two ways. Pick based on
the size of the result and where the human is going to look.

## Small result → return via `output`

For top-10 lists, summary statistics, single-row answers — keep it in
the conversation. The LLM cap is 10 KB for `output`.

```python
import pandas as pd
df = pd.DataFrame(dfs["products"])
top_5 = df.nlargest(5, 'price')[['product_id', 'name', 'price']]
output = {
    'top_5_by_price': top_5.to_dict('records'),
    'total_products': len(df),
}
```

## Tabular result → write back as a new tab

For full tables of results (top-100, all groups, anomaly lists), write
to a new tab in the **current artifact**. Use `output_sheets` (dict of
name→DataFrame):

```python
import pandas as pd
df = pd.DataFrame(dfs["sales"])

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
  "sheet_ids": ["sales"],
  "code": "<script that sets output_sheets>"
}
```

There is no `write_to_spreadsheet` arg in `crdt_doc_run_python`. New
tabs created via `output_sheets` always land in the **current artifact**
(the one `ctx.artifact_id` points at).

The dispatcher creates 3 new tabs. The LLM only receives metadata
(`name`, `resolved_name`, `sheet_id`, `n_rows`, `n_cols`) per tab. The
row contents NEVER pass through your context.

## Back-compat: the legacy single-sheet path

The older path with `output_sheet = <df>` + `write_to_sheet: "<name>"`
still works for one tab at a time. New code should use `output_sheets`
(the dict form) — it supports N tabs, the dispatcher prefers it when
both paths are set, and a `_warning` surfaces in that case.

## Naming new tabs

If a tab name already exists in the current artifact, the dispatcher
auto-suffixes ` (2)`, ` (3)`, etc. The response includes both `name`
(what you asked for) and `resolved_name` (what was actually created).
