---
name: anti_patterns
description: BEGIN/COMMIT, bind params ($1/?/:name), TRUNCATE, DROP, CREATE INDEX — what NOT to write and why. Includes example errors and fixes.
---

# Anti-patterns

Things the LLM commonly tries that DON'T work, plus the error you'll see and how to fix it.

## Transaction control

| You wrote | Error | Fix |
|---|---|---|
| `BEGIN;` | `syntax error at or near "BEGIN"` | Remove it — TX is automatic |
| `COMMIT;` | `syntax error at or near "COMMIT"` | Remove it |
| `ROLLBACK;` | `syntax error at or near "ROLLBACK"` | Remove it. To undo, send a corrective statement |
| `SAVEPOINT ...` | Allowed but useless — TX is already automatic | Just remove it |

## Bind parameters

| You wrote | Error | Fix |
|---|---|---|
| `WHERE id = $1` | `syntax error at or near "$"` | Paste literal: `WHERE id = 42` |
| `WHERE name = ?` | `syntax error at or near "?"` | Paste: `WHERE name = 'Alice'` |
| `WHERE name = :name` | `syntax error at or near ":"` | Same |

Strings with apostrophes → double them: `'It''s OK'`.

## Blocked DDL

| You wrote | Error | Alternative |
|---|---|---|
| `TRUNCATE TABLE t` | `TRUNCATE is not allowed` | `DELETE FROM t WHERE ...` |
| `DROP TABLE t` | `DROP is not allowed` | Ask the operator to run a migration |
| `ALTER TABLE t ADD COLUMN ...` | `ALTER is not allowed` | Ask the operator |
| `CREATE INDEX idx ON t(c)` | `CREATE INDEX is not supported` | Ask the operator |
| `CREATE VIEW v AS SELECT ...` | `CREATE VIEW is not supported` | Save the SELECT as text and run on demand |
| `CREATE SCHEMA s` | `CREATE SCHEMA is not supported` | Operator pre-creates allowed schemas at boot |
| `GRANT/REVOKE ...` | `is not supported` | Operator manages permissions |

## DELETE/UPDATE without WHERE

```sql
DELETE FROM orders                       -- ❌ blocked
UPDATE users SET active = false          -- ❌ blocked
```

Add a real predicate. `WHERE 1=1` won't bypass it. To affect every row in batches:
```sql
DELETE FROM stale_logs WHERE created_at < now() - INTERVAL '90 days';
```

## CREATE FUNCTION without COMMENT

```sql
CREATE FUNCTION sandbox.sum_things(a INT, b INT) RETURNS INT AS $$ SELECT a + b $$ LANGUAGE SQL;
-- ❌ requires a COMMENT ON FUNCTION
```

Fix — pair them:
```sql
CREATE FUNCTION sandbox.sum_things(a INT, b INT) RETURNS INT AS $$ SELECT a + b $$ LANGUAGE SQL;
COMMENT ON FUNCTION sandbox.sum_things(INT, INT) IS 'Adds two integers — used by reports.';
```

## Schema not in allowlist

```sql
SELECT * FROM secret.passwords  -- ❌ Access to schema 'secret' is not allowed
```

The schemas you can use are listed in the tool description at the top of the turn. If you need one that's not there, ask the user.

## Wildcard SELECTs (warning, not block)

```sql
SELECT * FROM orders  -- ⚠️ warning: prefer specific columns
```

Still works. Be specific when you know what you need.
