---
name: sinks_and_modes
description: "The output_tables modes (append/update/upsert/replace) in detail, plus the anti-patterns that break the tool: singular output_table, tool-arg collision policies, and manual file bytes via io/base64."
---

# output_tables modes, and the mistakes that silently break writes

## The spec-dict shape

```python
output_tables = {
    "analytics.ventas_por_region": {          # "schema.table" or bare "table"
        "mode": "upsert",                     # append | update | upsert | replace
        "df": resumen,                        # DataFrame or list of records
        "key": "region",                      # required for update/upsert (str or list for composite key)
        "columns": ["total", "updated_at"],   # optional (update/upsert): only touch these columns
    },
    "staging.import_raw": df_crudo,           # shorthand: bare DataFrame = mode append
}
```

A bare DataFrame value is shorthand for `{"mode": "append", "df": ...}`.
There is no bare shorthand for update/upsert/replace — those always need the
full dict because they need at least `mode` (and usually `key`).

## The four modes

| Mode | SQL semantics | Needs `key`? | If table doesn't exist |
|---|---|---|---|
| `append` (default) | Batched INSERT | No | Auto-created if operator policy allows |
| `update` | `UPDATE ... WHERE key = $k`, diff-driven if the table was also bound as a source in the same call | **Yes** | Errors `TableNotFound` — update never creates a table |
| `upsert` | `INSERT ... ON CONFLICT (key) DO UPDATE` | **Yes**, and it needs a UNIQUE/PK on it in Postgres | Auto-created with `UNIQUE(key)` if operator policy allows |
| `replace` | `DELETE FROM table; INSERT ...` in the same transaction | No | Auto-created if allowed; if it exists, blocked unless the operator opted in |

Pick the narrowest mode that does what you need:
- Only adding new rows, never touching old ones → `append`.
- Only touching rows that already exist → `update`.
- Insert new rows AND update existing ones by key → `upsert`.
- Fully replace the table's contents → `replace` (rarely what you want —
  it's the only mode that deletes data).

## Multiple tables in one call are atomic together

If you assign more than one table in `output_tables`, they all write inside
ONE transaction — if any table's write fails, everything rolls back. You
don't need to (and can't) control that; it's automatic.

## Anti-patterns that silently break the tool

### ❌ `output_table` (singular)

```python
output_table = {"crm.productos": df}   # WRONG — ignored, nothing is written
```

The global MUST be `output_tables` (plural). A singular assignment is not an
error — it's just a normal Python variable the tool doesn't look at, so
nothing gets written and no error is raised. If a write silently didn't
happen, check this first.

### ❌ Passing collision policy or spreadsheet target as an invented arg

```jsonc
// WRONG — these are not valid top-level args to invent:
{"on_existing_table": "overwrite", "on_existing_sheet": "auto_suffix", ...}
```

`on_existing_table` and `on_existing_sheet` are **operator-configured**
(`fixed_config`) — you cannot see or override them from a tool call. The
only sink-related arg you ever pass is `write_to_spreadsheet` (a spreadsheet
ID), and only when your code assigns `output_sheets`.

### ❌ Hand-building file bytes

```python
import io, base64   # WRONG — both modules are sandbox-blocked
buf = io.BytesIO()
df.to_excel(buf)
...
```

The sandbox has no filesystem and blocks byte/IO plumbing modules on
purpose. To produce a file, assign a DataFrame straight to
`output_attachments` — the Rust dispatcher does the serialization outside
the sandbox.

## Common errors and what they mean

| Error | Cause | Fix |
|---|---|---|
| `ColumnMismatch` | Your `df` has columns the target table doesn't (or vice versa, for non-auto-create paths) | Drop the extra columns, or target a new table name |
| `KeyColumnMissing` | `key` isn't present in your `df` or in the target table | Check the column name spelling; confirm the table actually has that column |
| `UpsertKeyNotUnique` | `key` has no UNIQUE/PK constraint on the existing table | Ask the operator to add one, or switch to `mode: "update"` |
| `DuplicateKeyInInput` | Your `df` has two rows with the same `key` value | Deduplicate before assigning to `output_tables` |
| `TableNotFound` (on `update`) | `update` never creates tables | Use `append`/`upsert` first, or confirm the table name |
| `EmptyDataFrame` | `update`/`upsert` with zero rows | Guard with `if len(df): output_tables = {...}` |
