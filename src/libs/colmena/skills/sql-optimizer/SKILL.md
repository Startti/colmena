---
name: sql-optimizer
description: Use when the user asks to write, review, or optimize SQL queries — performance, indexes, joins, aggregations, or query plans. Do NOT use for ORM-specific questions (those belong to the respective framework skill).
references:
  - name: query-plans
    description: How to read and interpret EXPLAIN / EXPLAIN ANALYZE output in PostgreSQL and MySQL
---

# SQL Optimizer

You are an expert at writing and optimizing SQL. Apply these principles:

## Indexes
- Every JOIN condition should be indexed on both sides.
- WHERE clauses on non-indexed columns cause full scans — check `EXPLAIN`.
- Composite indexes follow the leftmost-prefix rule: `(a, b, c)` helps queries filtering by `a`, `(a, b)`, or `(a, b, c)` but NOT standalone `b` or `c`.
- Prefer covering indexes (include columns used in SELECT) for hot queries.

## Joins
- `INNER JOIN` when both sides are required. `LEFT JOIN` only when the right side is optional.
- Beware of `LEFT JOIN` + `WHERE right_table.col = X` — this accidentally behaves like an inner join. Use `AND` in the `ON` clause.
- Small table on the right side of a nested-loop join; larger table on the left.

## Aggregations
- `GROUP BY` only the columns you need. Unnecessary grouping columns cost memory and time.
- Use `FILTER (WHERE ...)` (PostgreSQL) or conditional aggregates (`SUM(CASE WHEN ... THEN 1 ELSE 0 END)`) instead of multiple subqueries.
- `HAVING` filters after aggregation; `WHERE` before. Use `WHERE` whenever possible — it's cheaper.

## Common anti-patterns
- `SELECT *` in production code — specify columns.
- `NOT IN (SELECT ...)` with nullable columns — returns unexpected empty results. Use `NOT EXISTS`.
- `COUNT(*)` on huge tables when you just need "any" — use `EXISTS`.
- `OFFSET N` with large N for pagination — use keyset/seek pagination.

When the user shares a slow query, call load_skill again with `reference: "query-plans"` to get help reading the execution plan.
