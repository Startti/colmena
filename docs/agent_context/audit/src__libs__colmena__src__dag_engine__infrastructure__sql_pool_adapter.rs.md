# src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs

**Layer:** infrastructure  
**Purpose:** PostgreSQL connection pool adapter implementing tenant-isolation RLS (Row-Level Security), per-query runtime limits (statement_timeout, work_mem), and multi-statement SQL execution with atomic transaction semantics (Policy C, shipped 2026-06-09).

## Symbols

- `PgPoolAdapter` (struct, pub) — Wraps an Arc<PgPool> with statement_timeout_ms and work_mem_mb guardrails; does not own pool creation.
- `PgPoolAdapter::new()` (fn, pub) — Constructor accepting pool, statement_timeout_ms, work_mem_mb.
- `PgPoolAdapter::pool()` (fn, pub) — Returns Arc<PgPool> clone for reuse by PgRegistryAdapter.
- `PgPoolAdapter::quote_ident()` (fn, private) — SQL identifier quoting (prevents injection via quote+escape).
- `PgPoolAdapter::is_rls_enabled()` (fn, pub) — Queries pg_class.relrowsecurity to check if RLS is enabled on a table.
- `PgPoolAdapter::has_column()` (fn, pub) — Checks information_schema.columns for column existence.
- `PgPoolAdapter::has_policy()` (fn, private) — Queries pg_policies to check if a specific RLS policy exists on a table.
- `PgPoolAdapter::add_tenant_column()` (fn, pub) — ALTER TABLE ADD COLUMN with DEFAULT current_setting('app.current_user_id').
- `PgPoolAdapter::setup_rls_for_table()` (fn, pub) — Enables RLS and creates isolation policy (tenant-aware) or read-only policy (shared table), including DEFAULT tenant column setup.
- `PgPoolAdapter::setup_rls_for_new_table()` (fn, pub) — Wraps setup_rls_for_table(); auto-adds tenant column if missing.
- `impl SqlConnectionPort for PgPoolAdapter::execute_query()` (fn, pub async) — Executes multi-statement SQL within a single atomic transaction; parses with sqlparser, executes per-statement, applies LIMIT only to final SELECT, returns rows or rows_affected sum.
- `impl SqlConnectionPort for PgPoolAdapter::load_table_metadata()` (fn, pub async) — Queries information_schema + pg_catalog to fetch table names and descriptions for requested schemas.
- `impl SqlConnectionPort for PgPoolAdapter::load_table_schemas()` (fn, pub async) — Full schema introspection: columns with NOT NULL/PK/UNIQUE flags, single-column foreign keys, assembled into TableSchema structs.
- `impl SqlConnectionPort for PgPoolAdapter::missing_schemas()` (fn, pub async) — Identifies which schemas in the input list do not exist; excludes information_schema and pg_catalog.
- `impl SqlConnectionPort for PgPoolAdapter::create_schema()` (fn, pub async) — CREATE SCHEMA IF NOT EXISTS (idempotent); fails with clear error if role lacks privilege.
- `impl SqlConnectionPort for PgPoolAdapter::execute_setup_sql()` (fn, pub async) — Executes operator-provided multi-statement SQL via sqlx::raw_sql with escape normalization (literal \n/\t outside strings → space); implicit transaction.
- `impl SqlConnectionPort for PgPoolAdapter::is_connected()` (fn, pub) — Stub returning true.
- `marshall_rows()` (fn, private) — Converts sqlx::postgres::PgRow vector to Vec<serde_json::Value>; handles type coercion (INT8→i64, FLOAT4/8→f64, NUMERIC→BigDecimal→f64, BOOL, default to String), null on conversion failure.
- `normalize_setup_sql()` (fn, private) — Rewrites literal backslash-escape sequences (\n, \t, \r) to space outside single-quoted strings and dollar-quoted blocks (PL/pgSQL bodies); preserves '' and $tag$ escapes.
- Test module with 15 tests: `normalize_setup_sql` (pure, 7 tests), schema lifecycle (missing/create, 2 tests), multi-statement execution Policy C (7 tests).

## File-level notes

- **Logging via println!()**: Lines 211–214 and 235–238 use `println!("[RLS] ...")` for status messages in async functions. In high-concurrency pool scenarios, this is thread-unsafe and lacks log-level filtering. Should use structured logging (tracing/log crate) instead.
- **Silent type-coercion defaults**: marshall_rows() and is_rls_enabled() use `unwrap_or(Value::Null)` and `unwrap_or(false)` to handle type conversion failures. These are intentional (lenient type coercion), but could silently hide schema mismatches or unexpected data types if Postgres returns an unexpected column type.
- **Linear schema search**: load_table_schemas() uses a closure `find()` that linear-searches the Vec<TableSchema> multiple times (once per column, once per FK). Acceptable for non-hot-path schema introspection, but could use HashMap for large schemas.
- **Defensive unreachable!()**: Line 464 correctly uses unreachable!() after proving via logic that all branches in the per-statement loop return when processing the final statement. This is sound defensive programming, not a stub.
- **Test coverage**: Comprehensive (#[ignore] gates on DATABASE_URL dependency). Policy C multi-statement tests validate rollback, row aggregation, LIMIT injection, NUMERIC precision (BigDecimal→f64), and escape normalization. All tests pass.
