---
name: db_to_file
description: SQL SELECT in, downloadable file out. Query a table and export it as xlsx/csv via output_attachments — no filesystem writes, no manual byte-building.
---

# Recipe: DB → downloadable file

Use this when the user wants a table (or a query result) as a file they can
download — "give me this table as an Excel file", "export last month's
orders as CSV".

## Shape

```jsonc
{
  "bindings": [
    {"var": "ventas", "query": "SELECT * FROM analytics.ventas_2026"}
  ],
  "code": "
import pandas as pd
df = pd.DataFrame(ventas)
output_attachments = {'ventas_2026.xlsx': df}
output = {'filas': len(df)}
"
}
```

## Why this shape

- **Do the filtering/aggregation in the SQL binding, not in pandas**, when
  possible — a `WHERE`/`GROUP BY` in the `query` binding keeps the row cap
  (100 000 rows per binding) far away and avoids fetching data you don't
  need.
- **`output_attachments` values can be a bare DataFrame** (format inferred
  from the filename extension: `.xlsx` or `.csv`), or a spec dict for
  options: `{"df": df_err, "delimiter": ";"}`.
- **Never build the file yourself.** Don't import `io`, `base64`,
  `openpyxl`, or try to write to a path — the sandbox has no filesystem and
  those modules are blocked. Assigning a DataFrame to `output_attachments`
  is the entire mechanism; the Rust dispatcher serializes and registers it
  in the attachment catalog, returning a `document_id`.
- **`output` should report the shape, not the content.** `{'filas': len(df)}`
  is enough — the file itself is what the user downloads.

## Multiple files in one call

```python
output_attachments = {
    'reporte_mensual.xlsx': df_ok,
    'errores.csv': {'df': df_err, 'delimiter': ';'},
}
```

Each entry gets its own `document_id` in the response — report all of them
if you mention filenames back to the user, so they can tell which is which.
