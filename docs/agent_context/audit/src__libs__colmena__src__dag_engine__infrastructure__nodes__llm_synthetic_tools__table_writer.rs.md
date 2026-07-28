# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/table_writer.rs

**Layer:** infrastructure  
**Purpose:** Parse, validate, and transactionally execute SQL write-back statements from Python sandbox code into Postgres. Transforms `output_tables` JSON globals into typed write specs supporting INSERT/UPSERT/UPDATE/REPLACE modes with schema allowlist and permission checks.

## Symbols

### Constants
- `MAX_ROWS` (const, private) — maximum rows accepted in a single `output_tables` write (100,000)
- `WRITE_CHUNK_SIZE` (const, private) — upper bound on rows chunked into one INSERT/UPSERT statement (1,000)
- `PG_MAX_BIND_PARAMS` (const, private) — Postgres hard limit on bind parameters per prepared statement (65,535)

### Enums
- `WriteMode` (pub enum, derives Debug/Clone/Copy/PartialEq/Eq) — table write modes: Append, Update, Upsert, Replace; defaults to Append for bare DataFrames
- `InferredType` (enum, private) — SQL column types inferred from JSON values: BigInt, DoublePrecision, Boolean, TimestampTz, Text

### Structs
- `TableWriteSpec` (pub struct) — parsed write spec with table name, mode, record list, optional key columns, optional column filter

### Functions (pub)
- `parse_output_tables(value: &Value) -> Result<Vec<TableWriteSpec>, Value>` — parse `output_tables` JSON object (array → Append, spec dict → mode + df + key + columns); returns structured error envelope on malformed input
- `validate_write_spec(spec, allowed_schemas, perms, table_exists, table_columns) -> Result<(), Value>` — pure validation covering schema allowlist, operation permissions, table existence, key presence/uniqueness, duplicate keys in input, column name validity, column mismatch vs existing table, row count cap; returns first failing check as structured JSON
- `infer_create_ddl(schema, table, records, key) -> String` — build parameterized `CREATE TABLE IF NOT EXISTS` DDL inferring each column's SQL type from record values; appends UNIQUE(key_cols) if key provided
- `build_insert_sql(schema, table, cols, row_chunk) -> (String, Vec<Value>)` — build parameterized INSERT statement with row-major placeholder numbering; binds NULL for missing keys
- `build_upsert_sql(schema, table, cols, key, row_chunk) -> (String, Vec<Value>)` — build `INSERT ... ON CONFLICT (key) DO UPDATE SET` statement with EXCLUDED-sourced updates for non-key columns
- `build_update_sql_from_changes(schema, table, key, changes) -> Vec<(String, Vec<Value>)>` — build UPDATE statements from cell-level diffs grouped by key_value; returns empty vec for composite keys (limitation documented)
- `write_output_tables(pool, specs, allowed_schemas, perms, tenant_user_id, loaded_snapshots, on_missing_table, on_existing_table, statement_timeout_ms, work_mem_mb) -> Value` — transactional executor: validates all specs, applies them sequentially inside single transaction with per-spec auto-create/permission gates; returns `{"wrote_tables": [...]}` or first error with rollback

### Functions (private)
- `parse_spec_dict(table, spec_obj) -> Result<TableWriteSpec, Value>` — parse one spec dict entry extracting mode/df/key/columns with validation; mode defaults to Append
- `normalize_records(value) -> Vec<Map<String, Value>>` — convert 2D arrays (header row + data rows) or object arrays to record list via rectangle_to_records
- `input_columns(records) -> Vec<String>` — union of column names across records in first-seen order (deduped)
- `InferredType::as_sql(self) -> &'static str` — map type enum to SQL type string (BigInt→"BIGINT", etc.)
- `infer_column_type(values: impl Iterator<Item = &Value>) -> InferredType` — infer column type by scanning non-null values: checks for integer/float/bool/ISO8601-timestamp/text; all-null → Text; mixed int+float → DoublePrecision; u64 > i64::MAX → Text (lossless binding)
- `quote_ident(ident) -> String` — quote SQL identifier with double quotes, escaping embedded quotes per SQL standard
- `bind_json_value(query, value) -> Query` — bind JSON Value to sqlx query mapping scalars to native types; Array/Object as JSON text
- `exec_bound(tx, sql, params) -> Result<PgQueryResult, Error>` — execute parameterized statement binding each Value in order
- `is_unique_violation(err) -> bool` — detect Postgres SQLSTATE 42P10 (ON CONFLICT arbiter mismatch)
- `is_generic_constraint_violation(err) -> bool` — detect Postgres SQLSTATE 23505 (unrelated unique constraint collision)
- `introspect_columns(tx, schema, table) -> Result<Vec<String>, Error>` — query information_schema for table columns in ordinal position order; empty vec if table doesn't exist
- `chunk_rows_for(num_cols) -> usize` — compute row-chunk size capping `cols * chunk_rows <= PG_MAX_BIND_PARAMS` while never exceeding WRITE_CHUNK_SIZE; always ≥ 1
- `cast_trailing_where_key_to_text(sql) -> String` — inject `::text` cast into trailing WHERE key column to handle type coercion when key_value is bound as string
- `scalar_key_to_string(v) -> Option<String>` — convert JSON scalar to string (null/array/object → None); matches diff_writer's key stringification
- `build_full_record_updates(schema, table, key, cols, records) -> Vec<(String, Vec<Value>)>` — build full-record UPDATE per record filtered by non-key columns; returns empty if only key columns present (no SET clause)
- `write_one_table(tx, spec, allowed_schemas, perms, loaded_snapshots, on_missing_table, on_existing_table) -> Result<Value, Value>` — apply one spec inside transaction: introspect columns, validate, apply policy gates, auto-create if needed, dispatch write mode (Append→insert_chunked, Upsert→upsert_chunked, Replace→delete+insert_chunked, Update→update_records with diff-driven or full-record fallback)
- `insert_chunked(tx, schema, table, cols, records) -> Result<u64, Value>` — insert records in chunks respecting bind-param budget; returns total rows_affected
- `upsert_chunked(tx, schema, table, cols, key, records, table_label) -> Result<u64, Value>` — upsert records in chunks; maps Postgres errors to UpsertKeyNotUnique (42P10) or ConstraintViolation (23505)
- `update_records(tx, schema, table, key, cols, spec, loaded_snapshots) -> Result<(u64, Option<Value>), Value>` — dispatch Update: diff-driven against snapshot if available (returns rows_affected + changes summary), else full-record UPDATE fallback; composite-key diff limitation documented

### Tests (module #[cfg(test)])
Comprehensive test suite covering:
- Parsing: bare DataFrames, spec dicts, mode/key/columns parsing, 2D array conversion, error cases (missing df, empty df, invalid mode, non-object top level)
- Validation: permission checks, schema allowlist, table existence, key requirements, duplicate keys, column mismatch, row-count cap, key-column presence
- DDL inference: type inference (bigint/double precision/boolean/timestamptz/text), u64 overflow handling, all-null columns, ISO8601 detection, composite keys, quote escaping
- SQL builders: INSERT with placeholders, UPSERT with ON CONFLICT, multi-row numbering, UPDATE from cell changes, composite-key limitation
- Integration (Postgres): append with auto-create, append into existing, upsert with PK/unique constraints, replace, update (full-record fallback and diff-driven with snapshot), composite-key fallback, error handling (constraint violations, rollback on batch failure), tenant context, operator policy gates

## File-level notes

- **Architecture:** Pure parsing + validation layer (lines 57–858) with zero infrastructure dependencies, followed by async transactional executor (lines 884–1317) using sqlx Postgres bindings. Error handling is comprehensive: all validation failures return structured JSON envelopes with error code, table name, and actionable message; all DB operations return JSON results for consistent error reporting.

- **Schema lock-in:** The module carefully avoids hard-coded schema assumptions and threads schema/table names through all builders and executors, supporting operator-defined `allowed_schemas` allowlist and multi-tenant isolation via `app.current_user_id` session config.

- **Type inference heuristic:** The `infer_column_type` function uses conservative logic to avoid data loss (u64 > i64::MAX → TEXT, not overflow; any non-ISO8601 string → TEXT, not failed parse). Fully tested against edge cases.

- **Chunk management:** Respects Postgres's 65,535 bind-parameter limit per prepared statement via `chunk_rows_for`, preventing silent parameter overflow on wide tables.

- **Update dispatch:** Supports diff-driven updates (using pre-loaded snapshots via `diff_records`) for minimal writes; falls back to full-record UPDATE for composite keys (limitation explicit in docstring) or missing snapshots. Zero-change updates are safe no-ops.

- **Operator policy gates:** `on_missing_table` (create/fail) and `on_existing_table` (fail/append/overwrite) policies are validated upfront before any DB access; Replace on existing table with fail-policy is rejected BEFORE delete runs (safety-first semantics).

- **Test coverage:** Over 100 test cases including 30+ unit tests (parsing, validation, builders) and 20+ integration tests with real Postgres (all #[ignore] with TEST_DATABASE_URL requirement). Tests verify rollback on batch failure, constraint-violation classification, runtime limits (statement_timeout, work_mem), and edge cases (duplicate keys, zero-change updates, composite keys, tenant isolation).

- **No unsafe/unfinished code:** All functions have complete bodies with proper error propagation; no todo!(), unimplemented!(), unreachable!() markers; one defensive unreachable!() at line 462 correctly marked as safe (null filtered before match).
