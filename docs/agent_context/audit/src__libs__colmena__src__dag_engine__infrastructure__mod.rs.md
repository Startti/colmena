# src/libs/colmena/src/dag_engine/infrastructure/mod.rs

**Layer:** infrastructure  **Purpose:** Re-exports the infrastructure submodules of the DAG engine, organizing database adapters, node implementations, SQL validation, and tool execution orchestration.

## Symbols

- `dag_tool_executor` (mod, pub) — Tool execution orchestration for DAG nodes
- `node_schema_merge` (mod, pub) — Merges node schema definitions with runtime tool definitions
- `nodes` (mod, pub) — Implementations of all executable DAG node types (40+ node implementations)
- `persistence` (mod, pub) — Database persistence layer for DAG state and memory
- `pool_registry` (mod, pub) — Connection pool registry for database and external resource management
- `registry` (mod, pub) — Node type registry and executor factory; central dispatch for node instantiation
- `sql_ast` (mod, pub) — SQL abstract syntax tree parser for query validation and analysis
- `sql_function_registry` (mod, pub) — User-defined SQL function definitions and management
- `sql_llm_critic` (mod, pub) — LLM-based SQL query critique and validation feedback
- `sql_pool_adapter` (mod, pub) — Database connection pool adapter implementations
- `sql_port_factory` (mod, pub) — Factory for SQL connection port adapters
- `sql_static_validator` (mod, pub) — Static validation of SQL queries for security and correctness

## File-level notes

- Pure module re-export file; no implementation logic
- Comprehensive organization of the infrastructure layer with clear separation of concerns (persistence, SQL validation, node registry, tool execution)
- No visibility gaps or re-export omissions evident
