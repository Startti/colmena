---
name: schema_discovery
description: Using information_schema queries to discover columns, types, constraints. Useful before writing INSERTs against unfamiliar tables.
---

# Schema discovery

The tool description lists table NAMES but not columns/types. When you need column details, query `information_schema` (always allowed regardless of allowlist).

## ✅ Patterns

### List tables in a schema with their comments
```sql
SELECT t.table_name, pg_catalog.obj_description(c.oid) AS description
  FROM information_schema.tables t
  LEFT JOIN pg_catalog.pg_class c ON c.relname = t.table_name
  WHERE t.table_schema = 'production' AND t.table_type = 'BASE TABLE'
  ORDER BY t.table_name;
```

### Get columns of a table with types and nullability
```sql
SELECT column_name, data_type, is_nullable, column_default
  FROM information_schema.columns
  WHERE table_schema = 'production' AND table_name = 'orders'
  ORDER BY ordinal_position;
```

### Find primary key columns
```sql
SELECT kcu.column_name
  FROM information_schema.table_constraints tc
  JOIN information_schema.key_column_usage kcu
    ON tc.constraint_name = kcu.constraint_name
   AND tc.table_schema    = kcu.table_schema
  WHERE tc.table_schema = 'production'
    AND tc.table_name   = 'orders'
    AND tc.constraint_type = 'PRIMARY KEY'
  ORDER BY kcu.ordinal_position;
```

### Find foreign keys pointing INTO a table
```sql
SELECT
  tc.table_schema AS from_schema, tc.table_name AS from_table,
  kcu.column_name AS from_column,
  ccu.table_name  AS to_table,  ccu.column_name AS to_column
FROM information_schema.table_constraints tc
JOIN information_schema.key_column_usage kcu
  ON tc.constraint_name = kcu.constraint_name
JOIN information_schema.constraint_column_usage ccu
  ON ccu.constraint_name = tc.constraint_name
WHERE tc.constraint_type = 'FOREIGN KEY'
  AND ccu.table_schema = 'production' AND ccu.table_name = 'users';
```

### Find UNIQUE constraints (for upsert planning)
```sql
SELECT kcu.column_name
  FROM information_schema.table_constraints tc
  JOIN information_schema.key_column_usage kcu
    ON tc.constraint_name = kcu.constraint_name
  WHERE tc.table_schema = 'production' AND tc.table_name = 'users'
    AND tc.constraint_type IN ('UNIQUE', 'PRIMARY KEY');
```

## When to use this vs the tool description

| Situation | Use |
|---|---|
| Need to know schema is reachable | Tool description |
| Need column names of a known table | Query `information_schema.columns` |
| Picking which column to JOIN on | Query FKs (pattern above) |
| Planning an upsert (`ON CONFLICT`) | Query UNIQUE constraints |
| Choosing the right datatype to insert | Query `information_schema.columns.data_type` |

## Notes

- `information_schema` is always allowed, regardless of `allowed_schemas`.
- `pg_catalog` is also allowed — use it when `information_schema` doesn't have what you need.
- Don't try to SELECT permission tables — focus on schema, not security.
