# src/libs/colmena/src/dag_engine/infrastructure/sql_function_registry.rs

**Layer:** infrastructure  **Purpose:** PostgreSQL adapter implementing `FunctionRegistryPort` to manage sandbox schema tables (`function_registry`, `query_feedback`) for AI-generated SQL functions and validator feedback.

## Symbols

- `PgRegistryAdapter` (pub struct) — PostgreSQL adapter holding pool and sandbox schema name for function/feedback persistence
- `PgRegistryAdapter::new` (pub fn) — Constructor taking pool and sandbox_schema; returns Self
- `FunctionRegistryPort` impl (async_trait impl) — Trait implementation for PostgreSQL-backed registry
- `ensure_schema` (async fn) — Creates schema and two tables (`function_registry`, `query_feedback`) if missing; comments both tables with metadata
- `register_function` (async fn) — Inserts or updates function metadata via upsert (ON CONFLICT); binds function_name, schema_name, parameters, return_type, description, session_id
- `list_functions` (async fn) — Fetches all functions from function_registry, ordered by name; returns Vec<FunctionInfo>  [FLAG: improvement — silent data loss via unwrap_or_default() on required fields]
- `record_feedback` (async fn) — Inserts query feedback row with session_id, query_text, feedback_type, source, message

## File-level notes

- **SQL injection via string formatting (lines 34, 39, 62, 72, 90, 112, 140, 173)**: Schema name is interpolated via `format!()` instead of parameterized binding. Low-risk since schema is config-time (not user-input), but violates best practices. Prefer `sqlx::raw_sql()` with quoted identifier helpers if schema must be configurable.
- **Silent error handling in `list_functions` (lines 152–157)**: Required fields (`function_name`, `schema_name`, `description`) call `unwrap_or_default()`, converting NULL, parse errors, or missing columns to empty strings without logging. Should propagate errors or use `map_err()` to preserve diagnostic info.
- **No rows-affected validation in `register_function` (line 127)**: Query executes and errors only if database error occurs; doesn't verify the upsert affected rows. Acceptable for this use case (database error is the failure signal).
- **Query feedback table design**: No uniqueness constraint or de-duplication logic; same query+feedback_type pairs can accumulate in unbounded inserts. No index on session_id for query performance.
- **Unused imports**: None detected; async_trait, sqlx, thiserror types all used.
