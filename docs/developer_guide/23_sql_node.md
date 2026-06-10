# 23. SQL Query Node (`sql_query`)

## Overview

The `sql_query` node executes PostgreSQL queries with granular permission control and a hybrid validation pipeline. It is designed primarily as an **LLM tool** — configure connection, permissions, and guardrails as fixed values in `node_schema`, exposing only the `query` parameter to the LLM.

**Key capabilities:**

| Feature | Description |
|---|---|
| Permission presets | `read_only`, `read_write`, `full` with optional `deny` list |
| Static validator | Blocks dangerous operations (TRUNCATE, DROP, ALTER, DELETE without WHERE) |
| LLM critic (optional) | A second LLM reviews queries for security risks before execution |
| Schema introspection | Automatically injects table/function metadata into tool descriptions |
| Sandbox schema | Isolated schema for agent-created functions and tables |
| Multi-tenant RLS | Row-Level Security via `tenant_user_id`, `tenant_column`, `auto_rls` |
| Runtime limits | `statement_timeout`, `work_mem`, `max_rows` per query |
| Function registry | Tracks agent-created functions with metadata and usage |

**Source:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`
**Registered as:** `"sql_query"` in the node registry

---

## Architecture

The SQL node follows the hexagonal architecture pattern with clear separation of concerns:

```
┌─────────────────────────────────────────────────────────┐
│                    SqlNode (node)                        │
│              infrastructure/nodes/sql.rs                 │
└────────────────────┬────────────────────────────────────┘
                     │ depends on
┌────────────────────▼────────────────────────────────────┐
│              SqlExecutionService (use case)              │
│         application/sql_execution_service.rs             │
│                                                          │
│   Pipeline: validate → critic → execute → post-process   │
└────────────────────┬────────────────────────────────────┘
                     │ depends on ports (traits)
┌────────────────────▼────────────────────────────────────┐
│                 Domain Ports (traits)                     │
│              domain/sql_ports.rs                         │
│                                                          │
│   SqlConnectionPort    — pool + query execution          │
│   SqlValidatorPort     — static rule validation          │
│   SqlCriticPort        — LLM-based security review       │
│   FunctionRegistryPort — sandbox function tracking       │
└────────────────────┬────────────────────────────────────┘
                     │ implemented by
┌────────────────────▼────────────────────────────────────┐
│             Infrastructure Adapters                       │
│                                                          │
│   PgPoolAdapter          — sqlx PgPool wrapper           │
│   StaticRuleValidator    — regex + rule-based checks     │
│   LlmCriticAdapter       — LLM call via LlmRepository   │
│   PgRegistryAdapter      — function_registry + feedback  │
└──────────────────────────────────────────────────────────┘
```

**Related domain files:**

| File | Purpose |
|---|---|
| `domain/sql_permissions.rs` | `SqlPermissions` struct, presets, deny list, RLS config |
| `domain/sql_errors.rs` | `SqlNodeError` enum: Blocked, CriticRejected, ConnectionError, ExecutionError, ConfigError |
| `domain/sql_ports.rs` | Port traits + data types (QueryResult, ValidationResult, CriticResult, FunctionInfo, TableInfo) |

---

## Configuration Reference

All string config fields support `${VAR_NAME}` environment variable resolution.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `connection_url` | string | **Yes** | — | PostgreSQL connection URL (e.g., `${DATABASE_URL}`) |
| `permissions` | object | No | `read_only` preset | Permission configuration (see Permissions section) |
| `runtime_limits` | object | No | see below | Runtime limits per query (see Runtime Limits section) |
| `guardrail_enabled` | boolean | No | `true` | Enable static validation rules |
| `guardrail_llm` | object | No | `{ enabled: false }` | LLM critic configuration (see Guardrail LLM section) |
| `query` | string | **Yes** | — | The SQL query to execute |

### Permissions Object

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `preset` | string | No | `"read_only"` | Permission preset: `read_only`, `read_write`, `full` |
| `deny` | array | No | `[]` | Operations to deny from the preset (e.g., `["delete"]`) |
| `allowed_schemas` | array | Recommended | `[]` (all) | PostgreSQL schemas the agent can access |
| `sandbox_schema` | string | No | `"sandbox"` | Schema for agent-created functions/tables |
| `tenant_user_id` | string | No | `null` | User ID for RLS isolation. Supports `${ENV_VAR}` |
| `tenant_column` | string | No | `"user_id"` | Column name for tenant isolation in RLS policies |
| `auto_rls` | boolean | No | `false` | Auto-create RLS policies during init and after CREATE TABLE |
| `create_schemas_if_missing` | boolean | No | `true` | At init, create any `allowed_schemas` that don't exist (operator-driven). Hard-fails init if creation fails. Set `false` for legacy validate-only behavior |

#### Permission Presets

| Preset | Allowed Operations |
|---|---|
| `read_only` | SELECT |
| `read_write` | SELECT, INSERT, UPDATE |
| `full` | SELECT, INSERT, UPDATE, DELETE, CREATE FUNCTION, CREATE TABLE |

**Always blocked (no preset enables these):** TRUNCATE, DROP, ALTER

#### Deny List

The `deny` array removes operations from the preset. Example: `{ "preset": "full", "deny": ["delete"] }` allows everything except DELETE.

Valid deny values: `select`, `insert`, `update`, `delete`, `create_function`, `create_table`

### Runtime Limits Object

| Field | Type | Default | Description |
|---|---|---|---|
| `max_rows` | integer | `100` | Maximum rows returned by SELECT. Results exceeding this are truncated |
| `statement_timeout_ms` | integer | `30000` | Maximum query execution time in milliseconds |
| `work_mem_mb` | integer | `64` | PostgreSQL `work_mem` for sort/hash operations |

### Guardrail LLM Object

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `false` | Activate the LLM critic |
| `provider` | string | `"openai"` | LLM provider (`openai`, `google`, `anthropic`) |
| `model` | string | `"gpt-4o-mini"` | Model for security analysis |
| `api_key` | string | — | API key for the critic LLM. Supports `${ENV_VAR}` |

---

## Input Ports

| Port | Type | Description |
|---|---|---|
| `query` | string | SQL query to execute (**default input**) |

When used as an LLM tool via `tool_configurations`, all `node_schema` fixed values arrive in inputs (not config). The node reads everything from inputs for tool-call compatibility.

**Default input:** `query`

---

## Output Ports

| Port | Type | Description |
|---|---|---|
| `output` | any | Query results (**default output**) |
| `row_count` | integer | Number of rows returned or affected |
| `truncated` | boolean | Whether results were truncated due to `max_rows` |

**Default output:** `output`

### SELECT Output

```json
{
  "output": [
    { "id": 1, "name": "Alice", "email": "alice@example.com" },
    { "id": 2, "name": "Bob", "email": "bob@example.com" }
  ],
  "row_count": 2,
  "truncated": false
}
```

#### Tipo-mapping de columnas (PgRow → JSON)

| Postgres type | JSON output | Notas |
|---|---|---|
| `INT2`, `INT4`, `INT8`, `OID` | JSON number | enteros exactos |
| `FLOAT4`, `FLOAT8` | JSON number | f64, ~15-17 sig digits |
| `NUMERIC`, `DECIMAL` | JSON number | decodifica vía `BigDecimal` → `f64`. Para precisión arbitraria castear a `TEXT` en el SELECT. Fix shipped 2026-06-09. |
| `BOOL` | JSON boolean | |
| Cualquier otro (`TEXT`, `VARCHAR`, `TIMESTAMPTZ`, `JSONB`, etc.) | JSON string | fallback `try_get::<String, _>()`. `TIMESTAMPTZ` queda en formato ISO 8601 estándar; `JSONB` viene como string serializado — el LLM puede `JSON.parse`. |
| Tipos no soportados (`MONEY`, `NUMRANGE`, `INTERVAL`, arrays nativos, custom types) | JSON null | El marshaller no los reconoce. Workaround: castear a TEXT en el SELECT. |

Para columnas `NUMERIC(p,s)` con `p > 15` (alta precisión, e.g. systems
financieros estrictos), recomendamos castear a TEXT:

```sql
SELECT id, amount::TEXT AS amount_text FROM big_money_table;
```

El LLM recibe un string que puede ecoear verbatim sin pérdida.

### Mutation Output (INSERT, UPDATE, DELETE)

```json
{
  "output": { "rows_affected": 3 },
  "row_count": 3,
  "truncated": false
}
```

### CREATE FUNCTION Output

```json
{
  "output": { "created": true },
  "row_count": 0,
  "truncated": false
}
```

### CREATE TABLE Output

```json
{
  "output": { "created": true, "type": "table" },
  "row_count": 0,
  "truncated": false
}
```

### Error Envelope

On failure, the node returns an error envelope **without throwing**. This allows downstream nodes to handle errors gracefully:

```json
{
  "error": "BLOCKED by static validator (static_validator): DELETE without a WHERE clause is not allowed.",
  "source": "static_validator"
}
```

The `source` field indicates where the error originated: `"static_validator"`, `"llm_critic"`, or `"execution"`.

### Warnings and Optimization Hints

When present, these are appended to the output:

```json
{
  "output": [...],
  "row_count": 10,
  "truncated": false,
  "warnings": ["Prefer selecting specific columns instead of SELECT *"],
  "optimization_hints": ["Consider adding an index on column 'email'"]
}
```

---

## Execution Pipeline

Every query goes through a four-stage pipeline orchestrated by `SqlExecutionService`:

```
Query
  │
  ▼
┌─────────────────────────┐
│  Stage 1: Static Rules  │  StaticRuleValidator
│  - Operation detection  │  (sync, <1ms)
│  - Permission check     │
│  - Schema access check  │
│  - Safety rules         │
│    (WHERE required for  │
│     DELETE/UPDATE)      │
│  - COMMENT required for │
│    CREATE FUNCTION      │
└──────────┬──────────────┘
           │ allowed
           ▼
┌─────────────────────────┐
│  Stage 2: LLM Critic    │  LlmCriticAdapter
│  (optional)             │  (async, ~1-3s)
│  - Security assessment  │
│  - Optimization hints   │
└──────────┬──────────────┘
           │ security_ok
           ▼
┌─────────────────────────┐
│  Stage 3: Execute       │  PgPoolAdapter
│  - SET LOCAL tenant ctx │  (async)
│  - Run query            │
│  - Truncate to max_rows │
└──────────┬──────────────┘
           │ result
           ▼
┌─────────────────────────┐
│  Stage 4: Post-process  │
│  - Record feedback      │  PgRegistryAdapter
│  - Register functions   │  (async)
│  - Auto-RLS on CREATE   │
│    TABLE                │
└─────────────────────────┘
```

### Static Validation Rules

The `StaticRuleValidator` enforces these rules synchronously:

| Rule | Behavior |
|---|---|
| Unknown operation | **Block** — only SELECT, INSERT, UPDATE, DELETE, CREATE FUNCTION, CREATE TABLE recognized |
| TRUNCATE, DROP, ALTER | **Block** — always, regardless of preset |
| Operation not in preset | **Block** — e.g., INSERT on `read_only` |
| Schema not in `allowed_schemas` | **Block** — except `information_schema` and `pg_catalog` (always allowed for introspection) |
| DELETE/UPDATE without WHERE | **Block** — prevents mass data changes |
| CREATE FUNCTION without COMMENT | **Block** — forces documentation of agent-created functions |
| SELECT * | **Warn** — non-blocking, suggests specifying columns |

### LLM Critic

When `guardrail_llm.enabled: true`, a secondary LLM analyzes each query before execution. The critic checks for:

- Mass UPDATE/DELETE affecting potentially thousands of rows
- Queries that could leak sensitive data (password, token, secret columns)
- SQL injection patterns or dynamic SQL construction
- Optimization opportunities (missing LIMIT, subqueries → CTEs, missing indexes)

The critic uses a low-temperature call (`temperature: 0.0`, `max_tokens: 500`) for deterministic responses. It **fails open** — if the LLM response can't be parsed, the query is allowed.

---

## Initialization and Schema Introspection

The SQL node implements `InitializableNode`, which runs once when the node is first loaded (either at DAG startup or on first tool call):

1. **Connect** — Obtains the pool from the shared `PgPoolRegistry` via `SqlPortFactory::get_adapter(url)`. If another node (or the internal state repository) already opened a pool for this URL, it is **reused**; otherwise a new pool is created and cached. TOCTOU-safe via `tokio::sync::OnceCell`.
2. **Provision schemas** (if `create_schemas_if_missing`, default `true`) — Checks each schema in `allowed_schemas` and creates the ones that don't exist. See below.
3. **Ensure sandbox** — Creates the sandbox schema and registry tables (`function_registry`, `query_feedback`)
4. **Load metadata** — Queries `information_schema.tables` for table names and `pg_catalog.obj_description` for table comments
5. **Load functions** — Queries `sandbox.function_registry` for registered functions
6. **Build description supplement** — Generates a text block listing available tables, functions, permissions, and max_rows
7. **Auto-RLS** (if enabled) — Sets up RLS policies on all existing tables

### Operator-Driven Schema Provisioning

When `permissions.create_schemas_if_missing` is `true` (the default), initialization
checks each schema listed in `allowed_schemas` **one at a time** and runs
`CREATE SCHEMA IF NOT EXISTS "<schema>"` (quoted identifier) for the ones that don't
yet exist. Schemas that already exist are left untouched and never trigger a
`CREATE`, so a read-only agent pointing at existing schemas needs no `CREATE`
privilege.

Key properties:

- **Operator-driven, not LLM-driven.** The schema names come from the node's
  fixed `allowed_schemas` config, authored by the operator. This is a different
  trust level than a query — **LLM-issued `CREATE SCHEMA` in a query stays blocked**
  by the static validator regardless of this flag.
- **Hard-fail on error.** If a missing schema can't be created (e.g. the database
  role lacks `CREATE` privilege on the database), node initialization fails with
  `Failed to create schema '<name>': ...` and the node does not start.
- **Introspection schemas are skipped.** `information_schema` and `pg_catalog`
  are never treated as missing or created.
- **No-op when empty.** An empty `allowed_schemas` means "all schemas allowed"
  (no restriction), so there is nothing to provision.

Set `create_schemas_if_missing: false` to restore the legacy behavior where
`allowed_schemas` is purely a validation allowlist and missing schemas surface
as a query-time error instead.

### Shared-pool behavior

Distintos nodos SQL con el **mismo** `connection_url` comparten el mismo `PgPoolAdapter` y por lo tanto el mismo pool sqlx. Implicaciones:

- El `statement_timeout_ms` y `work_mem_mb` se aplican con `SET LOCAL` dentro de la transacción de cada query — no afectan a otras conexiones del pool.
- Si un nodo apunta al mismo URL que el `DATABASE_URL` del proceso, reutiliza el pool **pinned** del engine.
- Cambiar `sslmode` crea una nueva entrada (pools separados). Ver [`13_security_strategy.md`](./13_security_strategy.md#postgres-pgpoolregistry) para la tabla de modos.

The **description supplement** is automatically appended to the tool's description when used as an LLM tool. This gives the LLM context about the database schema without manual configuration:

```
Available tables (schema: public):
  - users -- User accounts and profiles
  - orders -- Customer orders
  - products

Available functions (sandbox):
  - calculate_total(order_id INT) -- Computes total for an order

Permissions: SELECT, INSERT, UPDATE | Max rows: 50
Use introspection queries to discover column details when needed.
```

---

## Multi-Tenant Row-Level Security (RLS)

The SQL node supports automatic multi-tenant isolation using PostgreSQL Row-Level Security:

### How It Works

1. **Configuration** — Set `tenant_user_id`, `tenant_column`, and `auto_rls: true` in permissions
2. **On initialization** — For each table in `allowed_schemas`:
   - If the table has the `tenant_column`: enables RLS + creates a `colmena_tenant_isolation` policy that filters rows by `current_setting('app.current_user_id')`
   - If the table lacks the `tenant_column`: enables RLS + creates a `colmena_shared_read` policy (SELECT only)
3. **On every query** — Runs `SET LOCAL app.current_user_id = '<tenant_user_id>'` inside the transaction before executing the query
4. **On CREATE TABLE** — If `auto_rls` is enabled, automatically adds the `tenant_column` (if missing) and sets up RLS policies on the new table

### RLS Policy Details

**Tenant isolation policy** (`colmena_tenant_isolation`):
```sql
CREATE POLICY colmena_tenant_isolation ON schema.table
  USING (tenant_column = current_setting('app.current_user_id'))
  WITH CHECK (tenant_column = current_setting('app.current_user_id'));
```

- `USING` filters rows on SELECT, UPDATE, DELETE
- `WITH CHECK` validates rows on INSERT and UPDATE
- `FORCE ROW LEVEL SECURITY` ensures the policy applies even to the table owner

**Shared read policy** (`colmena_shared_read`):
```sql
CREATE POLICY colmena_shared_read ON schema.table
  FOR SELECT USING (true);
```

Tables without the tenant column become read-only — all users can SELECT but nobody can modify.

### Tenant Column Auto-Default

When `auto_rls` creates or discovers a tenant column, it sets a DEFAULT:
```sql
ALTER TABLE schema.table ALTER COLUMN tenant_column
  SET DEFAULT current_setting('app.current_user_id');
```

This means INSERT statements don't need to explicitly include the tenant column — PostgreSQL fills it automatically from the session variable.

---

## Tipos de Statement Soportados

Las consultas se analizan con el dialecto PostgreSQL del crate `sqlparser`.
Cada statement de un script (los scripts multi-statement se validan
statement a statement) debe ser uno de:

| Operación | Notas |
|-----------|-------|
| `SELECT` / `WITH … SELECT` | Sujeto a `max_rows`; se agrega `LIMIT max_rows + 1` automáticamente solo si no hay LIMIT explícito en la consulta. |
| `INSERT` | Se admiten VALUES multi-fila e INSERT … SELECT. No hay límite de filas (sujeto a `statement_timeout_ms`). |
| `UPDATE` | Requiere cláusula `WHERE`. |
| `DELETE` | Requiere cláusula `WHERE`. |
| `CREATE TABLE` | Solo dentro de `allowed_schemas`. Si `auto_rls` está activo, se aplica RLS tras la creación. |
| `CREATE FUNCTION` | Debe ir acompañado de un statement `COMMENT ON FUNCTION` en el mismo script. Se registra en el function registry. |
| `COMMENT ON` | Permitido cuando el objeto referenciado pertenece a un schema en `allowed_schemas` (solo metadatos, sin exposición de datos). |

**Siempre bloqueados:** `DROP`, `ALTER`, `TRUNCATE`, `CREATE SCHEMA`, `CREATE INDEX`,
`CREATE VIEW`, `GRANT`, `REVOKE`. Gestiona el ciclo de vida de schemas, índices,
vistas y permisos mediante tus herramientas de migración, no a través del LLM.

Si `sqlparser` no puede parsear la consulta, el nodo devuelve un error
`Failed to parse SQL: …` y se niega a ejecutarla — no existe fallback a
comparación de strings heurística.

---

## Multi-statement queries (Política C, shipped 2026-06-09)

### Problema que resuelve

`sqlx::query(…).execute()` usa el **extended protocol** de Postgres
(PREPARE + BIND + EXECUTE) que solo acepta UN SQL command por mensaje.
Cuando el LLM escribía multi-statement (super natural, ej. dos INSERTs
separados por `;\n`), Postgres respondía con error críptico:

```
cannot insert multiple commands into a prepared statement
```

### Pipeline

```
┌──────────────────────────────────────────────────────────────────┐
│ Política C — sqlx::query() loop sobre AST statements             │
├──────────────────────────────────────────────────────────────────┤
│ BEGIN (auto)                                                     │
│   SET LOCAL statement_timeout = ...                              │
│   SET LOCAL work_mem = ...                                       │
│   SELECT set_config('app.current_user_id', ...)   ← if tenant   │
│                                                                  │
│   for (idx, stmt) in stmts.iter().enumerate():                   │
│     let is_last = idx == last_idx;                               │
│     let stmt_sql = stmt.to_string();   // sqlparser reserializes │
│                                                                  │
│     if is_select && is_last:                                     │
│       sqlx::query(limited_sql).fetch_all → marshall rows         │
│       → return Ok({output: rows, row_count, truncated})          │
│     elif is_select:                                              │
│       sqlx::query(stmt_sql).execute  // rows discarded           │
│     else (mutation):                                             │
│       result = sqlx::query(stmt_sql).execute                     │
│       rows_affected_sum += result.rows_affected()                │
│       if is_last:                                                │
│         match stmt {                                             │
│           CreateFunction → {created: true}                       │
│           CreateTable    → {created: true, type: "table"}        │
│           _              → {rows_affected: sum}                  │
│         }                                                        │
│         → return Ok                                              │
│                                                                  │
│ COMMIT (auto) ⤴ inside each branch's return path                 │
└──────────────────────────────────────────────────────────────────┘
```

### Reglas exactas

| Caso | Output |
|---|---|
| Último statement es SELECT | `output: Vec<Value>`, `row_count: N`, LIMIT auto si no tiene |
| Último statement es INSERT/UPDATE/DELETE | `output: {rows_affected: SUM_de_todos_los_mutadores}` |
| Último statement es CREATE TABLE | `output: {created: true, type: "table"}` |
| Último statement es CREATE FUNCTION | `output: {created: true}` |
| Cualquier statement falla | Rollback completo, error retornado al caller |
| Query con solo whitespace o solo comentarios | Error: `empty SQL script` |

**Intermediate SELECTs**: se ejecutan (para que sus side effects como
`set_config` corran), pero las rows se descartan. **NO se inyecta
LIMIT** en SELECTs intermedios — solo en el último.

### Por qué no usamos `sqlx::raw_sql`

`raw_sql` ejecuta multi-statement en simple protocol pero NO permite:
- Recuperar rows tipadas del SELECT final (devuelve `PgQueryResult`, no `PgRow`).
- Aplicar `LIMIT` automático per-statement.
- Distinguir CREATE TABLE de INSERT en el output.

La iteración manual por `Vec<Statement>` (AST output) da todo eso a
costa de N round-trips a Postgres por N statements. Para los workloads
típicos del LLM (1–10 stmts/call) el costo es despreciable.

### Backward compatibility

Queries single-statement siguen funcionando idéntico — el loop entra al
if/else apropiado en la primera iteración y retorna. No hay cambio en
el wire format de `QueryResult`.

### `auto_rls` + multi-statement

Si `permissions.auto_rls = true`, el post-execution hook itera TODOS
los stmts del script buscando `CREATE TABLE` (vía
`sql_ast::created_table_name`) y aplica RLS a cada uno. Multi-statement
con varios `CREATE TABLE` funciona; cada tabla nueva recibe sus
policies.

### Cómo lo ve el LLM

El `description_supplement` que el nodo construye trae siempre un
bloque corto con multi-statement + anti-patterns visuales (BEGIN, $1,
TRUNCATE, etc.). Para profundizar, los operators pueden habilitar la
skill built-in `sql-query-best-practices` vía
`llm_call.skills.paths` — tiene 6 references on-demand
(`multi_statement`, `bulk_insert`, `select_after_mutation`,
`anti_patterns`, `schema_discovery`, `error_recovery`).

Ver:
- `src/libs/colmena/skills/sql-query-best-practices/SKILL.md`
- Tests: `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs`
  → `pc_*` tests (7 integration tests `#[ignore]`-gated, run con
  `TEST_DATABASE_URL=$DATABASE_URL cargo test -- --ignored`).

---

## Sandbox Schema and Function Registry

The `sandbox` schema (configurable via `sandbox_schema`) provides an isolated space for agent-created objects:

### Function Registry

When an agent executes a `CREATE FUNCTION` statement, the node automatically registers it in `sandbox.function_registry`:

| Column | Type | Description |
|---|---|---|
| `function_name` | TEXT | Function name |
| `schema_name` | TEXT | Schema (defaults to sandbox) |
| `parameters` | TEXT | Parameter signature (nullable) |
| `return_type` | TEXT | Return type (nullable) |
| `description` | TEXT | From COMMENT ON FUNCTION (required) |
| `created_by_session` | TEXT | Session ID that created the function |
| `created_at` | TIMESTAMPTZ | Creation timestamp |
| `usage_count` | INT | Number of times invoked |

### Query Feedback

All validation results (blocked queries, warnings, optimization hints) are recorded in `sandbox.query_feedback`:

| Column | Type | Description |
|---|---|---|
| `session_id` | TEXT | Session identifier |
| `query_text` | TEXT | The SQL query |
| `feedback_type` | TEXT | `blocked`, `warning`, or `optimization` |
| `source` | TEXT | `static_validator` or `llm_critic` |
| `message` | TEXT | Feedback message |

---

## Environment Variable Resolution

All string values in config support `${VAR_NAME}` syntax:

```json
{
  "connection_url": "${DATABASE_URL}",
  "permissions": {
    "tenant_user_id": "${CURRENT_USER_ID}"
  },
  "guardrail_llm": {
    "api_key": "${OPENAI_API_KEY}"
  }
}
```

Resolution is **not recursive** — only top-level string values containing `${...}` are expanded. Nested objects within `permissions` or `runtime_limits` are JSON objects, not strings.

---

## Example 1: Standalone Node — Read-Only Query

A minimal graph that executes a fixed SQL query and logs the result.

```json
{
  "comment": "Standalone sql_query: read-only select",
  "metadata": {
    "category": "external",
    "requires_env": ["DATABASE_URL"]
  },
  "nodes": {
    "trigger": {
      "type": "input",
      "config": {
        "query": "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'"
      }
    },
    "sql": {
      "type": "sql_query",
      "config": {
        "connection_url": "${DATABASE_URL}",
        "permissions": {
          "preset": "read_only",
          "allowed_schemas": ["public"]
        }
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "trigger", "to": "sql" },
    { "from": "sql", "to": "log_result" }
  ]
}
```

**What happens:**
1. `trigger` emits `{ "query": "SELECT table_name ..." }` as output
2. `sql` receives the query on its default input (`query`), connects to PostgreSQL, validates, and executes
3. `log_result` receives `sql.output` (the default output) — an array of row objects

---

## Example 2: As LLM Tool — Data Analyst Agent

An LLM agent that uses `sql_query` as a tool via `tool_configurations`. The LLM controls only the `query` parameter; connection, permissions, and guardrails are fixed.

```json
{
  "comment": "LLM agent with SQL query tool for data analysis",
  "metadata": {
    "category": "agents",
    "requires_env": ["OPENAI_API_KEY", "DATABASE_URL"]
  },
  "nodes": {
    "sql_agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "${OPENAI_API_KEY}",
        "system_message": "You are a helpful data analyst. Use the query_database tool to answer questions. Always start by listing available tables.",
        "enabled_tools": ["query_database"],
        "stream": false,
        "tool_configurations": {
          "query_database": {
            "name": "query_database",
            "node_type": "sql_query",
            "description": "Query the production database.",
            "node_schema": {
              "connection_url": {
                "type": "string",
                "fixed": "${DATABASE_URL}"
              },
              "permissions": {
                "type": "object",
                "fixed": {
                  "preset": "read_only",
                  "allowed_schemas": ["public"]
                }
              },
              "runtime_limits": {
                "type": "object",
                "fixed": {
                  "max_rows": 50,
                  "statement_timeout_ms": 15000,
                  "work_mem_mb": 32
                }
              },
              "guardrail_enabled": {
                "type": "boolean",
                "fixed": true
              },
              "guardrail_llm": {
                "type": "object",
                "fixed": { "enabled": false }
              },
              "query": {
                "type": "string",
                "required": true,
                "description": "SQL SELECT query to execute against the PostgreSQL database."
              }
            }
          }
        },
        "prompt": "What tables are in the database and what is their structure?"
      }
    },
    "result": {
      "type": "output",
      "config": { "label": "SQL Agent Result" }
    }
  },
  "edges": [
    { "from": "sql_agent", "to": "result" }
  ]
}
```

**Key patterns in this example:**

- **`node_schema` with `fixed` fields:** `connection_url`, `permissions`, `runtime_limits`, `guardrail_enabled`, and `guardrail_llm` are all `fixed` — hidden from the LLM and auto-filled at execution time
- **Only `query` is dynamic:** The LLM sees and provides only the SQL query
- **Schema introspection:** The node automatically appends available table names to the tool description, so the LLM knows what tables exist
- **Static validation:** Even though the preset is `read_only`, the static validator provides defense-in-depth against accidental mutations

---

## Example 3: As LLM Tool — Multi-Tenant Todo Manager with RLS

An LLM agent that manages per-user todo lists with automatic Row-Level Security isolation.

```json
{
  "comment": "LLM agent with multi-tenant SQL tool and auto-RLS",
  "metadata": {
    "category": "agents",
    "requires_env": ["OPENAI_API_KEY", "DATABASE_URL"]
  },
  "nodes": {
    "todo_agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "${OPENAI_API_KEY}",
        "system_message": "You are a personal todo manager. Create tables, insert tasks, list and update them. Start by checking if a 'todos' table exists.",
        "enabled_tools": ["manage_todos"],
        "stream": false,
        "tool_configurations": {
          "manage_todos": {
            "name": "manage_todos",
            "node_type": "sql_query",
            "description": "Manage the user's todo list in the database.",
            "node_schema": {
              "connection_url": {
                "type": "string",
                "fixed": "${DATABASE_URL}"
              },
              "permissions": {
                "type": "object",
                "fixed": {
                  "preset": "full",
                  "allowed_schemas": ["public"],
                  "tenant_user_id": "julian",
                  "tenant_column": "user_id",
                  "auto_rls": true
                }
              },
              "runtime_limits": {
                "type": "object",
                "fixed": {
                  "max_rows": 100,
                  "statement_timeout_ms": 15000,
                  "work_mem_mb": 32
                }
              },
              "guardrail_enabled": { "type": "boolean", "fixed": true },
              "guardrail_llm": { "type": "object", "fixed": { "enabled": false } },
              "query": {
                "type": "string",
                "required": true,
                "description": "SQL query to manage the user's todo list."
              }
            }
          }
        },
        "prompt": "What tasks do I still need to do?"
      }
    },
    "result": {
      "type": "output",
      "config": { "label": "Todo Agent Result" }
    }
  },
  "edges": [
    { "from": "todo_agent", "to": "result" }
  ]
}
```

**Key patterns in this example:**

- **`preset: "full"`** allows CREATE TABLE, INSERT, UPDATE, DELETE — needed for a todo manager
- **`auto_rls: true`** automatically creates RLS policies: when the agent runs `CREATE TABLE todos (...)`, the node auto-adds a `user_id` column and sets up tenant isolation
- **`tenant_user_id: "julian"`** — every query runs within julian's tenant context. Other users' data is invisible
- **Defense in depth:** Even with `full` permissions, TRUNCATE/DROP/ALTER are always blocked, and DELETE requires WHERE

---

## Troubleshooting

### "sql_query node requires 'connection_url' in config"

**Cause:** No `connection_url` provided in config or inputs.

**Solution:** Set `connection_url` in config or provide it via a fixed `node_schema` field. Ensure the environment variable (e.g., `${DATABASE_URL}`) is exported.

### "BLOCKED by static validator: ... is not permitted by the current permission preset"

**Cause:** The query's operation type is not allowed by the configured preset.

**Solutions:**
- Change the preset (e.g., `read_only` → `read_write` for INSERT/UPDATE)
- Check the `deny` list isn't removing the needed operation
- Verify you're using the right preset for the use case

### "BLOCKED by static validator: DELETE without a WHERE clause"

**Cause:** Safety rule prevents mass deletion.

**Solution:** Add a WHERE clause to target specific rows. This rule cannot be disabled.

### "BLOCKED by static validator: CREATE FUNCTION requires a COMMENT ON FUNCTION"

**Cause:** Agent-created functions must be documented.

**Solution:** Append `COMMENT ON FUNCTION schema.func_name() IS 'description'` in the same query.

### "BLOCKED by LLM critic: ..."

**Cause:** The LLM critic flagged the query as a security risk.

**Solutions:**
- Review the critic's reason — it may have detected a legitimate risk
- If the query is safe, disable the critic (`guardrail_llm.enabled: false`) or adjust the critic model
- Check `sandbox.query_feedback` for the full feedback history

### "Env var X not found"

**Cause:** A `${VAR_NAME}` reference points to an undefined environment variable.

**Solution:** Export the variable before running the DAG: `export VAR_NAME=value`

### "Failed to initialize SQL pool"

**Cause:** Cannot connect to PostgreSQL.

**Solutions:**
- Verify the connection URL is correct and the database is running
- Check network connectivity and firewall rules
- Ensure the PostgreSQL user has sufficient privileges
- Check that `${ENV_VAR}` references in the connection URL resolve correctly

### Empty results with RLS enabled

**Cause:** The `tenant_user_id` doesn't match any rows, or the tenant column name is wrong.

**Solutions:**
- Verify `tenant_user_id` matches values in the `tenant_column`
- Check that `tenant_column` matches the actual column name in the table
- Query `pg_policies` to inspect active RLS policies
- Verify `app.current_user_id` is being set correctly (check `[SqlNode]` log output)

### Query timeout

**Cause:** Query exceeded `statement_timeout_ms`.

**Solutions:**
- Increase `statement_timeout_ms` in `runtime_limits`
- Optimize the query (add indexes, reduce result set)
- Check if the LLM critic is adding latency (disable if not needed)

---

## attachment_run_python (shipped 2026-06-10)

Tool sintético que **carga un CSV/XLSX adjunto en pandas y ejecuta código del LLM
server-side**. La data del archivo nunca cruza al contexto del LLM — solo
viaja el resultado (stdout + `result` global) de vuelta.

### Workflow

```
LLM ─[attachment_run_python({attachment_id, code: "result = df['price'].max()"})]→
       Backend carga DataFrame → ejecuta sandbox → devuelve {result, stdout, ...}
LLM ←[~80 tokens]──
```

### Args

| Field | Tipo | Default | Descripción |
|---|---|---|---|
| `attachment_id` | string (req) | — | document_id del catálogo |
| `code` | string (req) | — | Python source. `df` pre-loaded |
| `delimiter` | string | auto | CSV only |
| `sheet_name` | string | primera | XLSX only |
| `header_row` | int | 1 | 1-indexed |

### Pre-loaded en el sandbox

- `df` — pandas DataFrame del attachment completo
- `pd` (pandas), `np` (numpy), `stats` (scipy.stats)
- stdlib: math, datetime, decimal, json, re, statistics, string, collections,
  functools, itertools

### Cómo surface output

| Camino | Cómo |
|---|---|
| `stdout` | `print(...)` (capturado) |
| `result` | Asignar a global `result`. DataFrames → `to_dict(orient='records')`, Series → `to_list()`, numpy scalars → `.item()` |

### Restricciones de sandbox

Mismo `restricted` mode que `gsheets_run_python`:

- Blocked: `os`, `sys`, `subprocess`, `socket`, `urllib`, `requests`, `importlib`, `builtins`, `ctypes`
- No filesystem, no network
- Wall-clock cap 30 s

### Caps (v1, hardcoded)

| | Valor |
|---|---|
| Attachment size | 50 MB |
| DataFrame rows | 100 000 |
| Code wall-clock | 30 s |
| Response stdout/error | 50 KB |

### Configuración del operador

```jsonc
"tool_configurations": {
  "attachment_run_python": {
    "name": "attachment_run_python",
    "node_type": "attachment_run_python",
    "fixed_config": {}
  }
}
```

No requiere `connection_url` ni `permissions` — el sandbox es self-contained.

### Soporta inline + signed URL

El dispatcher usa el cable compartido (Bulk T0) → `OutputStorageRepository`.
Funciona idénticamente para archivos enviados como `data:` base64 (inline) o
como signed URLs. El LLM no necesita saber el source type.

### Cuándo NO usarlo

- DataFrame > 100K rows → usar `sql_bulk_insert_from_attachment` para meter en DB y luego `sql_query` con filtros
- Archivo > 50 MB → mismo workaround
- Quiere ver TODA la data al contexto → `load_attachment` (último recurso)

### Referencias

- Tool YAML: [`text/tools/sql.yaml`](../../src/libs/colmena/text/tools/sql.yaml) (entry `attachment_run_python`)
- Implementación: [`attachment_run_python.rs`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_run_python.rs)
- E2E graph: [`tests/graphs/agents/attachment_run_python_e2e.json`](../../tests/graphs/agents/attachment_run_python_e2e.json)
- CHANGELOG §23
