---
name: bulk_insert
description: Inline VALUES patterns, when to split into multiple INSERTs, when to ask the operator for sql_bulk_insert_from_attachment.
---

# Bulk insert

Strategy depends on how many rows you're inserting and where the data comes from.

## ✅ Patterns

### 1–20 rows — inline multi-row VALUES (preferred)
```sql
INSERT INTO products (sku, name, price) VALUES
  ('A1', 'Widget',  10.50),
  ('A2', 'Gadget',  25.00),
  ('A3', 'Gizmo',    5.75);
```
Single statement, single network roundtrip, single transaction.

### 20–500 rows — split into multiple multi-row INSERTs in one call
```sql
INSERT INTO products (sku, name, price) VALUES
  ('B1', 'a', 1), ('B2', 'b', 2), /* ... up to ~50 per VALUES ... */;
INSERT INTO products (sku, name, price) VALUES
  ('B51', 'aa', 51), /* ... etc ... */;
```
Whole thing still runs in one transaction. Smaller VALUES blocks keep parsing fast.

### >500 rows OR data from CSV/Excel — use the bulk tools

If `sql_inspect_attachment` + `sql_bulk_insert_from_attachment` are enabled, prefer them over INSERT. They stream the file directly to the DB without loading rows through your context. The workflow is **always two calls**:

1. **`sql_inspect_attachment`** — opens the attachment, returns header, inferred column types, sample rows (default 5), total row count. Pass the optional `target_table` arg to also receive the destination table schema in the same call (saves a round-trip vs querying `information_schema` separately).

   ```jsonc
   // LLM call
   {"name": "sql_inspect_attachment",
    "arguments": {
      "attachment_id": "doc_csv_abc123",
      "sample_rows": 5,
      "target_table": "public.products"
    }}

   // LLM receives ~300 tokens regardless of file size
   {
     "columns": ["product_id", "sku", "price"],
     "inferred_types": ["integer", "text", "numeric"],
     "sample": [{"product_id": "1", "sku": "A001", "price": "9.99"}, ...],
     "total_rows": 1487,
     "format": "csv",
     "delimiter": ",",
     "target_table_schema": {
       "table": "public.products",
       "columns": [
         {"name": "id", "data_type": "integer", "is_nullable": false},
         {"name": "sku", "data_type": "text", "is_nullable": false},
         {"name": "price", "data_type": "numeric", "is_nullable": false}
       ]
     }
   }
   ```

2. **`sql_bulk_insert_from_attachment`** — runs `COPY FROM STDIN` server-side. `column_mapping` is **required** and must cover every column in the CSV header (identity mappings included). Default `on_error: "fail_fast"` rolls back the whole batch on any row error (no partial state).

   ```jsonc
   // LLM call
   {"name": "sql_bulk_insert_from_attachment",
    "arguments": {
      "attachment_id": "doc_csv_abc123",
      "table": "public.products",
      "column_mapping": {
        "product_id": "id",
        "sku": "sku",
        "price": "price"
      }
    }}

   // LLM receives ~80 tokens
   {
     "rows_inserted": 1487,
     "rows_skipped": 0,
     "duration_ms": 230,
     "method": "copy_from_stdin",
     "errors": []
   }
   ```

**Anti-patterns to avoid with the bulk tools:**

- ❌ Calling `sql_bulk_insert_from_attachment` without `sql_inspect_attachment` first. You need to see headers + sample to know what `column_mapping` to use.
- ❌ Skipping `column_mapping` for "identity" cases. v1 requires it explicitly — `{"a":"a","b":"b"}` is fine.
- ❌ Trying `on_error: "skip_rows"` or `"partial_commit"`. v1 supports `fail_fast` only; the others return a clear error and are tracked for v1.1.
- ❌ Passing an `.xlsx` attachment. v1 supports CSV only. If the user uploaded XLSX, ask them to upload as CSV, or use `python_script` to convert.

If the tools aren't enabled and the data is in your prompt, warn the user about token cost before proceeding.

## ❌ Anti-patterns

### One INSERT per row
```sql
INSERT INTO products (sku) VALUES ('A1');  -- ❌ wasteful
INSERT INTO products (sku) VALUES ('A2');
INSERT INTO products (sku) VALUES ('A3');
```
N statements = N round-trips of parsing/planning. Use multi-row VALUES instead.

### Bind params expecting DB to interpolate
```sql
INSERT INTO products VALUES ($1, $2)  -- ❌ no bind support
```
Paste literal values. Escape apostrophes by doubling them (`'O''Brien'`).

## Edge cases

- **Duplicate keys**: by default, the whole TX rolls back on conflict. Use `ON CONFLICT DO NOTHING` or `ON CONFLICT (col) DO UPDATE SET ...` for upsert semantics.
- **NULL columns**: use the literal `NULL` (not the string `'NULL'`).
- **Date/timestamp literals**: ISO 8601 in single quotes — `'2026-06-09T15:00:00Z'`.
- **JSON columns**: pass a single-quoted JSON literal cast — `'{"key": "v"}'::jsonb`.
