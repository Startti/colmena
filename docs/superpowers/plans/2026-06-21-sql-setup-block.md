# `setup_sql` Environment-Bootstrapping Block — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional `setup_sql` field to the `sql_query` node that runs author-defined DDL+seed idempotently at node init (operator trust-level, bypassing the LLM validator), so a published graph auto-provisions its database environment on first use.

**Architecture:** Extend the existing operator-trust init path in `SqlNode::do_initialize_inner` (the same place that already runs `CREATE SCHEMA IF NOT EXISTS` for `allowed_schemas`). A new `SqlConnectionPort::execute_setup_sql` runs the block as one atomic transaction via `sqlx::raw_sql`. It executes after schema provisioning and **before** metadata introspection, so the LLM tool description reflects freshly-created tables. Idempotency is the author's contract (`IF NOT EXISTS` / `ON CONFLICT`); failure hard-fails init with rollback.

**Tech Stack:** Rust, `sqlx` 0.8 (Postgres, `raw_sql`), `async-trait`, hexagonal ports/adapters. Crate: `colmena_dag_engine`.

---

## Background for the implementer (read first)

- **You know nothing about this codebase — here is the only path that matters.** The `sql_query` node lives in `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`. It is used two ways: as a standalone DAG node, and as an LLM tool. In BOTH cases, initialization funnels through one private method, `do_initialize_inner` (around `sql.rs:66`), guarded by a `tokio::sync::OnceCell` so it runs at most once per node instance. That method already does operator-level DDL: it runs `CREATE SCHEMA IF NOT EXISTS` for each schema in `permissions.allowed_schemas`. **`setup_sql` is the same kind of operator DDL, in the same place.**
- **Trust level.** The runtime `query` the LLM sends goes through `StaticRuleValidator` (blocks `CREATE TABLE`, `DROP`, etc.). `setup_sql` does NOT go through that validator — it is author/build-time SQL, exactly like the existing schema provisioning. Do not route it through the validator.
- **Where the field arrives.** As an LLM tool, fixed `node_schema` fields arrive in `inputs` (merged into `effective_config` at `sql.rs:323`). As a standalone node, `InitializableNode::initialize(config)` (`sql.rs:300`) is called with the real config. Both call `get_or_init(...)` → `do_initialize_inner(config)`, so `config.get("setup_sql")` works in both. No routing changes needed.
- **Atomicity & Postgres.** Postgres supports transactional DDL. Wrapping the whole block in one `BEGIN/COMMIT` means a later failed statement rolls back earlier `CREATE`s.
- **Tests need a real Postgres.** Integration tests are `#[ignore]`-gated and read `TEST_DATABASE_URL` (see the existing `test_adapter()` helper at `sql_pool_adapter.rs:571`). Run them locally with the repo `.env` sourced. CI does not run them.
- **Deny-warnings is on** (`Cargo.toml [lints.rust] warnings = "deny"`). No unused imports / dead code or the build fails.

### Files touched

| File | Responsibility | Change |
|---|---|---|
| `src/libs/colmena/src/dag_engine/domain/sql_ports.rs` | `SqlConnectionPort` trait | **Modify** — add `execute_setup_sql` method (~`:76`) |
| `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs` | `PgPoolAdapter` impl + tests | **Modify** — impl method (~`:558`) + 2 integration tests (tests mod) |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs` | `SqlNode` init wiring + new test mod | **Modify** — call `execute_setup_sql` in `do_initialize_inner` (~`:123`) + add `#[cfg(test)] mod` |
| `docs/node_configurations.json` | Canonical node schema | **Modify** — add `setup_sql` to `sql_query` |
| `docs/developer_guide/23_sql_node.md` | SQL node guide | **Modify** — new section |
| `tests/graphs/agents/sql_setup_finanzas.json` | E2E graph | **Create** |

There is only ONE implementor of `SqlConnectionPort` (`PgPoolAdapter`) and NO mock of it — adding a trait method only requires updating that one impl.

---

## Task 1: `execute_setup_sql` primitive on `SqlConnectionPort`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/sql_ports.rs` (trait, ~line 76)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs` (impl ~line 558; tests in `mod tests`)

- [ ] **Step 1: Write the failing tests**

Add these two tests inside the existing `#[cfg(test)] mod tests` block in `sql_pool_adapter.rs` (after the last `pc_*` test, before the closing `}` of the module). They reuse the existing `test_adapter()` helper.

```rust
    /// A unique schema name so parallel test runs never collide.
    fn unique_schema(prefix: &str) -> String {
        format!(
            "{}_{}",
            prefix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn setup_sql_runs_and_is_idempotent() {
        let Some(adapter) = test_adapter().await else {
            eprintln!("skip: TEST_DATABASE_URL not set");
            return;
        };
        let schema = unique_schema("colmena_setup");
        let sql = format!(
            "CREATE SCHEMA IF NOT EXISTS {s};\n\
             CREATE TABLE IF NOT EXISTS {s}.cat (id SERIAL PRIMARY KEY, nombre TEXT UNIQUE NOT NULL);\n\
             INSERT INTO {s}.cat (nombre) VALUES ('a'),('b') ON CONFLICT (nombre) DO NOTHING;",
            s = schema
        );

        // First run creates schema + table + seed.
        adapter.execute_setup_sql(&sql).await.expect("first setup_sql run");
        // Second run is a no-op: no error, no duplicate seed rows.
        adapter.execute_setup_sql(&sql).await.expect("second setup_sql run (idempotent)");

        let count: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {}.cat", schema))
                .fetch_one(&*adapter.pool())
                .await
                .unwrap();
        assert_eq!(count, 2, "seed must not duplicate across runs");

        sqlx::query(&format!("DROP SCHEMA {} CASCADE", schema))
            .execute(&*adapter.pool())
            .await
            .ok();
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn setup_sql_rolls_back_on_failure() {
        let Some(adapter) = test_adapter().await else {
            eprintln!("skip: TEST_DATABASE_URL not set");
            return;
        };
        let schema = unique_schema("colmena_setup_fail");
        // Valid CREATE SCHEMA followed by a garbage statement: the whole tx must roll back,
        // so the schema must NOT exist afterwards.
        let sql = format!(
            "CREATE SCHEMA IF NOT EXISTS {s};\nTHIS IS NOT VALID SQL;",
            s = schema
        );

        let res = adapter.execute_setup_sql(&sql).await;
        assert!(res.is_err(), "invalid setup_sql must return an error");

        let missing = adapter.missing_schemas(&[schema.clone()]).await.unwrap();
        assert_eq!(missing, vec![schema], "failed setup_sql must roll back the CREATE SCHEMA");
    }
```

- [ ] **Step 2: Run the tests to verify they fail to compile**

Run: `set -a && source .env && set +a && TEST_DATABASE_URL=$DATABASE_URL cargo test --lib setup_sql_ -- --ignored --nocapture`
Expected: COMPILE ERROR — `no method named execute_setup_sql found for ... PgPoolAdapter`.

- [ ] **Step 3: Add the trait method**

In `sql_ports.rs`, inside `pub trait SqlConnectionPort`, immediately after the `create_schema` method (around line 76), add:

```rust
    /// Execute an operator-authored setup SQL block (DDL + seed) as a single
    /// atomic transaction. Multi-statement blocks separated by `;` are supported.
    ///
    /// This is **operator trust-level** — it bypasses the LLM static validator,
    /// exactly like schema provisioning. It is intended for author/build-time
    /// environment bootstrapping, never for LLM-issued queries. Idempotency is
    /// the author's responsibility (`CREATE ... IF NOT EXISTS`, `INSERT ... ON
    /// CONFLICT`). Any statement failure rolls back the whole block.
    async fn execute_setup_sql(&self, sql: &str) -> Result<(), SqlNodeError>;
```

- [ ] **Step 4: Implement the method on `PgPoolAdapter`**

In `sql_pool_adapter.rs`, inside `impl SqlConnectionPort for PgPoolAdapter`, after the `create_schema` method (after line 558), add:

```rust
    async fn execute_setup_sql(&self, sql: &str) -> Result<(), SqlNodeError> {
        let mut tx = self.pool.begin().await.map_err(|e| {
            SqlNodeError::ExecutionError(format!("setup_sql: failed to begin transaction: {}", e))
        })?;
        // `raw_sql` uses the simple query protocol, which permits multiple
        // statements separated by `;` in one round-trip. Running it inside the
        // transaction makes the whole block atomic (Postgres has transactional DDL).
        sqlx::raw_sql(sql).execute(&mut *tx).await.map_err(|e| {
            SqlNodeError::ExecutionError(format!("setup_sql execution failed: {}", e))
        })?;
        tx.commit().await.map_err(|e| {
            SqlNodeError::ExecutionError(format!("setup_sql: failed to commit: {}", e))
        })?;
        Ok(())
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `set -a && source .env && set +a && TEST_DATABASE_URL=$DATABASE_URL cargo test --lib setup_sql_ -- --ignored --nocapture`
Expected: PASS — `setup_sql_runs_and_is_idempotent ... ok` and `setup_sql_rolls_back_on_failure ... ok`.

- [ ] **Step 6: Verify no warnings**

Run: `cargo clippy --lib 2>&1 | tail -5`
Expected: no warnings/errors (deny-warnings is on).

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/sql_ports.rs \
        src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs
git commit -m "feat(sql): add operator-level execute_setup_sql to SqlConnectionPort"
```

---

## Task 2: Wire `setup_sql` into `SqlNode` init

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs` (init ~line 123; new test module at end of file)

- [ ] **Step 1: Write the failing tests**

`sql.rs` currently has no test module. Add a new one at the END of the file:

```rust
#[cfg(test)]
mod setup_sql_tests {
    use super::*;
    use crate::dag_engine::infrastructure::pool_registry::{PgPoolRegistry, PoolConfig};
    use crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;
    use serde_json::json;

    fn unique(prefix: &str) -> String {
        format!(
            "{}_{}",
            prefix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    fn fresh_node() -> SqlNode {
        let registry = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
        let factory = Arc::new(SqlPortFactory::new(registry));
        SqlNode::new(factory)
    }

    async fn raw_pool() -> sqlx::PgPool {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap()
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn setup_sql_runs_at_init_and_table_is_introspected() {
        if std::env::var("TEST_DATABASE_URL").is_err() {
            eprintln!("skip: TEST_DATABASE_URL not set");
            return;
        }
        let schema = unique("colmena_node_setup");
        let setup = format!(
            "CREATE SCHEMA IF NOT EXISTS {s};\n\
             CREATE TABLE IF NOT EXISTS {s}.gastos (id SERIAL PRIMARY KEY, nombre TEXT UNIQUE NOT NULL);\n\
             INSERT INTO {s}.gastos (nombre) VALUES ('a'),('b') ON CONFLICT (nombre) DO NOTHING;",
            s = schema
        );
        // `${TEST_DATABASE_URL}` is resolved by SqlNode::resolve_env_vars.
        let config = json!({
            "connection_url": "${TEST_DATABASE_URL}",
            "permissions": { "preset": "read_write", "allowed_schemas": [schema] },
            "setup_sql": setup,
        });

        // Init runs setup_sql, then introspects — so the table shows in the supplement.
        let ctx = fresh_node().initialize(&config).await.expect("init with setup_sql");
        let supplement = ctx.description_supplement.unwrap_or_default();
        assert!(
            supplement.contains("gastos"),
            "description supplement should list the setup-created table:\n{}",
            supplement
        );

        // A second, independent node re-runs setup: seed must stay at 2 rows (idempotent).
        fresh_node().initialize(&config).await.expect("second init is a no-op");

        let pool = raw_pool().await;
        let count: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {}.gastos", schema))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 2, "seed must be idempotent across inits");

        sqlx::query(&format!("DROP SCHEMA {} CASCADE", schema)).execute(&pool).await.ok();
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn bad_setup_sql_hard_fails_init_and_rolls_back() {
        if std::env::var("TEST_DATABASE_URL").is_err() {
            eprintln!("skip: TEST_DATABASE_URL not set");
            return;
        }
        let schema = unique("colmena_node_setup_fail");
        // allowed_schemas is empty so schema provisioning does NOT pre-create the schema;
        // setup_sql creates it and then fails, so rollback must leave it absent.
        let config = json!({
            "connection_url": "${TEST_DATABASE_URL}",
            "permissions": { "preset": "read_only", "allowed_schemas": [] },
            "setup_sql": format!("CREATE SCHEMA IF NOT EXISTS {s};\nTHIS IS NOT VALID SQL;", s = schema),
        });

        let res = fresh_node().initialize(&config).await;
        assert!(res.is_err(), "bad setup_sql must hard-fail init");
        let err = format!("{}", res.err().unwrap());
        assert!(err.contains("setup_sql"), "error must mention setup_sql, got: {}", err);

        let pool = raw_pool().await;
        let exists: Option<String> = sqlx::query_scalar(
            "SELECT schema_name FROM information_schema.schemata WHERE schema_name = $1",
        )
        .bind(&schema)
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert!(exists.is_none(), "failed setup_sql must roll back the schema");
        pool.close().await;
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `set -a && source .env && set +a && TEST_DATABASE_URL=$DATABASE_URL cargo test --lib setup_sql_runs_at_init -- --ignored --nocapture`
Expected: FAIL — `setup_sql_runs_at_init_and_table_is_introspected` fails the `assert!(supplement.contains("gastos"))` because `setup_sql` is not yet executed (the table is never created).

- [ ] **Step 3: Wire `setup_sql` into `do_initialize_inner`**

In `sql.rs`, in `do_initialize_inner`, immediately AFTER the schema-provisioning block that ends at line 123 (the closing `}` of `if permissions.create_schemas_if_missing()`) and BEFORE the registry adapter creation at line 125 (`let sandbox_schema = ...`), insert:

```rust
        // Operator-driven environment bootstrapping: run author-defined setup SQL
        // (DDL + seed). Operator trust-level — bypasses the LLM static validator,
        // same as schema provisioning above. Idempotency is the author's contract
        // (`CREATE ... IF NOT EXISTS`, `INSERT ... ON CONFLICT`). Atomic: any failure
        // rolls the whole block back and hard-fails init. Runs BEFORE metadata
        // introspection so the tool description reflects freshly-created tables.
        if let Some(setup_sql) = config.get("setup_sql").and_then(|v| v.as_str()) {
            let trimmed = setup_sql.trim();
            if !trimmed.is_empty() {
                println!("[SqlNode] running setup_sql ({} bytes)", trimmed.len());
                let conn: &dyn SqlConnectionPort = adapter.as_ref();
                conn.execute_setup_sql(trimmed)
                    .await
                    .map_err(|e| format!("Failed to run setup_sql: {}", e))?;
            }
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `set -a && source .env && set +a && TEST_DATABASE_URL=$DATABASE_URL cargo test --lib setup_sql_ -- --ignored --nocapture`
Expected: PASS — both `setup_sql_runs_at_init_and_table_is_introspected` and `bad_setup_sql_hard_fails_init_and_rolls_back` (plus Task 1's adapter tests) pass.

- [ ] **Step 5: Regression — confirm graphs without `setup_sql` still init**

Run: `set -a && source .env && set +a && cargo test --lib sql -- --nocapture 2>&1 | tail -15`
Expected: existing non-ignored SQL unit tests pass; absence of `setup_sql` is a no-op (the `if let Some(...)` is simply skipped).

- [ ] **Step 6: Verify no warnings**

Run: `cargo clippy --lib 2>&1 | tail -5`
Expected: no warnings/errors.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs
git commit -m "feat(sql): run author setup_sql at node init (idempotent env bootstrap)"
```

---

## Task 3: Documentation

**Files:**
- Modify: `docs/node_configurations.json` (`sql_query` entry)
- Modify: `docs/developer_guide/23_sql_node.md`

- [ ] **Step 1: Add `setup_sql` to the canonical schema**

Open `docs/node_configurations.json`, find the `sql_query` node entry, and add a `setup_sql` field to its config fields, alongside `connection_url`. Use this description text:

```json
"setup_sql": {
  "type": "string",
  "required": false,
  "default": null,
  "description": "Operator-authored DDL + seed run once at node init to bootstrap the database environment (schema, tables, seed rows). Operator trust-level: bypasses the LLM validator. Runs idempotently on every init — MUST be written idempotent (CREATE ... IF NOT EXISTS, INSERT ... ON CONFLICT DO NOTHING). Executed as one atomic transaction; any failure rolls back and hard-fails init. The LLM never sees it."
}
```

(Match the surrounding JSON formatting/indentation exactly. Keep the file valid JSON.)

- [ ] **Step 2: Verify the JSON is still valid**

Run: `python3 -c "import json; json.load(open('docs/node_configurations.json')); print('valid')"`
Expected: `valid`

- [ ] **Step 3: Add a guide section to `23_sql_node.md`**

Append a new section after the "Configuration Reference" table area (before "Input Ports" is fine, or as a standalone top-level section after "Initialization and Schema Introspection"). Use this content:

````markdown
## Bootstrapping de entorno con `setup_sql`

`setup_sql` permite que el **autor** del grafo adjunte el DDL + datos seed que el
agente necesita, de modo que un grafo publicado **auto-provisiona su entorno** la
primera vez que se usa. Es ideal para grafos plantilla (ej. un agente de finanzas
que crea tickets de gastos): el consumidor lo usa directamente, sin configurar nada.

### Semántica

| Aspecto | Comportamiento |
|---|---|
| **Cuándo corre** | En el init del nodo (lazy: el primer uso de la DB en cada run). Después de provisionar `allowed_schemas` y **antes** de la introspección, así la descripción de la tool ya lista las tablas creadas. |
| **Nivel de confianza** | Operador — **no** pasa por el validador del LLM. Permite DDL (CREATE TABLE/SCHEMA) que el `query` del LLM tiene bloqueado. El LLM nunca ve `setup_sql`. |
| **Idempotencia** | Corre en cada init. **Debés escribirlo idempotente.** La idempotencia la garantizan tus cláusulas SQL, no un flag de estado. |
| **Atomicidad** | Una transacción; si cualquier statement falla, rollback completo y el init **hard-failea** con `Failed to run setup_sql: ...`. |
| **Aislamiento** | Agnóstico. El destino (otra DB, otro schema, multi-tenant RLS, compartido) sale de cómo configures `connection_url`/`allowed_schemas`/`auto_rls`. El mismo `setup_sql` sirve a los 4 modelos. |

### Contrato de idempotencia

| Operación | Forma idempotente |
|---|---|
| Schema | `CREATE SCHEMA IF NOT EXISTS finanzas;` |
| Tabla | `CREATE TABLE IF NOT EXISTS finanzas.gastos (...);` |
| Columna nueva | `ALTER TABLE ... ADD COLUMN IF NOT EXISTS ...;` |
| Datos seed | `INSERT INTO ... VALUES (...) ON CONFLICT (col) DO NOTHING;` (requiere un `UNIQUE`) |
| Índice | `CREATE INDEX IF NOT EXISTS ...;` |

Un `INSERT` plano sin `ON CONFLICT` **se duplica en cada mensaje** — siempre usá `ON CONFLICT`.

### Ejemplo (tool de finanzas)

```json
"tool_configurations": {
  "gastos_db": {
    "name": "gastos_db",
    "node_type": "sql_query",
    "description": "Gestiona los gastos del usuario.",
    "node_schema": {
      "connection_url": { "type": "string", "fixed": "${DATABASE_URL}" },
      "permissions": {
        "type": "object",
        "fixed": { "preset": "read_write", "allowed_schemas": ["finanzas"] }
      },
      "setup_sql": {
        "type": "string",
        "fixed": "CREATE SCHEMA IF NOT EXISTS finanzas;\nCREATE TABLE IF NOT EXISTS finanzas.categorias (id SERIAL PRIMARY KEY, nombre TEXT UNIQUE NOT NULL);\nCREATE TABLE IF NOT EXISTS finanzas.gastos (id SERIAL PRIMARY KEY, categoria_id INT REFERENCES finanzas.categorias(id), monto NUMERIC(12,2), fecha DATE DEFAULT CURRENT_DATE, descripcion TEXT);\nINSERT INTO finanzas.categorias (nombre) VALUES ('Comida'),('Transporte'),('Hospedaje') ON CONFLICT (nombre) DO NOTHING;"
      },
      "query": { "type": "string", "required": true, "description": "SQL para gestionar gastos." }
    }
  }
}
```

`setup_sql` va `fixed` → el LLM solo ve `query`.

### Limitaciones (v1)

- **No hay guard "run-once".** Corre idempotente en cada init; no hay tabla de tracking. Para setups pesados con seed no idempotente, ver BACKLOG.
- **No hay lint de idempotencia.** El motor confía en que el SQL es idempotente.
- **1 DB = 1 tool con setup** es el patrón esperado. Varios nodos `sql_query` a la misma DB corren cada uno su propio `setup_sql` (seguro por idempotencia, pero redundante).
- **Aislamiento per-usuario** (otra DB / otro schema por usuario) requiere que el host (ADP) instancie el grafo fresco por run — que es como corre hoy.
````

- [ ] **Step 4: Add a BACKLOG entry for the deferred run-once guard**

Open `docs/BACKLOG.md` and add this item under an appropriate section (e.g. SQL node):

```markdown
- **`setup_sql` run-once guard (Fase 2)** — `setup_sql` corre idempotente en cada init.
  Para setups pesados con seed no idempotente, agregar un opt-in `run_once: true` + tabla
  de tracking keyed por `hash(connection_url + schema + versión)`. También: lint de
  idempotencia (warn si `INSERT` sin `ON CONFLICT` / `CREATE` sin `IF NOT EXISTS`) y
  versionado de schema entre versiones del grafo. Ver
  `docs/superpowers/specs/2026-06-21-sql-setup-block-design.md` §6.
```

- [ ] **Step 5: Commit**

```bash
git add docs/node_configurations.json docs/developer_guide/23_sql_node.md docs/BACKLOG.md
git commit -m "docs(sql): document setup_sql environment bootstrapping"
```

---

## Task 4: End-to-end finance agent graph

**Files:**
- Create: `tests/graphs/agents/sql_setup_finanzas.json`

- [ ] **Step 1: Create the E2E graph**

Create `tests/graphs/agents/sql_setup_finanzas.json` with this content:

```json
{
  "comment": "E2E: finance agent whose sql_query tool bootstraps its schema via setup_sql",
  "metadata": {
    "category": "agents",
    "requires_env": ["GEMINI_API_KEY", "DATABASE_URL"]
  },
  "nodes": {
    "finanzas_agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "stream": false,
        "system_message": "Eres un asistente de finanzas personales. Usás la tool gastos_db para registrar y consultar gastos. El esquema 'finanzas' con las tablas 'categorias' y 'gastos' ya existe. Para registrar un gasto, buscá el id de la categoría por nombre e insertá en finanzas.gastos. Respondé en español.",
        "tool_configurations": {
          "gastos_db": {
            "name": "gastos_db",
            "node_type": "sql_query",
            "description": "Registra y consulta gastos del usuario en la base finanzas.",
            "node_schema": {
              "connection_url": { "type": "string", "fixed": "${DATABASE_URL}" },
              "permissions": {
                "type": "object",
                "fixed": { "preset": "read_write", "allowed_schemas": ["finanzas"] }
              },
              "setup_sql": {
                "type": "string",
                "fixed": "CREATE SCHEMA IF NOT EXISTS finanzas;\nCREATE TABLE IF NOT EXISTS finanzas.categorias (id SERIAL PRIMARY KEY, nombre TEXT UNIQUE NOT NULL);\nCREATE TABLE IF NOT EXISTS finanzas.gastos (id SERIAL PRIMARY KEY, categoria_id INT REFERENCES finanzas.categorias(id), monto NUMERIC(12,2), fecha DATE DEFAULT CURRENT_DATE, descripcion TEXT);\nINSERT INTO finanzas.categorias (nombre) VALUES ('Comida'),('Transporte'),('Hospedaje') ON CONFLICT (nombre) DO NOTHING;"
              },
              "query": {
                "type": "string",
                "required": true,
                "description": "SQL para registrar o consultar gastos (SELECT/INSERT/UPDATE)."
              }
            }
          }
        },
        "prompt": "Registrá un gasto de comida de $20 de hoy con descripción 'almuerzo', y después decime cuánto llevo gastado en total."
      }
    },
    "result": { "type": "output", "config": { "label": "Finanzas Agent Result" } }
  },
  "edges": [
    { "from": "finanzas_agent", "to": "result" }
  ]
}
```

- [ ] **Step 2: Run it against real Gemini + Postgres and capture SSE**

```bash
mkdir -p /tmp/colmena_e2e
set -a && source .env && set +a
cargo run --bin dag_engine -- run tests/graphs/agents/sql_setup_finanzas.json \
  --agent-session-id agent_sql_setup_001 2>&1 | tee /tmp/colmena_e2e/sql_setup_finanzas.sse
```
Expected: the agent calls `gastos_db` at least twice (insert + total). No `Failed to run setup_sql`. Final answer states a total of `20` (or `20.00`).

- [ ] **Step 3: Verify the row landed in Postgres**

```bash
set -a && source .env && set +a
psql "$DATABASE_URL" -c "SELECT g.monto, g.descripcion, c.nombre FROM finanzas.gastos g JOIN finanzas.categorias c ON c.id = g.categoria_id ORDER BY g.id DESC LIMIT 3;"
```
Expected: a row with `monto = 20.00`, `descripcion = almuerzo`, `nombre = Comida`.

- [ ] **Step 4: Run twice more to prove idempotent setup (no dup categories)**

```bash
set -a && source .env && set +a
cargo run --bin dag_engine -- run tests/graphs/agents/sql_setup_finanzas.json --agent-session-id agent_sql_setup_002 >/dev/null 2>&1
psql "$DATABASE_URL" -c "SELECT count(*) AS categorias FROM finanzas.categorias;"
```
Expected: `categorias = 3` (seed never duplicates, regardless of how many runs hit init).

- [ ] **Step 5: Write a friendly E2E report**

Summarize for the user (do NOT paste the whole SSE): input prompt, the tool calls the agent made, the inserted row, total tokens, and the idempotency check (3 categorías after N runs). Note where the SSE is saved (`/tmp/colmena_e2e/sql_setup_finanzas.sse`).

- [ ] **Step 6: Commit**

```bash
git add tests/graphs/agents/sql_setup_finanzas.json
git commit -m "test(sql): E2E finance agent with setup_sql env bootstrap"
```

---

## Final verification (before declaring done)

- [ ] **Full test sweep (catches doctest/integration failures CI would catch)**

Run: `set -a && source .env && set +a && TEST_DATABASE_URL=$DATABASE_URL cargo test --verbose -- --include-ignored 2>&1 | tail -30`
Expected: all pass, including the four new `setup_sql_*` tests.

- [ ] **Clippy clean**

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: no warnings (deny-warnings).

- [ ] **Format**

Run: `cargo fmt`

- [ ] **ADP breaking-change sweep** — `setup_sql` is a new optional field; `execute_setup_sql` is added to an internal trait with a single in-repo impl and no external implementors. Confirm no ADP worker code implements `SqlConnectionPort` (it does not). No action expected, but state this explicitly in the final report.

---

## Notes / out of scope (BACKLOG, do NOT build)

- `run_once: true` guard + tracking table for heavy non-idempotent seed.
- Idempotency lint/warning on `setup_sql`.
- Evolutionary schema versioning/migrations across graph versions.
- `statement_timeout` applied to the setup transaction (v1 runs without `SET LOCAL`).
