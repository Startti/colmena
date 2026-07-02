---
name: data-run-python-recipes
description: Recipes for moving/analyzing tabular data between files, Google Sheets, and SQL with data_run_python — bindings + output_tables/output_sheets/output_attachments sinks.
references:
  - name: spreadsheet_to_db
    description: "Uploaded file (or Google Sheet) in, DB out. Read an attachment/gsheet, diff or transform it in pandas, upsert the result into a SQL table. The canonical \"update the DB from this Excel\" flow."
  - name: db_to_file
    description: SQL SELECT in, downloadable file out. Query a table and export it as xlsx/csv via output_attachments — no filesystem writes, no manual byte-building.
  - name: cross_source_join
    description: Bind two sources at once (e.g. an incoming table vs an existing DB table, or a gsheet vs a DB table), merge/diff in pandas, write only what changed to one or more sinks.
  - name: sinks_and_modes
    description: "The output_tables modes (append/update/upsert/replace) in detail, plus the anti-patterns that break the tool: singular output_table, tool-arg collision policies, and manual file bytes via io/base64."
---

# data_run_python — recipes and sink contract

`data_run_python` moves and analyzes tabular data between files (attachments),
Google Sheets, and the SQL database, without ever putting raw rows in your
context. Load the reference for the recipe that matches what you're doing —
if the request is generic ("sync this Excel with the DB"), start with
`spreadsheet_to_db`.

## The contract (read this before writing code)

### 1. Bindings — where the data lives NOW

Each entry in `bindings` names a Python global (`var`) and exactly one
source discriminator:

| Source | Fields | Produces |
|---|---|---|
| Attachment (uploaded CSV/XLSX) | `attachment_id` (+ `sheet_name?`, `delimiter?`, `header_row?`) | list of records |
| Google Sheet | `spreadsheet_id` + `sheet` (+ `range?`) | list of records |
| SQL (SELECT-only) | `query` | list of records |
| Inline | `data` (array of records) | list of records |

Every binding lands in the sandbox as a plain list of dict records — wrap it
with `pd.DataFrame(<var>)` to get a DataFrame. Rows never enter your context;
only what `output` explicitly echoes does.

### 2. Sinks — where results go. THESE ARE PLURAL DICTS.

Assign these Python globals in your code to write results back:

```python
output_tables = {"schema.table": {...}}       # → SQL write-back
output_sheets = {"Tab name": {...}}           # → Google Sheets write-back
output_attachments = {"name.xlsx": df}         # → downloadable file in the catalog
output = {...}                                 # → what you report to the user (small!)
```

**Common mistakes that break the tool — do not make these:**

- ❌ `output_table = {...}` (singular) — the global MUST be plural
  (`output_tables`). A singular assignment is silently ignored; nothing gets
  written and you'll wonder why.
- ❌ Passing `write_to_spreadsheet`, `on_existing_sheet`, or
  `on_existing_table` as tool call **arguments** you invent. `write_to_spreadsheet`
  is a real tool arg but is only needed when you assign `output_sheets` — set
  it once at the top level of your call, not inside the code. Collision
  policies (`on_existing_sheet` / `on_existing_table`) are **operator
  config**, not something you pass — you can't override them from code.
- ❌ Hand-building a file with `io.BytesIO`, `base64`, or similar — those
  modules are sandbox-blocked (no filesystem, no raw byte plumbing). To
  export a file, just assign a DataFrame (or `{"df": ..., "delimiter": ...}`)
  to `output_attachments`; the Rust dispatcher serializes it.

### 3. output_tables modes at a glance

| Mode | Needs `key`? | Effect |
|---|---|---|
| `append` (default for a bare df) | No | INSERT — never touches existing rows |
| `update` | Yes | UPDATE matching rows only (diff-driven when the table was also a binding) |
| `upsert` | Yes (+ UNIQUE/PK on it) | INSERT ... ON CONFLICT DO UPDATE |
| `replace` | No | DELETE + INSERT (needs operator opt-in if table exists) |

See `sinks_and_modes` for the full spec-dict shape and shorthand rules.

## The 4 canonical recipes

1. **`spreadsheet_to_db`** — "Here's an updated price list, impact it in the
   database." Attachment/gsheet + existing table as bindings, diff in
   pandas, `output_tables` upsert.
2. **`db_to_file`** — "Download this table as an Excel file." SQL SELECT
   binding, `output_attachments` xlsx/csv.
3. **`cross_source_join`** — Two live sources at once (incoming data vs a DB
   table, or a gsheet vs a DB table), merged/diffed in pandas, writing only
   the delta.
4. **`sinks_and_modes`** — Deep dive on `output_tables` modes and the sink
   anti-patterns above; read this whenever a write silently does nothing or
   errors with `ColumnMismatch`/`UpsertKeyNotUnique`/`KeyColumnMissing`.

If your task doesn't map cleanly to one of these, combine them: bindings and
sinks compose freely in a single call.
