# Reading Query Plans

## PostgreSQL: EXPLAIN ANALYZE

Run the query with `EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT) <query>` to get actual timings plus buffer hits.

Key things to look for:

- **`Seq Scan`** on a large table — usually bad. Suggests a missing index or a predicate that can't use one.
- **`Nested Loop`** with a large outer side — performs the inner side once per outer row. Fine for small outer sides; catastrophic for millions.
- **`Hash Join`** — builds a hash of one side; fine for equality joins on large sets.
- **`Merge Join`** — requires both inputs sorted; often comes with a `Sort` node.
- **Rows estimated vs. actual**: if estimates are off by 10x or more, run `ANALYZE <table>` to refresh statistics.
- **`Buffers: shared hit=X read=Y`** — `read` means disk I/O. High read counts on repeat queries suggest cache-miss or oversized working set.

## MySQL: EXPLAIN

`EXPLAIN` in MySQL is less detailed than PostgreSQL's. Columns to watch:

- `type`: `ALL` is a full table scan (bad). Aim for `ref`, `range`, `eq_ref`, or `const`.
- `key`: which index is used (or NULL if none). `possible_keys` lists candidates the planner considered.
- `rows`: estimated rows examined. Multiply across joined tables to estimate total work.
- `Extra`: `Using filesort` and `Using temporary` are red flags for large result sets.

Use `EXPLAIN ANALYZE` (MySQL 8.0.18+) for actual timings.

## Debugging approach

1. Run `EXPLAIN ANALYZE` and capture the output.
2. Find the node with the highest `actual time` or `rows`.
3. If it's a scan: is the predicate indexable? Is the index present?
4. If it's a join: is the join condition indexed? Is the planner picking the right join type?
5. If nothing obvious: check `pg_stat_statements` / `performance_schema` for whether the query is even the bottleneck, or if statistics are stale (run `ANALYZE`).
