---
name: spreadsheet_to_db
description: "Uploaded file (or Google Sheet) in, DB out. Read an attachment/gsheet, diff or transform it in pandas, upsert the result into a SQL table. The canonical \"update the DB from this Excel\" flow."
---

# Recipe: spreadsheet/gsheet → DB (upsert)

Use this when the user hands you a file (or points you at a Google Sheet)
and wants the database updated to match it — "here's the new price list,
impact it in the DB", "this Sheet has the latest inventory counts, sync it".

## Shape

Bind BOTH the incoming data and the current DB state in ONE call, so you can
diff in pandas instead of blindly overwriting:

```jsonc
{
  "bindings": [
    {"var": "nuevo",  "attachment_id": "doc_ab12"},
    {"var": "actual", "query": "SELECT sku, precio, stock FROM crm.productos"}
  ],
  "code": "
import pandas as pd
df_new = pd.DataFrame(nuevo)
df_cur = pd.DataFrame(actual)

# normalize the join key before merging — trailing spaces / case mismatches
# are the #1 cause of a merge silently missing rows
df_new['sku'] = df_new['sku'].str.strip().str.upper()
df_cur['sku'] = df_cur['sku'].str.strip().str.upper()

merged = df_new.merge(df_cur, on='sku', how='left', suffixes=('', '_old'))
changed = merged[(merged['precio'] != merged['precio_old']) | (merged['stock'] != merged['stock_old'])]

output_tables = {
    'crm.productos': {
        'mode': 'upsert',
        'df': changed[['sku', 'precio', 'stock']],
        'key': 'sku',
    }
}
output = {'filas_upserted': len(changed), 'muestra': changed.head(3).to_dict('records')}
"
}
```

## Why this shape

- **Bind the target table too**, not just the incoming file. Diffing in
  pandas means you only write rows that actually changed, and you can
  report an honest count instead of "upserted everything".
- **`upsert` requires a `key`** that has a UNIQUE or PRIMARY KEY constraint
  on the target table. If the table doesn't have one, either add it (ask the
  operator) or use `mode: "update"` instead, which does not require a
  unique constraint but also does not insert new rows.
- **Keep `output` small.** Report a count and maybe a 2-3 row sample — never
  dump the full changed set into `output`; that defeats the whole point of
  keeping rows out of your context.
- If the source is a Google Sheet instead of an attachment, swap the
  binding: `{"var": "nuevo", "spreadsheet_id": "...", "sheet": "Q4"}`. The
  rest of the recipe is identical.

## Variant: brand-new table (no existing rows to diff against)

If the target table doesn't exist yet, skip the second binding and just
write with `append` or `upsert` — nothing to set from your code. Whether a
missing table gets auto-created (vs. failing with `TableNotFound`) is
governed by `on_missing_table` in the **operator's** `fixed_config.sql`
block, not by anything you pass. In most deployments the operator default
is `on_missing_table: "create"`, so the table is auto-created with inferred
column types and, for `upsert`, a `UNIQUE` constraint on `key` — but you
can't assume that; if the write comes back `TableNotFound`, that means this
deployment's operator set `on_missing_table: "fail"`, and the advice in the
error tells you to ask the operator to create the table or change the
policy.

```python
output_tables = {'staging.import_raw': df_crudo}  # shorthand = mode append
```
