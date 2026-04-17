# SQL Node Design Spec — `sql_query`

## Overview

A new DAG node type (`sql_query`) that enables LLM agents to interact with PostgreSQL databases as a tool. The node provides granular permission control, a hybrid validation pipeline (static rules + optional LLM critic), a sandbox schema for agent-created functions, and auto-injection of database context into the LLM's tool description.

**Scope:** PostgreSQL only (v1). Multi-driver support (MySQL, SQLite) deferred to future versions.

---

## Architecture

### Hexagonal (Nodo Delgado + Servicios)

`SqlNode` implements `ExecutableNode` and acts as a thin orchestrator, delegating to specialized services through domain traits (ports):

```
SqlNode (infrastructure/nodes/sql.rs)
  |-- SqlConnectionPort (domain/trait) --> PgPoolAdapter
  |-- SqlValidatorPort  (domain/trait) --> StaticRuleValidator
  |-- SqlCriticPort     (domain/trait) --> LlmCriticAdapter
  |-- FunctionRegistryPort (domain/trait) --> PgRegistryAdapter
```

### File Structure

```
src/libs/colmena/src/dag_engine/

domain/
  |-- sql_ports.rs          <-- traits: SqlConnectionPort, SqlValidatorPort,
  |                              SqlCriticPort, FunctionRegistryPort
  |-- sql_permissions.rs    <-- SqlPermissions struct, PermissionPreset enum,
  |                              operation enums
  |-- sql_errors.rs         <-- SqlError enum (Blocked, ValidationFailed,
                                 ConnectionError, CriticRejected...)

application/
  |-- sql_execution_service.rs  <-- orchestrates: validate -> critic -> execute -> feedback

infrastructure/
  |-- nodes/sql.rs              <-- SqlNode impl ExecutableNode + InitializableNode
  |-- sql_pool_adapter.rs       <-- PgPool via sqlx, statement_timeout, work_mem
  |-- sql_static_validator.rs   <-- static rules (pattern matching)
  |-- sql_llm_critic.rs         <-- adapter that uses LlmCallUseCase to evaluate SQL
  |-- sql_function_registry.rs  <-- CRUD over sandbox.function_registry
```

### New Trait: `InitializableNode`

An optional trait the engine calls once before the first execution. `SqlNode` implements it to:

1. Create the connection pool with runtime limits (`statement_timeout`, `work_mem`)
2. Ensure sandbox schema and registry tables exist
3. Load metadata (table names + comments, function inventory) for LLM context injection

```rust
#[async_trait]
pub trait InitializableNode: Send + Sync {
    async fn initialize(&self, config: &Value) -> Result<InitContext, Box<dyn StdError + Send + Sync>>;
}
```

The `InitContext` returned contains the enriched tool description that `DagToolExecutor` injects into the tool definition sent to the LLM.

### Dependency

- `sqlx` with features `postgres` + `runtime-tokio-rustls`

---

## Permission Model

### Presets + Deny

Permissions are configured via presets with an optional `deny` list for fine-tuning:

| Preset | SELECT | INSERT | UPDATE | DELETE | CREATE FUNCTION |
|--------|--------|--------|--------|--------|-----------------|
| `read_only` | yes | no | no | no | no |
| `read_write` | yes | yes | yes | no | no |
| `full` | yes | yes | yes | yes | yes |

**Default:** When `permissions` is omitted, `read_only` is assumed (principle of least privilege).

**Deny override:** Removes permissions from a preset.

```json
{
  "preset": "read_write",
  "deny": ["delete"]
}
```

### Always-On Behaviors

- **Introspection** is always active (not configurable). The agent can always query `information_schema` and `pg_catalog` to discover table structure.
- **TRUNCATE** is always blocked. No preset enables it.
- **DROP / ALTER** on protected schemas is always blocked.

### Schema Access

- `allowed_schemas` (required) defines which schemas the agent can query/write to.
- `sandbox_schema` (optional, default: `"sandbox"`) defines where the agent can create functions and temporary tables. Only relevant when `create_function` is enabled (via `full` preset or equivalent). Must be included in `allowed_schemas`.
- Operations against schemas not in `allowed_schemas` are blocked by the static validator.

---

## Node Configuration (JSON)

### Example 1: Read-Only Agent

```json
"query_database": {
  "name": "query_database",
  "node_type": "sql_query",
  "description": "Query the production database.",
  "node_schema": {
    "connection_url": { "type": "string", "fixed": "${DATABASE_URL}" },
    "permissions": {
      "type": "object",
      "fixed": {
        "preset": "read_only",
        "allowed_schemas": ["production"]
      }
    },
    "runtime_limits": {
      "type": "object",
      "fixed": {
        "max_rows": 100,
        "statement_timeout_ms": 30000,
        "work_mem_mb": 64
      }
    },
    "guardrail_enabled": { "type": "boolean", "fixed": true },
    "guardrail_llm": {
      "type": "object",
      "fixed": {
        "enabled": false
      }
    },
    "query": {
      "type": "string",
      "required": true,
      "description": "SQL SELECT query to execute."
    }
  }
}
```

### Example 2: Read-Write Agent with LLM Critic and Sandbox

```json
"manage_orders": {
  "name": "manage_orders",
  "node_type": "sql_query",
  "description": "Manage orders and create analysis functions.",
  "node_schema": {
    "connection_url": { "type": "string", "fixed": "${DATABASE_URL}" },
    "permissions": {
      "type": "object",
      "fixed": {
        "preset": "full",
        "deny": ["delete"],
        "allowed_schemas": ["production", "sandbox"],
        "sandbox_schema": "sandbox"
      }
    },
    "runtime_limits": {
      "type": "object",
      "fixed": {
        "max_rows": 200,
        "statement_timeout_ms": 60000,
        "work_mem_mb": 128
      }
    },
    "guardrail_enabled": { "type": "boolean", "fixed": true },
    "guardrail_llm": {
      "type": "object",
      "fixed": {
        "enabled": true,
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "${OPENAI_API_KEY}"
      }
    },
    "query": {
      "type": "string",
      "required": true,
      "description": "SQL query to execute. You can SELECT, INSERT, UPDATE on production, and CREATE FUNCTION in sandbox."
    }
  }
}
```

---

## Hybrid Validation Pipeline

Every query passes through a two-stage pipeline before reaching PostgreSQL:

```
SQL from agent
  --> Static Rules (always active, <1ms)
  --> LLM Critic (only if guardrail_llm.enabled = true, ~1-3s)
  --> PostgreSQL (execute)
```

### Stage 1: Static Rules (Always Active)

Pattern-based validation. Instantaneous, zero cost.

**Blocking rules:**
- DELETE / UPDATE without WHERE clause
- DROP / TRUNCATE / ALTER on protected schemas
- Operation not permitted by resolved permissions
- Access to schema not in `allowed_schemas`
- CREATE TABLE / CREATE FUNCTION without `COMMENT ON` (documentation requirement)

**Warning rules (execute with feedback):**
- `SELECT *` detected (suggest specific columns)
- Queries with excessive JOINs
- Columns created without `COMMENT ON COLUMN` (suggested, not required)

### Stage 2: LLM Critic (Optional)

A second LLM evaluates the query for security and optimization. Activated by `guardrail_llm.enabled: true`.

**Returns:**
- **Security assessment:** OK or BLOCK (with explanation). Blocks are enforced — the query does not execute.
- **Optimization suggestions:** Non-blocking feedback (missing indexes, unnecessary columns, missing LIMIT, etc.)

The LLM critic catches risks that static rules cannot detect, such as:
- Mass updates that represent business decisions requiring human review
- Queries that are syntactically valid but semantically dangerous
- Subtle data leakage patterns

### Documentation Enforcement

When the agent creates objects in the sandbox:
- **Tables and functions:** `COMMENT ON TABLE/FUNCTION` is **required** (blocking).
- **Columns:** `COMMENT ON COLUMN` is **suggested** (warning, non-blocking).

---

## Execution Flow

```
 1. LLM generates tool call: query = "SELECT id, total FROM production.orders WHERE..."
        |
 2. DagToolExecutor receives tool call
        |  (merges fixed values + LLM args, same as http_request)
        |
 3. SqlNode.execute() receives inputs + config
        |
 4. Pool exists?
        |  No  --> create pool with connection_url, set statement_timeout, work_mem
        |  Yes --> reuse
        |
 5. Resolve permissions (preset + deny --> final set of allowed operations)
        |
 6. STATIC VALIDATOR (<1ms)
        |  |-- Parse operation type (SELECT/INSERT/UPDATE/DELETE/CREATE/DROP...)
        |  |-- Operation permitted? --> No: BLOCK
        |  |-- Schema permitted? --> No: BLOCK
        |  |-- DELETE/UPDATE without WHERE? --> BLOCK
        |  |-- DROP/TRUNCATE/ALTER on protected schema? --> BLOCK
        |  |-- CREATE without COMMENT? --> BLOCK
        |  |-- SELECT *? --> WARNING
        |  |-- Columns without COMMENT? --> WARNING
        |
 7. LLM CRITIC (only if guardrail_llm.enabled = true)
        |  |-- Send query + schema context to critic LLM
        |  |-- Receive: { security: ok/block, optimization: [...suggestions] }
        |  |-- Security = block? --> BLOCK with explanation
        |  |-- Optimization suggestions? --> attach to result
        |
 8. EXECUTE on PostgreSQL
        |  |-- SELECT --> rows (capped at max_rows)
        |  |-- INSERT --> { rows_affected, returning }
        |  |-- UPDATE --> { rows_affected }
        |  |-- DELETE --> { rows_affected }
        |  |-- CREATE FUNCTION --> { created: true, name, schema }
        |
 9. POST-EXECUTION
        |  |-- If CREATE FUNCTION --> register in sandbox.function_registry
        |  |-- If warnings from validator --> save to sandbox.query_feedback
        |  |-- If suggestions from critic --> save to sandbox.query_feedback
        |  |-- Emit events via ExecutionObserver
        |
10. RETURN to LLM
        {
          "output": [ ... rows or result ... ],
          "row_count": 47,
          "truncated": false,
          "warnings": ["Prefer specific columns instead of *"],
          "optimization_hints": ["Consider adding index on orders.status"]
        }
```

---

## Output Format

### SELECT Results

```json
{
  "output": [
    { "id": 1, "total": 150.00, "status": "pending" },
    { "id": 2, "total": 89.50, "status": "pending" }
  ],
  "row_count": 2,
  "truncated": false
}
```

When results exceed `max_rows`, `truncated: true` signals the agent that more data exists.

### Mutation Results (INSERT/UPDATE/DELETE)

```json
{
  "output": { "rows_affected": 3 },
  "row_count": 3,
  "truncated": false
}
```

### CREATE FUNCTION Result

```json
{
  "output": { "created": true, "name": "calculate_revenue", "schema": "sandbox" },
  "row_count": 0,
  "truncated": false
}
```

### Blocked Query Result (returned to agent as tool error)

```json
{
  "error": "BLOCKED by static validator: DELETE without WHERE clause is not allowed. Specify which rows to delete.",
  "source": "static_validator"
}
```

---

## LLM Context Injection

During `initialize()`, the node connects to PostgreSQL and loads metadata. This is injected into the tool description that the LLM sees:

```
Tool: query_database
Description: Query the production database to answer user questions.

Available tables (schema: production):
  - orders -- Customer purchase orders
  - customers -- Registered customer accounts
  - products -- Product catalog
  - order_items -- Line items within each order

Available functions (schema: sandbox):
  - calculate_monthly_revenue(month int) -- Returns total revenue for a given month
  - customer_lifetime_value(cid int) -- Calculates LTV for a customer

Permissions: SELECT only | Max rows: 100
Use introspection queries to discover column details when needed.

Parameters:
  - query (string, required): SQL query to execute
```

Table descriptions come from PostgreSQL `COMMENT ON TABLE`. Function descriptions come from `sandbox.function_registry`. If a table has no comment, only the name is listed.

---

## Database Schema (Sandbox Tables)

Created automatically during `initialize()` if they don't exist:

```sql
CREATE SCHEMA IF NOT EXISTS sandbox;

CREATE TABLE IF NOT EXISTS sandbox.function_registry (
    id SERIAL PRIMARY KEY,
    function_name TEXT NOT NULL,
    schema_name TEXT NOT NULL DEFAULT 'sandbox',
    parameters TEXT,
    return_type TEXT,
    description TEXT NOT NULL,
    created_by_session TEXT,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    last_used_at TIMESTAMPTZ,
    usage_count INT DEFAULT 0,
    UNIQUE(schema_name, function_name)
);

COMMENT ON TABLE sandbox.function_registry
IS 'Registry of SQL functions created by AI agents in the sandbox schema';

CREATE TABLE IF NOT EXISTS sandbox.query_feedback (
    id SERIAL PRIMARY KEY,
    session_id TEXT NOT NULL,
    query_text TEXT NOT NULL,
    feedback_type TEXT NOT NULL,
    source TEXT NOT NULL,
    message TEXT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

COMMENT ON TABLE sandbox.query_feedback
IS 'Feedback history from static validator and LLM critic on agent queries';
```

---

## Connection Management

- **Pool:** Created once during `initialize()`, reused across all tool calls within the DAG run.
- **Runtime limits:** Applied at session level via `SET statement_timeout` and `SET work_mem` on each connection checkout.
- **Credentials:** Resolved via `${ENV_VAR}` syntax (same as LLM `api_key`). Compatible with `secure_values` if the connection string comes from another node's output.
- **Library:** `sqlx` with `postgres` + `runtime-tokio-rustls` features.

---

## Integration Points

### DagToolExecutor

- Detects `node_type: "sql_query"` in `tool_configurations`
- Checks if node implements `InitializableNode` and calls `initialize()` before first use
- Uses returned `InitContext` to enrich the tool description with database metadata
- Passes pool reference to the node on each tool call execution

### ExecutionObserver

All validation results, critic feedback, and execution events are emitted through the existing `ExecutionObserver` trait. This provides visibility in CLI logs and the REST server frontend.

### Secure Values

No new mechanisms needed. The existing `${ENV_VAR}` resolution and `secure_values` hash/inject flow apply to `connection_url` without modification.

### Node Registry

`SqlNode` is registered alongside `HttpNode` and `SocketIoNode` in the node registry. The node type string is `"sql_query"`.
