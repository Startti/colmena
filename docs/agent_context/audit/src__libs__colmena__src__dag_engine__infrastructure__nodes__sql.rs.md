# src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs

**Layer:** infrastructure  
**Purpose:** SQL query node implementation for DAG execution. Provides PostgreSQL query execution with permission control, static/LLM validation, schema introspection, auto-RLS setup, and lazy connection pooling via OnceCell.

## Symbols

- `SqlNodeInit` (struct, private) — holds initialized adapter and description supplement, created once per node via OnceCell
- `SqlNode` (struct, pub) — main SQL node wrapping SqlPortFactory and lazy OnceCell initialization
- `MAX_SCHEMA_TABLES` (const, private) — threshold (40) for truncating schema description when too many tables
- `MAX_SCHEMA_CHARS` (const, private) — threshold (8000) for truncating schema description by character count
- `SqlNode::new()` (fn, pub) — constructor; creates node with factory and empty OnceCell
- `SqlNode::get_or_init()` (fn, async, private) — lazy initialization; calls `get_or_try_init` on OnceCell, ensures single initialization across concurrent callers
- `SqlNode::do_initialize_inner()` (fn, async, private) — performs full setup: resolves connection URL, loads runtime limits/permissions, provisioning schemas, runs setup_sql, loads table/function metadata, builds description supplement, auto-RLS if enabled  [FLAG: improvement — non-idiomatic function name; "inner" suffix is unclear; consider `initialize_impl` or `perform_initialization`]
- `SqlNode::resolve_env_vars()` (fn, private) — replaces `${ENV_VAR}` placeholders in connection strings with environment values
- `SqlNode::build_description_supplement()` (fn, private) — assembles tool description from table/function metadata, permissions, and LLM anti-patterns guidance; includes schema render (with graceful cap), capability statement, multi-statement query rules, blocked operations list
- `SqlNode::render_schema()` (fn, private) — formats table schemas with columns, primary keys, foreign keys for LLM consumption; qualified names for SQL accuracy
- `SqlNode::new_session_id()` (fn, private) — generates deterministic session ID from current timestamp
- `InitializableNode impl for SqlNode` (trait impl) — provides `initialize()` method, returns `InitContext` with description supplement
- `ExecutableNode impl for SqlNode` (trait impl) — provides `execute()` method (queries with validation/critic/RLS), `schema()`, `description()`, `default_input()`, `default_output()`, `as_initializable()`
- `SqlNode::execute()` (fn, async) — main execution path; resolves effective_config from inputs, performs lazy init, runs SqlExecutionService query, posts RLS to new tables, notifies observer of warnings/hints, wraps errors as JSON
- `SqlNode::schema()` (fn) — returns JSON schema (config fields, inputs, outputs) for node metadata
- `SqlNode::description()` (fn) — returns static description for node discovery
- `SqlNode::default_input()` (fn) — returns "query" as default input field
- `SqlNode::default_output()` (fn) — returns "output" as default output field
- `SqlNode::as_initializable()` (fn) — exposes `self` as `InitializableNode` trait object for tool initialization
- `supplement_tests::t()` (fn, private) — test fixture returning minimal TableSchema for finanzas.gastos with PK and FK
- `supplement_tests::supplement_includes_columns_pk_and_fk()` (test) — verifies description supplement contains PK, columns, and FK references
- `setup_sql_tests::unique()` (fn, private) — generates unique schema name using timestamp nanoseconds
- `setup_sql_tests::fresh_node()` (fn) — creates new SqlNode with fresh registry and factory for testing
- `setup_sql_tests::raw_pool()` (fn, async) — opens raw PostgreSQL connection for verification queries in tests
- `setup_sql_tests::setup_sql_runs_at_init_and_table_is_introspected()` (test) — verifies setup_sql executes at init, table is introspected into supplement, and seed is idempotent across nodes
- `setup_sql_tests::bad_setup_sql_hard_fails_init_and_rolls_back()` (test) — confirms invalid setup_sql fails init and rolls back schema creation
- `setup_sql_tests::empty_or_absent_setup_sql_is_a_noop()` (test) — verifies whitespace-only and missing setup_sql are skipped without error

## File-level notes

- **Logging anti-pattern**: Multiple `println!` statements (lines 120, 137, 184, 191, 433, 446, 532, 546, 559, 570, 596) used for production-facing events. Infrastructure layer should use a structured logger (e.g., `tracing`, `log`) for deployment control over log levels and output format.  [improvement]
- **Minor type redundancy**: Line 337 parameter annotation uses full path `crate::dag_engine::domain::sql_ports::TableSchema` when `TableSchema` is already imported on line 14. Should use just `&[TableSchema]`.  [improvement]
- **Architecture**: Correct hexagonal pattern — domain traits (SqlPermissions, SqlConnectionPort, FunctionRegistryPort) injected via factory; infrastructure adapters (PgPoolAdapter, PgRegistryAdapter, LlmCriticAdapter) instantiated here. OnceCell pattern correctly eliminates TOCTOU race on expensive DB setup.
- **Error handling boundary**: SqlNodeError wrapped as JSON response (line 597–604) for tool use; errors are not propagated as Rust errors from `execute()`. This is intentional (tool-call semantics) but unusual for a node trait.
- **Test coverage**: Well-tested (`supplement_tests`, `setup_sql_tests`). All three setup_sql scenarios verified: normal execution, rollback on failure, no-op on empty/absent. Tests use `#[ignore]` gate with explicit env-var requirement.
