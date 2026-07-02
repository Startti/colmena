---
name: cross_source_join
description: Bind two sources at once (e.g. an incoming table vs an existing DB table, or a gsheet vs a DB table), merge/diff in pandas, write only what changed to one or more sinks.
---

# Recipe: cross-source join (two live sources, one call)

Use this when the task genuinely spans two stores at once — "sync the team's
Google Sheet with the DB", "compare this CSV against what's already in
staging", "flag orders in the sheet that aren't in the database yet". The
distinguishing feature of this recipe (vs `spreadsheet_to_db`) is that
**both sinks can be written in the same call**, not just one.

## Shape — gsheet ↔ table sync

```jsonc
{
  "bindings": [
    {"var": "pipeline", "spreadsheet_id": "1abc", "sheet": "Pipeline"},
    {"var": "oportunidades", "query": "SELECT opp_id, stage, amount FROM crm.oportunidades"}
  ],
  "write_to_spreadsheet": "1abc",
  "code": "
import pandas as pd
df_sheet = pd.DataFrame(pipeline)
df_db = pd.DataFrame(oportunidades)

merged = df_sheet.merge(df_db, on='opp_id', how='left', suffixes=('', '_db'))

# push new/changed rows from the sheet into the DB
output_tables = {
    'crm.oportunidades': {'mode': 'upsert', 'df': merged[['opp_id', 'stage', 'amount']], 'key': 'opp_id'}
}

# mark the sheet rows that are now synced
merged['synced'] = True
output_sheets = {
    'Pipeline': {'mode': 'update_in_place', 'df': merged[['opp_id', 'synced']], 'key': 'opp_id', 'columns': ['synced']}
}

output = {'synced_rows': len(merged)}
"
}
```

## Shape — CSV vs existing table (find what's new)

```jsonc
{
  "bindings": [
    {"var": "incoming", "attachment_id": "doc_xy99"},
    {"var": "existing", "query": "SELECT order_id FROM staging.orders"}
  ],
  "code": "
import pandas as pd
df_in = pd.DataFrame(incoming)
df_ex = pd.DataFrame(existing)

new_rows = df_in[~df_in['order_id'].isin(df_ex['order_id'])]

output_tables = {'staging.orders': new_rows}  # shorthand = append
output = {'new_rows': len(new_rows), 'sample': new_rows.head(3).to_dict('records')}
"
}
```

## Why this shape

- **Every source you touch needs its own binding** — you cannot query the DB
  or read a sheet from inside the sandbox code; the sandbox has no network.
  If you need a third source (e.g. an attachment too), add a third binding.
- **`write_to_spreadsheet` is required** whenever your code assigns
  `output_sheets` — set it as a top-level tool arg (not inside the code)
  and it must match the `spreadsheet_id` you bound from/are writing to.
- **`update_in_place` on `output_sheets`** diff-writes only changed cells —
  it needs the same `key` you used to merge, and the snapshot from the
  original gsheet binding is what makes the diff possible. This only works
  when the sheet was bound in the same call (or a very recent one); it does
  not apply to SQL/attachment-sourced data.
- **Writing to two sinks in one call is atomic per sink**, not across sinks
  — `output_tables` writes are one Postgres transaction; `output_sheets`
  writes are a separate Sheets API batch. If you need "both or neither"
  guarantees across stores, that's not offered here; report partial
  progress honestly if one sink fails.
