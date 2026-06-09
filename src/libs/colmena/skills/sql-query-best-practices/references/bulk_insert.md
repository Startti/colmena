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

### >500 rows OR data from CSV/Excel — ask for the bulk tool
If `sql_bulk_insert_from_attachment` is enabled, prefer it. It streams the file directly to the DB without loading rows through your context. If it's not enabled and the data is in your prompt, warn the user about token cost before proceeding.

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
