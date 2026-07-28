# src/libs/colmena/src/dag_engine/domain/sql_ports.rs

**Layer:** domain  **Purpose:** Defines domain ports (traits) for the SQL node's hexagonal architecture. Each trait defines a capability boundary that infrastructure adapters implement; the application service and node depend only on the traits.

## Symbols

- `ValidationResult` (struct, pub) — Result of static or LLM validation with allowed/block_reason/warnings fields
- `CriticResult` (struct, pub) — Result of LLM critic analysis with security_ok/security_reason/optimization_hints fields
- `FunctionInfo` (struct, pub) — Metadata about a registered function in the sandbox (function_name, schema_name, parameters, return_type, description)
- `TableInfo` (struct, pub) — Metadata about a table with schema_name, table_name, and optional description
- `ColumnInfo` (struct, pub) — A column within a table for LLM schema context (name, data_type, not_null, is_pk, is_unique)
- `ForeignKey` (struct, pub) — A single-column foreign key (column, ref_schema, ref_table, ref_column)
- `TableSchema` (struct, pub) — Full schema of a table including columns and foreign keys for LLM context injection
- `SqlConnectionPort` (trait, pub) — Port for managing PostgreSQL connection pool and executing queries
  - `execute_query` (async fn) — Execute a SQL query and return results as JSON; supports tenant_user_id for RLS
  - `load_table_metadata` (async fn) — Load table metadata (names and comments) for given schemas
  - `load_table_schemas` (async fn) — Load full schema (columns, types, PK/UNIQUE/NOT NULL, FKs) for LLM tool description injection
  - `missing_schemas` (async fn) — Return subset of schemas that do not exist in database (introspection schemas never reported as missing)
  - `create_schema` (async fn) — Create a schema if not already exists (idempotent, operator-driven, quoted identifiers for safety)
  - `execute_setup_sql` (async fn) — Execute operator-authored setup SQL block as single atomic transaction; bypasses LLM validation for DDL+seed
  - `is_connected` (fn) — Check if pool is connected and ready
- `QueryResult` (struct, pub) — Result of SQL query execution with output (Value), row_count, and truncated flag
- `SqlValidatorPort` (trait, pub) — Port for static SQL validation rules
  - `validate` (fn) — Validate a SQL query against static rules and permissions
- `SqlCriticPort` (trait, pub) — Port for LLM-based SQL critic analysis (optional, activated by config flag)
  - `analyze` (async fn) — Analyze a SQL query for security risks and optimization opportunities
- `FunctionRegistryPort` (trait, pub) — Port for managing function registry in sandbox schema
  - `ensure_schema` (async fn) — Ensure sandbox schema and registry tables exist
  - `register_function` (async fn) — Register a newly created function with session_id tracking
  - `list_functions` (async fn) — Load all registered functions
  - `record_feedback` (async fn) — Record a feedback entry (warning or optimization hint) with source tracking

## File-level notes

- Pure domain-layer trait and value-object definitions; no implementations present
- All traits use `async_trait` with `Send + Sync` bounds for thread-safe async execution
- Value objects (`ValidationResult`, `CriticResult`, `FunctionInfo`, `TableInfo`, `ColumnInfo`, `ForeignKey`, `TableSchema`, `QueryResult`) are well-documented with clear field semantics
- Documentation is comprehensive on capability boundaries, trust levels, and operational constraints (e.g., operator-level `create_schema`/`execute_setup_sql` bypass LLM validation)
- No dead code, unfinished implementations, or missing error handling detected
- Follows hexagonal architecture pattern cleanly with zero infrastructure dependencies
