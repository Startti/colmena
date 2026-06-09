---
name: error_recovery
description: Common error messages and what they mean. "cannot insert multiple commands" = multi-statement issue. "syntax error at or near" = quote/escape issue. Etc.
---

# Error recovery

When the tool returns `{error: ..., source: ...}`, the `source` tells you which layer rejected the query:
- `static_validator` — blocked by safety rules (don't retry, change the query)
- `llm_critic` — blocked by AI security review (rare)
- `execution` — the DB errored (syntax, constraint, etc.)

## Execution errors

### "cannot insert multiple commands into a prepared statement"
Older colmena version (pre-2026-06-09). Either send one statement per call, or ask the operator to upgrade.

### "syntax error at or near 'foo'"
- Apostrophe inside a string literal not escaped: `'O'Brien'` → `'O''Brien'`.
- Used a bind placeholder (`$1`, `?`, `:name`) — paste the value.
- Tried `BEGIN`/`COMMIT`/`ROLLBACK` — remove.
- Missing/extra parenthesis.

### "relation 'schema.table' does not exist"
- Schema or table name typo. Query `information_schema.tables`.
- Wrong schema — check tool description for `Available tables (schema: ...)`.
- Table is unqualified — Postgres only looked in `public`. Qualify it: `production.users`.

### "column 'foo' does not exist"
- Typo. Query `information_schema.columns` for the table.
- Column is CamelCase but you wrote it lowercase — Postgres folds unquoted identifiers to lowercase. Use double quotes: `"camelCaseColumn"`.

### "null value in column 'foo' violates not-null constraint"
- Column is required. Query columns for `is_nullable = NO` with no `column_default`.
- Include those columns in your INSERT.

### "duplicate key value violates unique constraint 'xxx'"
- Row conflicts with an existing one.
- Use `ON CONFLICT DO NOTHING` to skip, or `ON CONFLICT (col) DO UPDATE SET ...` to upsert.

### "insert or update on table 'x' violates foreign key constraint 'fk_y'"
- Referencing a row in another table that doesn't exist.
- Insert/find the parent row first, then use its id.

### "permission denied for schema 'x'" or "Access to schema 'x' is not allowed"
- Schema not in `allowed_schemas`. Ask the user.

### "statement timeout"
- Query took longer than the limit (default 30s). Add a selective `WHERE`, or break it into smaller queries.

## Validator-block errors

### "DELETE/UPDATE without a WHERE clause is not allowed"
Add real predicates. `WHERE 1=1` doesn't work.

### "TRUNCATE/DROP/ALTER is not allowed"
Hard block. See `anti_patterns` for alternatives.

### "CREATE FUNCTION requires a COMMENT ON FUNCTION"
Add `COMMENT ON FUNCTION <schema>.<name>(<arg_types>) IS '...'` in the same call.

### "Failed to parse SQL: <reason>"
AST parser rejected the query. Read the parser message — usually points at the offending token. Often a missing closing paren or a stray semicolon inside a string literal.

## When to retry vs give up

- `source: "static_validator"` → don't retry, change the query.
- `source: "llm_critic"` → don't retry, the security check said no.
- `source: "execution"` + transient (timeout, "could not serialize access") → retry once.
- `source: "execution"` + syntax/constraint error → fix the query, then retry.
