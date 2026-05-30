---
name: sql_query-guide
description: Best practices for using a sql_query tool against PostgreSQL.
node_type: sql_query
---

# sql_query — best practices

## Mindset

You are talking to a real PostgreSQL database. The access policy section
above this guide tells you exactly what operations and schemas you may
use; treat it as ground truth. The tool's static validator will reject
anything outside it — there is no point in trying.

## Before writing the query

- If you do not know the table layout, run an introspection query first:
  `SELECT table_name FROM information_schema.tables WHERE table_schema = '<schema>'`
  and then `SELECT column_name, data_type FROM information_schema.columns WHERE table_name = '<table>'`.
- Prefer explicit columns over `SELECT *` — narrower results, cleaner
  output, lower token cost.
- When the user asks an aggregate question ("how many", "total of"), use
  `COUNT(*)`, `SUM(...)`, `AVG(...)` directly instead of selecting rows
  and summing yourself.

## Safety rules (will be enforced)

- `DELETE` and `UPDATE` without a `WHERE` clause are rejected. Always
  scope the rows you intend to affect.
- `DROP`, `ALTER`, `TRUNCATE`, `CREATE SCHEMA/INDEX/VIEW`, `GRANT`,
  `REVOKE` are blocked unconditionally. If the user asks for any of
  these, explain that schema and lifecycle changes happen through
  migrations, not the agent.
- `CREATE FUNCTION` requires an accompanying `COMMENT ON FUNCTION ... IS
  '...'` in the same script — otherwise it is rejected.

## Reading large tables

- `SELECT` results are truncated to the max_rows limit shown in the
  policy. If the user wants more, add a more specific `WHERE` clause or
  use aggregations.
- Date filters tend to be the fastest narrowing: prefer `WHERE
  created_at >= '<date>'` over post-fetch filtering.

## Pagination

There is no native paging hook. If you need page N of a result set,
issue another query with `OFFSET (N-1)*size LIMIT size`.

## Errors

When a query fails, the tool returns an error envelope `{ "error":
"...", "source": "static_validator" | "llm_critic" | "execution" }`.
Read the message and adjust. Do not retry the same query — re-read the
policy first.

## Multi-tenant data (when RLS is on)

The tenant filter is enforced server-side. You do not need to add
`WHERE user_id = ...` yourself; the database sets the row-visibility
window. Acting as if rows from other tenants do not exist is the right
mental model.
