# Enriched schema/capability context + `read_write_delete` preset — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `read_write_delete` permission preset (SELECT/INSERT/UPDATE/DELETE + `ALTER TABLE ADD COLUMN` only), let `full` also add columns, and enrich the `sql_query` tool description with the full table schema (columns + types + PK/FK/UNIQUE) and a natural-language capability statement — so the agent understands its data model and limits without introspecting or hitting blocks.

**Architecture:** Hexagonal. `SqlOperation`/preset logic lives in `domain/sql_permissions.rs`; statement classification in `infrastructure/sql_ast.rs`; the validator in `infrastructure/sql_static_validator.rs` is data-driven (classify → preset check), so allowing ADD COLUMN needs no validator branch beyond what classification provides. Introspection is a new port method on `SqlConnectionPort` implemented by `PgPoolAdapter`. The tool-description text is assembled in `SqlNode::build_description_supplement` at node init.

**Tech Stack:** Rust, `sqlparser` (AST), `sqlx` 0.8 (Postgres). Crate: `colmena_dag_engine`.

---

## Background for the implementer (read first)

- The `sql_query` node is used as an LLM tool. At init (`SqlNode::do_initialize_inner` in `nodes/sql.rs`), it introspects `allowed_schemas` and builds a **description supplement** appended to the tool's description so the LLM knows what's available.
- **Permission model:** a `preset` (`read_only`/`read_write`/`full`) maps to a set of `SqlOperation`. The static validator parses the query to an AST, `classify`-ies each statement to a `SqlOperation`, then checks it against the preset. Operations not in the preset are blocked. `Truncate`/`Drop`/`Alter` are also hard-blocked explicitly.
- **What we're adding:** (1) `SqlOperation::AddColumn` representing `ALTER TABLE … ADD COLUMN` *and only that variant*; (2) preset `read_write_delete`; (3) `full` gains `AddColumn`; (4) richer introspection (columns + keys); (5) an NL capability statement; (6) the supplement renders all of it with a size cap.
- **Crate name is `colmena_dag_engine`.** Tests: `cargo test --lib`. Integration tests that need Postgres are `#[ignore]`-gated and read `TEST_DATABASE_URL`; run locally with the repo `.env` sourced (`TEST_DATABASE_URL=$DATABASE_URL`).
- **Deny-warnings is ON** (`Cargo.toml [lints.rust] warnings = "deny"`). No unused imports/dead code. Use `cargo clippy --all-targets` (not `--lib`) — `--lib` does not lint test modules.
- This branch is based on `origin/develop` and does NOT contain the `setup_sql` feature (separate PR). Do not depend on it.

### Files touched

| File | Change |
|---|---|
| `src/libs/colmena/src/dag_engine/domain/sql_permissions.rs` | `AddColumn` op, `read_write_delete` preset, `full`+AddColumn, deny `add_column`, new `describe_capabilities_nl` |
| `src/libs/colmena/src/dag_engine/infrastructure/sql_ast.rs` | `classify` ALTER→AddColumn (iff all ops are AddColumn); `referenced_schemas` handles `AlterTable` |
| `src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs` | tests only (behavior is data-driven) |
| `src/libs/colmena/src/dag_engine/domain/sql_ports.rs` | `ColumnInfo`/`ForeignKey`/`TableSchema` types + `load_table_schemas` trait method |
| `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs` | impl `load_table_schemas` |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs` | `build_description_supplement` rewrite + call site switch + cap constants |
| `docs/developer_guide/23_sql_node.md`, `docs/node_configurations.json` | docs |
| `tests/graphs/agents/sql_read_write_delete_e2e.json` | E2E graph (create) |

---

## Task 1: `AddColumn` operation + `read_write_delete` preset

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/sql_permissions.rs`

- [ ] **Step 1: Write the failing tests** — add to the `#[cfg(test)] mod tests` block in `sql_permissions.rs`:

```rust
    #[test]
    fn test_read_write_delete_preset() {
        let perms = SqlPermissions::from_config(Some(&serde_json::json!({
            "preset": "read_write_delete"
        })))
        .unwrap();
        assert!(perms.is_allowed(&SqlOperation::Select));
        assert!(perms.is_allowed(&SqlOperation::Insert));
        assert!(perms.is_allowed(&SqlOperation::Update));
        assert!(perms.is_allowed(&SqlOperation::Delete));
        assert!(perms.is_allowed(&SqlOperation::AddColumn));
        assert!(!perms.is_allowed(&SqlOperation::CreateTable));
        assert!(!perms.is_allowed(&SqlOperation::CreateFunction));
    }

    #[test]
    fn test_full_preset_allows_add_column() {
        let perms = SqlPermissions::from_config(Some(&serde_json::json!({ "preset": "full" }))).unwrap();
        assert!(perms.is_allowed(&SqlOperation::AddColumn));
        assert!(perms.is_allowed(&SqlOperation::CreateTable));
    }

    #[test]
    fn test_deny_add_column() {
        let perms = SqlPermissions::from_config(Some(&serde_json::json!({
            "preset": "read_write_delete",
            "deny": ["add_column"]
        })))
        .unwrap();
        assert!(perms.is_allowed(&SqlOperation::Delete));
        assert!(!perms.is_allowed(&SqlOperation::AddColumn));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib test_read_write_delete_preset test_full_preset_allows_add_column 2>&1 | tail -20`
(run each name separately if cargo rejects two positionals: `cargo test --lib test_read_write_delete_preset`)
Expected: COMPILE ERROR — `no variant named AddColumn`.

- [ ] **Step 3: Add the `AddColumn` variant.** In the `SqlOperation` enum (after `CreateTable`):

```rust
    /// `ALTER TABLE … ADD COLUMN` — and ONLY that ALTER variant. Allowed in
    /// `read_write_delete` and `full`. Destructive ALTER (DROP/RENAME/type change)
    /// is classified as `Alter` and stays blocked.
    AddColumn,
```

- [ ] **Step 4: Map `add_column` in `from_str_loose`.** Add this arm (before `_ => None`):

```rust
            "add_column" => Some(Self::AddColumn),
```

- [ ] **Step 5: Add the `ReadWriteDelete` preset variant.** In `enum PermissionPreset` (after `ReadWrite`):

```rust
    ReadWriteDelete,
```

In `PermissionPreset::from_str`, add (before `other =>`):

```rust
            "read_write_delete" => Ok(Self::ReadWriteDelete),
```

In `allowed_operations`, add the `ReadWriteDelete` arm and extend `Full`:

```rust
            Self::ReadWriteDelete => {
                let mut set = HashSet::new();
                set.insert(SqlOperation::Select);
                set.insert(SqlOperation::Insert);
                set.insert(SqlOperation::Update);
                set.insert(SqlOperation::Delete);
                set.insert(SqlOperation::AddColumn);
                set
            }
```

And inside the existing `Self::Full => { ... }` arm, add one line after `set.insert(SqlOperation::CreateTable);`:

```rust
                set.insert(SqlOperation::AddColumn);
```

- [ ] **Step 6: Run to verify pass**

Run: `cargo test --lib test_read_write_delete_preset; cargo test --lib test_full_preset_allows_add_column; cargo test --lib test_deny_add_column`
Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/sql_permissions.rs
git commit -m "feat(sql): add read_write_delete preset + AddColumn operation"
```

---

## Task 2: Classify `ALTER … ADD COLUMN` + schema extraction

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/sql_ast.rs`

- [ ] **Step 1: Write the failing tests** — add to the `#[cfg(test)] mod tests` block in `sql_ast.rs`:

```rust
    #[test]
    fn classify_add_column_only() {
        let stmts = parse("ALTER TABLE finanzas.gastos ADD COLUMN metodo_pago TEXT").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::AddColumn));
    }

    #[test]
    fn classify_alter_drop_column_is_blocked_alter() {
        let stmts = parse("ALTER TABLE finanzas.gastos DROP COLUMN monto").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Alter));
    }

    #[test]
    fn classify_alter_mixed_ops_is_blocked_alter() {
        let stmts =
            parse("ALTER TABLE finanzas.gastos ADD COLUMN a INT, DROP COLUMN b").unwrap();
        assert_eq!(classify(&stmts[0]), Some(SqlOperation::Alter));
    }

    #[test]
    fn referenced_schemas_includes_alter_target() {
        let stmts = parse("ALTER TABLE finanzas.gastos ADD COLUMN x INT").unwrap();
        assert!(referenced_schemas(&stmts[0]).contains(&"finanzas".to_string()));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib classify_add_column_only classify_alter_drop_column_is_blocked_alter 2>&1 | tail -20`
Expected: FAIL — `classify_add_column_only` returns `Some(Alter)` not `Some(AddColumn)`; `referenced_schemas_includes_alter_target` fails (ALTER target not captured).

- [ ] **Step 3: Update `classify` for `AlterTable`.** Add the import at the top of `sql_ast.rs` (with the other `sqlparser::ast` uses):

```rust
use sqlparser::ast::AlterTableOperation;
```

Replace the existing alter arm in `classify` (currently):

```rust
        Statement::AlterTable { .. }
        | Statement::AlterIndex { .. }
        | Statement::AlterView { .. } => Some(SqlOperation::Alter),
```

with:

```rust
        Statement::AlterTable { operations, .. } => {
            // Allow ONLY when every operation is ADD COLUMN. Any destructive or
            // mixed operation (DROP COLUMN, type change, RENAME, …) classifies as
            // `Alter`, which the validator hard-blocks for all presets.
            if !operations.is_empty()
                && operations
                    .iter()
                    .all(|op| matches!(op, AlterTableOperation::AddColumn { .. }))
            {
                Some(SqlOperation::AddColumn)
            } else {
                Some(SqlOperation::Alter)
            }
        }
        Statement::AlterIndex { .. } | Statement::AlterView { .. } => Some(SqlOperation::Alter),
```

- [ ] **Step 4: Make `referenced_schemas` capture the `ALTER TABLE` target.** In `referenced_schemas`, after the existing `Statement::Comment` block and before the `visit_relations` call, add:

```rust
    if let Statement::AlterTable { name, .. } = stmt {
        if name.0.len() >= 2 {
            if let Some(ident) = name.0[0].as_ident() {
                let schema = ident.value.to_lowercase();
                if !found.contains(&schema) {
                    found.push(schema);
                }
            }
        }
    }
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test --lib classify_add_column_only; cargo test --lib classify_alter_drop_column_is_blocked_alter; cargo test --lib classify_alter_mixed_ops_is_blocked_alter; cargo test --lib referenced_schemas_includes_alter_target`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/sql_ast.rs
git commit -m "feat(sql): classify ALTER ADD COLUMN as AddColumn; capture ALTER target schema"
```

---

## Task 3: Validator behavior tests (no src change)

The validator is data-driven: `AddColumn` is not in the always-blocked match, so a classified `AddColumn` falls through to the preset check (`is_allowed`). This task proves the end-to-end behavior.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs` (tests only)

- [ ] **Step 1: Write the tests** — add to the `#[cfg(test)] mod tests` block. Add a helper next to `full_perms()`:

```rust
    fn rwd_perms() -> SqlPermissions {
        SqlPermissions::from_config(Some(&serde_json::json!({
            "preset": "read_write_delete",
            "allowed_schemas": ["production"]
        })))
        .unwrap()
    }

    #[test]
    fn test_add_column_allowed_rwd() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "ALTER TABLE production.users ADD COLUMN nickname TEXT",
            &rwd_perms(),
        );
        assert!(r.allowed, "ADD COLUMN must be allowed under read_write_delete");
    }

    #[test]
    fn test_add_column_allowed_full() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "ALTER TABLE production.users ADD COLUMN nickname TEXT",
            &full_perms(),
        );
        assert!(r.allowed);
    }

    #[test]
    fn test_add_column_blocked_read_only() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "ALTER TABLE production.users ADD COLUMN nickname TEXT",
            &read_only_perms(),
        );
        assert!(!r.allowed, "ADD COLUMN must be blocked under read_only");
    }

    #[test]
    fn test_drop_column_blocked_even_full() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "ALTER TABLE production.users DROP COLUMN email",
            &full_perms(),
        );
        assert!(!r.allowed, "destructive ALTER must stay blocked even with full");
        assert!(r.block_reason.unwrap().contains("ALTER"));
    }

    #[test]
    fn test_add_column_on_disallowed_schema_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "ALTER TABLE secret.users ADD COLUMN x TEXT",
            &rwd_perms(), // allowed_schemas = ["production"]
        );
        assert!(!r.allowed, "ADD COLUMN must respect the schema allowlist");
        assert!(r.block_reason.unwrap().to_lowercase().contains("schema"));
    }
```

- [ ] **Step 2: Run the tests**

Run each test by name, e.g.: `cargo test --lib test_add_column_allowed_rwd`, then `test_add_column_allowed_full`, `test_add_column_blocked_read_only`, `test_drop_column_blocked_even_full`, `test_add_column_on_disallowed_schema_blocked`.
Expected: all five PASS. If `test_add_column_on_disallowed_schema_blocked` FAILS, it means Task 2 Step 4 (referenced_schemas for ALTER) is missing/incorrect — fix there.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs
git commit -m "test(sql): ADD COLUMN allowed for rwd/full, destructive ALTER and schema allowlist enforced"
```

---

## Task 4: Schema introspection (`load_table_schemas`)

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/sql_ports.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs`

- [ ] **Step 1: Add the domain types + trait method.** In `sql_ports.rs`, after the existing `TableInfo` struct, add:

```rust
/// A column within a table, for LLM schema context.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub not_null: bool,
    pub is_pk: bool,
    pub is_unique: bool,
}

/// A single-column foreign key (the common case for agent context).
#[derive(Debug, Clone)]
pub struct ForeignKey {
    pub column: String,
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_column: String,
}

/// Full schema of a table: columns + keys, for LLM context injection.
#[derive(Debug, Clone)]
pub struct TableSchema {
    pub schema_name: String,
    pub table_name: String,
    pub description: Option<String>,
    pub columns: Vec<ColumnInfo>,
    pub foreign_keys: Vec<ForeignKey>,
}
```

In the `SqlConnectionPort` trait, after `load_table_metadata`, add:

```rust
    /// Load full schema (columns + types + PK/UNIQUE/NOT NULL + FKs) for the
    /// given schemas, for injecting into the LLM tool description.
    async fn load_table_schemas(&self, schemas: &[String])
        -> Result<Vec<TableSchema>, SqlNodeError>;
```

- [ ] **Step 2: Write the failing integration test.** In the `#[cfg(test)] mod tests` of `sql_pool_adapter.rs` (it has a `test_adapter()` helper and a `unique_schema` pattern), add:

```rust
    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn load_table_schemas_returns_columns_pk_unique_fk() {
        let Some(adapter) = test_adapter().await else {
            eprintln!("skip: TEST_DATABASE_URL not set");
            return;
        };
        let schema = format!(
            "colmena_sch_{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        );
        for sql in [
            format!("CREATE SCHEMA IF NOT EXISTS {s}", s = schema),
            format!("CREATE TABLE {s}.cat (id SERIAL PRIMARY KEY, nombre TEXT UNIQUE NOT NULL)", s = schema),
            format!("CREATE TABLE {s}.item (id SERIAL PRIMARY KEY, cat_id INT REFERENCES {s}.cat(id), label TEXT)", s = schema),
        ] {
            sqlx::query(&sql).execute(&*adapter.pool()).await.unwrap();
        }

        let schemas = adapter.load_table_schemas(&[schema.clone()]).await.unwrap();
        let item = schemas.iter().find(|t| t.table_name == "item").expect("item table");
        // columns present with types
        assert!(item.columns.iter().any(|c| c.name == "label" && c.data_type.contains("text")));
        let id = item.columns.iter().find(|c| c.name == "id").unwrap();
        assert!(id.is_pk, "id should be PK");
        let cat = schemas.iter().find(|t| t.table_name == "cat").unwrap();
        let nombre = cat.columns.iter().find(|c| c.name == "nombre").unwrap();
        assert!(nombre.not_null && nombre.is_unique, "nombre should be NOT NULL + UNIQUE");
        // FK present
        assert!(item.foreign_keys.iter().any(|fk|
            fk.column == "cat_id" && fk.ref_table == "cat" && fk.ref_column == "id"));

        sqlx::query(&format!("DROP SCHEMA {} CASCADE", schema)).execute(&*adapter.pool()).await.ok();
    }
```

- [ ] **Step 3: Run to verify failure**

Run: `set -a && source .env && set +a && TEST_DATABASE_URL=$DATABASE_URL cargo test --lib load_table_schemas_returns_columns_pk_unique_fk -- --ignored 2>&1 | tail -20`
Expected: COMPILE ERROR — `no method named load_table_schemas`.

- [ ] **Step 4: Implement `load_table_schemas` on `PgPoolAdapter`.** Add inside `impl SqlConnectionPort for PgPoolAdapter`, after `load_table_metadata`. Also add `TableSchema, ColumnInfo, ForeignKey` to the `use crate::dag_engine::domain::sql_ports::{...}` import at the top of the file.

```rust
    async fn load_table_schemas(
        &self,
        schemas: &[String],
    ) -> Result<Vec<TableSchema>, SqlNodeError> {
        let pool = &*self.pool;
        if schemas.is_empty() {
            return Ok(vec![]);
        }

        // 1. Tables + comments (reuse the same shape as load_table_metadata).
        let base = self.load_table_metadata(schemas).await?;

        // 2. Columns with NOT NULL + PK + UNIQUE flags.
        let col_rows = sqlx::query(
            "SELECT c.table_schema, c.table_name, c.column_name, c.data_type, \
                    (c.is_nullable = 'NO') AS not_null, \
                    COALESCE(pk.is_pk, false) AS is_pk, \
                    COALESCE(uq.is_unique, false) AS is_unique \
             FROM information_schema.columns c \
             LEFT JOIN ( \
               SELECT tc.table_schema, tc.table_name, kcu.column_name, true AS is_pk \
               FROM information_schema.table_constraints tc \
               JOIN information_schema.key_column_usage kcu \
                 ON kcu.constraint_name = tc.constraint_name \
                AND kcu.constraint_schema = tc.constraint_schema \
               WHERE tc.constraint_type = 'PRIMARY KEY' \
             ) pk ON pk.table_schema=c.table_schema AND pk.table_name=c.table_name AND pk.column_name=c.column_name \
             LEFT JOIN ( \
               SELECT tc.table_schema, tc.table_name, kcu.column_name, true AS is_unique \
               FROM information_schema.table_constraints tc \
               JOIN information_schema.key_column_usage kcu \
                 ON kcu.constraint_name = tc.constraint_name \
                AND kcu.constraint_schema = tc.constraint_schema \
               WHERE tc.constraint_type = 'UNIQUE' \
             ) uq ON uq.table_schema=c.table_schema AND uq.table_name=c.table_name AND uq.column_name=c.column_name \
             WHERE c.table_schema = ANY($1) \
             ORDER BY c.table_schema, c.table_name, c.ordinal_position",
        )
        .bind(schemas)
        .fetch_all(pool)
        .await
        .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to load columns: {}", e)))?;

        // 3. Single-column foreign keys.
        let fk_rows = sqlx::query(
            "SELECT tc.table_schema AS local_schema, tc.table_name AS local_table, \
                    kcu.column_name AS local_column, \
                    ccu.table_schema AS ref_schema, ccu.table_name AS ref_table, \
                    ccu.column_name AS ref_column \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON kcu.constraint_name = tc.constraint_name \
              AND kcu.constraint_schema = tc.constraint_schema \
             JOIN information_schema.constraint_column_usage ccu \
               ON ccu.constraint_name = tc.constraint_name \
              AND ccu.constraint_schema = tc.constraint_schema \
             WHERE tc.constraint_type = 'FOREIGN KEY' AND tc.table_schema = ANY($1)",
        )
        .bind(schemas)
        .fetch_all(pool)
        .await
        .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to load foreign keys: {}", e)))?;

        // Assemble.
        let mut out: Vec<TableSchema> = base
            .into_iter()
            .map(|t| TableSchema {
                schema_name: t.schema_name,
                table_name: t.table_name,
                description: t.description,
                columns: vec![],
                foreign_keys: vec![],
            })
            .collect();

        let find = |out: &mut Vec<TableSchema>, s: &str, t: &str| -> Option<usize> {
            out.iter().position(|x| x.schema_name == s && x.table_name == t)
        };

        for row in &col_rows {
            let s: String = row.try_get("table_schema").unwrap_or_default();
            let t: String = row.try_get("table_name").unwrap_or_default();
            if let Some(i) = find(&mut out, &s, &t) {
                out[i].columns.push(ColumnInfo {
                    name: row.try_get("column_name").unwrap_or_default(),
                    data_type: row.try_get("data_type").unwrap_or_default(),
                    not_null: row.try_get("not_null").unwrap_or(false),
                    is_pk: row.try_get("is_pk").unwrap_or(false),
                    is_unique: row.try_get("is_unique").unwrap_or(false),
                });
            }
        }
        for row in &fk_rows {
            let s: String = row.try_get("local_schema").unwrap_or_default();
            let t: String = row.try_get("local_table").unwrap_or_default();
            if let Some(i) = find(&mut out, &s, &t) {
                out[i].foreign_keys.push(ForeignKey {
                    column: row.try_get("local_column").unwrap_or_default(),
                    ref_schema: row.try_get("ref_schema").unwrap_or_default(),
                    ref_table: row.try_get("ref_table").unwrap_or_default(),
                    ref_column: row.try_get("ref_column").unwrap_or_default(),
                });
            }
        }
        Ok(out)
    }
```

- [ ] **Step 5: Run to verify pass**

Run: `set -a && source .env && set +a && TEST_DATABASE_URL=$DATABASE_URL cargo test --lib load_table_schemas_returns_columns_pk_unique_fk -- --ignored --nocapture 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 6: clippy clean**

Run: `cargo clippy --all-targets 2>&1 | tail -3`
Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/sql_ports.rs \
        src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs
git commit -m "feat(sql): load_table_schemas introspection (columns + PK/UNIQUE/FK)"
```

---

## Task 5: Capability statement + enriched supplement render

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/sql_permissions.rs` (add `describe_capabilities_nl`)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs` (supplement + call site + cap)

- [ ] **Step 1: Write the failing tests** — add to `sql_permissions.rs` tests:

```rust
    #[test]
    fn test_capabilities_nl_read_write_delete() {
        let perms = SqlPermissions::from_config(Some(&serde_json::json!({
            "preset": "read_write_delete"
        })))
        .unwrap();
        let nl = perms.describe_capabilities_nl();
        assert!(nl.contains("SELECT") && nl.contains("INSERT") && nl.contains("UPDATE") && nl.contains("DELETE"));
        assert!(nl.to_lowercase().contains("agregar columnas"));
        assert!(nl.to_lowercase().contains("no") && nl.to_lowercase().contains("crear tablas"));
    }

    #[test]
    fn test_capabilities_nl_read_only_says_no_writes() {
        let perms = SqlPermissions::from_config(Some(&serde_json::json!({ "preset": "read_only" }))).unwrap();
        let nl = perms.describe_capabilities_nl();
        assert!(nl.contains("SELECT"));
        assert!(!nl.contains("INSERT"));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib test_capabilities_nl_read_write_delete 2>&1 | tail -10`
Expected: COMPILE ERROR — `no method named describe_capabilities_nl`.

- [ ] **Step 3: Implement `describe_capabilities_nl`.** Add to `impl SqlPermissions`, next to `describe_for_llm`:

```rust
    /// Natural-language statement of what the agent can and cannot do, derived
    /// from the actual allowed-operation set (so `deny` combos read correctly).
    pub fn describe_capabilities_nl(&self) -> String {
        let has = |op: SqlOperation| self.allowed_ops.contains(&op);
        let mut can: Vec<&str> = Vec::new();
        if has(SqlOperation::Select) { can.push("leer (SELECT)"); }
        if has(SqlOperation::Insert) { can.push("insertar filas (INSERT)"); }
        if has(SqlOperation::Update) { can.push("modificar filas (UPDATE)"); }
        if has(SqlOperation::Delete) { can.push("borrar filas (DELETE)"); }
        if has(SqlOperation::AddColumn) { can.push("agregar columnas a tablas existentes (ALTER TABLE ADD COLUMN)"); }
        if has(SqlOperation::CreateTable) || has(SqlOperation::CreateFunction) {
            can.push("crear tablas y funciones nuevas en el sandbox");
        }

        let mut cannot: Vec<&str> = Vec::new();
        if !has(SqlOperation::Delete) { cannot.push("borrar filas (DELETE)"); }
        if !has(SqlOperation::AddColumn) { cannot.push("agregar columnas"); }
        if !has(SqlOperation::CreateTable) { cannot.push("crear tablas nuevas"); }

        let can_str = if can.is_empty() { "nada".to_string() } else { can.join(", ") };
        let cannot_str = if cannot.is_empty() {
            String::new()
        } else {
            format!(" NO podés: {}.", cannot.join(", "))
        };
        format!(
            "Permisos del agente: podés {}.{} Restricciones permanentes: \
             DELETE/UPDATE requieren WHERE; CREATE SCHEMA, DROP, TRUNCATE y todo \
             ALTER que no sea ADD COLUMN están siempre bloqueados.",
            can_str, cannot_str
        )
    }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --lib test_capabilities_nl_read_write_delete; cargo test --lib test_capabilities_nl_read_only_says_no_writes`
Expected: PASS.

- [ ] **Step 5: Add cap constants + rewrite the supplement render.** In `nodes/sql.rs`, add near the top of the `impl SqlNode` block (or as module consts above it):

```rust
const MAX_SCHEMA_TABLES: usize = 40;
const MAX_SCHEMA_CHARS: usize = 8000;
```

Change the `build_description_supplement` signature from `tables: &[TableInfo]` to `tables: &[TableSchema]` (update the import: ensure `TableSchema` is in the `use ...sql_ports::{...}` line in `sql.rs`; `TableInfo` may still be needed elsewhere — keep both).

Replace the body **from the start of the function up to and including the permissions line** (the block that currently starts `let mut lines = Vec::new();` and ends at the `lines.push("Use introspection queries...")` line) with:

```rust
        let mut lines = Vec::new();

        // 1. Capability statement (always included — small).
        lines.push(permissions.describe_capabilities_nl());

        // 2. Schema render with graceful cap.
        if !tables.is_empty() {
            let rendered = Self::render_schema(tables);
            if tables.len() > MAX_SCHEMA_TABLES || rendered.len() > MAX_SCHEMA_CHARS {
                // Degrade: names only.
                lines.push(String::new());
                let mut current = String::new();
                for t in tables {
                    if t.schema_name != current {
                        current = t.schema_name.clone();
                        lines.push(format!("Tablas (schema: {}):", current));
                    }
                    lines.push(format!("  - {}", t.table_name));
                }
                lines.push(
                    "(Schema grande: usá introspección sobre information_schema \
                     para ver columnas.)".to_string(),
                );
            } else {
                lines.push(rendered);
            }
        }

        // 3. Functions (unchanged behavior).
        if !functions.is_empty() {
            lines.push(String::new());
            lines.push("Available functions (sandbox):".to_string());
            for func in functions {
                let params = func.parameters.as_deref().unwrap_or("");
                lines.push(format!("  - {}({}) -- {}", func.function_name, params, func.description));
            }
        }

        lines.push(String::new());
        lines.push(format!("Max rows: {}", max_rows));
```

Leave the trailing multi-statement/anti-pattern block (everything after the old `"Use introspection queries..."` line) exactly as-is.

Then add a helper `render_schema` in the same `impl SqlNode`:

```rust
    /// Render tables with columns + keys for the tool description.
    fn render_schema(tables: &[crate::dag_engine::domain::sql_ports::TableSchema]) -> String {
        let mut out = String::new();
        let mut current = String::new();
        for t in tables {
            if t.schema_name != current {
                current = t.schema_name.clone();
                out.push_str(&format!("\nEsquema disponible (schema: {}):\n", current));
            }
            let pk: Vec<&str> = t.columns.iter().filter(|c| c.is_pk).map(|c| c.name.as_str()).collect();
            let pk_str = if pk.is_empty() { String::new() } else { format!("  [PK: {}]", pk.join(", ")) };
            match &t.description {
                Some(d) => out.push_str(&format!("  • {}{}  -- {}\n", t.table_name, pk_str, d)),
                None => out.push_str(&format!("  • {}{}\n", t.table_name, pk_str)),
            }
            for c in &t.columns {
                let mut flags: Vec<&str> = Vec::new();
                if c.not_null { flags.push("NOT NULL"); }
                if c.is_unique { flags.push("UNIQUE"); }
                let fk = t.foreign_keys.iter().find(|f| f.column == c.name);
                let fk_str = fk.map(|f| format!("  → {}.{}.{} (FK)", f.ref_schema, f.ref_table, f.ref_column)).unwrap_or_default();
                let flag_str = if flags.is_empty() { String::new() } else { format!("  {}", flags.join(", ")) };
                out.push_str(&format!("      - {} {}{}{}\n", c.name, c.data_type, flag_str, fk_str));
            }
        }
        out
    }
```

- [ ] **Step 6: Switch the call site to `load_table_schemas`.** In `do_initialize_inner` (`nodes/sql.rs` ~line 147), replace:

```rust
        let tables: Vec<TableInfo> = {
            let conn: &dyn SqlConnectionPort = adapter.as_ref();
            conn.load_table_metadata(&allowed_schemas)
                .await
                .unwrap_or_default()
        };
```

with:

```rust
        let tables: Vec<crate::dag_engine::domain::sql_ports::TableSchema> = {
            let conn: &dyn SqlConnectionPort = adapter.as_ref();
            conn.load_table_schemas(&allowed_schemas)
                .await
                .unwrap_or_default()
        };
```

The auto_rls loop below already uses `table.schema_name` / `table.table_name`, which `TableSchema` also has — no change needed there.

**Deny-warnings note:** after this switch, `TableInfo` may no longer be referenced in `nodes/sql.rs`. If so, remove it from the `use crate::dag_engine::domain::sql_ports::{...}` import in `sql.rs` (otherwise the unused import fails the build). Verify with `cargo build --lib`.

- [ ] **Step 7: Add a render unit test** — in `nodes/sql.rs` add a `#[cfg(test)] mod` (or reuse one) with:

```rust
#[cfg(test)]
mod supplement_tests {
    use super::*;
    use crate::dag_engine::domain::sql_ports::{ColumnInfo, ForeignKey, TableSchema};
    use crate::dag_engine::domain::sql_permissions::SqlPermissions;

    fn t() -> Vec<TableSchema> {
        vec![TableSchema {
            schema_name: "finanzas".into(),
            table_name: "gastos".into(),
            description: None,
            columns: vec![
                ColumnInfo { name: "id".into(), data_type: "integer".into(), not_null: true, is_pk: true, is_unique: false },
                ColumnInfo { name: "categoria_id".into(), data_type: "integer".into(), not_null: false, is_pk: false, is_unique: false },
            ],
            foreign_keys: vec![ForeignKey { column: "categoria_id".into(), ref_schema: "finanzas".into(), ref_table: "categorias".into(), ref_column: "id".into() }],
        }]
    }

    #[test]
    fn supplement_includes_columns_pk_and_fk() {
        let perms = SqlPermissions::from_config(Some(&serde_json::json!({ "preset": "read_write_delete", "allowed_schemas": ["finanzas"] }))).unwrap();
        let s = SqlNode::build_description_supplement(&t(), &[], &perms, 100);
        assert!(s.contains("PK: id"));
        assert!(s.contains("categoria_id"));
        assert!(s.contains("→ finanzas.categorias.id"));
        assert!(s.to_lowercase().contains("agregar columnas")); // capability NL
    }
}
```

- [ ] **Step 8: Run tests + clippy**

Run: `cargo test --lib supplement_includes_columns_pk_and_fk; cargo test --lib test_capabilities_nl_read_write_delete; cargo clippy --all-targets 2>&1 | tail -3`
Expected: tests PASS, clippy clean.

- [ ] **Step 9: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/sql_permissions.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs
git commit -m "feat(sql): inject full schema + NL capability statement into tool description"
```

---

## Task 6: Documentation

**Files:**
- Modify: `docs/developer_guide/23_sql_node.md`
- Modify: `docs/node_configurations.json`

- [ ] **Step 1: Update the presets table in `23_sql_node.md`.** Find the "#### Permission Presets" table and replace it with:

```markdown
| Preset | Allowed Operations |
|---|---|
| `read_only` | SELECT |
| `read_write` | SELECT, INSERT, UPDATE |
| `read_write_delete` | SELECT, INSERT, UPDATE, DELETE, ALTER TABLE ADD COLUMN |
| `full` | SELECT, INSERT, UPDATE, DELETE, ALTER TABLE ADD COLUMN, CREATE FUNCTION, CREATE TABLE |

**Always blocked (no preset enables these):** TRUNCATE, DROP, CREATE SCHEMA, and any `ALTER` that is not exclusively `ADD COLUMN` (DROP COLUMN, ALTER COLUMN TYPE, RENAME).
```

Also update the "Valid deny values" line to include `add_column`.

- [ ] **Step 2: Add a section on the injected context.** After "Initialization and Schema Introspection", add:

```markdown
### Contexto de schema y capacidades que ve el agente

En el init, el nodo introspecciona `allowed_schemas` y antepone a la descripción de la tool:
1. Un **bloque de capacidades en lenguaje natural** derivado del preset (qué puede y qué NO: borrar, agregar columnas, crear tablas).
2. El **schema completo** por tabla: columnas + tipos + `NOT NULL`/`UNIQUE`, marca de PRIMARY KEY y foreign keys (`→ schema.tabla.columna`).

Si `allowed_schemas` tiene más de 40 tablas (o el render supera ~8000 chars), degrada a solo nombres de tablas + una nota para usar introspección. Así el agente entiende el modelo sin gastar turnos introspeccionando ni intentar operaciones que su preset bloquea.
```

- [ ] **Step 3: Update `node_configurations.json`.** In the `sql_query` → `permissions.preset` description/enum, add `read_write_delete` as a valid value; in `permissions.deny`, add `add_column` as a valid value. Keep the file valid JSON.

- [ ] **Step 4: Validate JSON + commit**

```bash
python3 -c "import json; json.load(open('docs/node_configurations.json')); print('valid')"
git add docs/developer_guide/23_sql_node.md docs/node_configurations.json
git commit -m "docs(sql): read_write_delete preset + ADD COLUMN + injected schema context"
```

---

## Task 7: End-to-end agent test

**Files:**
- Create: `tests/graphs/agents/sql_read_write_delete_e2e.json`

**Precondition:** the target DB must have a `finanzas.gastos` table. The E2E run includes a psql seed step (idempotent) so it is self-contained.

- [ ] **Step 1: Create the graph:**

```json
{
  "comment": "E2E: read_write_delete agent — sees injected schema + capabilities, can DELETE rows and ADD COLUMN, cannot CREATE TABLE.",
  "metadata": { "category": "agents", "requires_env": ["GEMINI_API_KEY", "DATABASE_URL"] },
  "nodes": {
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "stream": false,
        "system_message": "Sos un asistente de finanzas. Usás la tool gastos_db. Mirá tus permisos y el schema que se te inyectan en la descripción de la tool antes de actuar.",
        "tool_configurations": {
          "gastos_db": {
            "name": "gastos_db",
            "node_type": "sql_query",
            "description": "Gestiona los gastos.",
            "node_schema": {
              "connection_url": { "type": "string", "fixed": "${DATABASE_URL}" },
              "permissions": { "type": "object", "fixed": { "preset": "read_write_delete", "allowed_schemas": ["finanzas"] } },
              "query": { "type": "string", "required": true, "description": "SQL a ejecutar." }
            }
          }
        },
        "prompt": "Agregá una columna 'metodo_pago' de texto a la tabla de gastos, después borrá todos los gastos con monto menor a 1, y finalmente decime cuántos gastos quedan."
      }
    },
    "result": { "type": "output", "config": { "label": "RWD Agent Result" } }
  },
  "edges": [ { "from": "agent", "to": "result" } ]
}
```

- [ ] **Step 2: Seed the table + run + capture SSE**

```bash
mkdir -p /tmp/colmena_e2e
set -a && source .env && set +a
# Idempotent seed so the run is self-contained.
psql "$DATABASE_URL" -c "CREATE SCHEMA IF NOT EXISTS finanzas; CREATE TABLE IF NOT EXISTS finanzas.gastos (id SERIAL PRIMARY KEY, monto NUMERIC(12,2), descripcion TEXT);" 2>&1 | tail -2
cargo run --bin dag_engine -- run tests/graphs/agents/sql_read_write_delete_e2e.json --agent-session-id agent_rwd_001 > /tmp/colmena_e2e/sql_rwd.sse 2>&1
echo "exit=$?"
```
Expected: the agent issues an `ALTER TABLE finanzas.gastos ADD COLUMN metodo_pago …` (allowed), a `DELETE … WHERE monto < 1` (allowed), and a `SELECT count(*)`. No "ALTER is not allowed". If it also tries `CREATE TABLE`, that is blocked — acceptable as long as the required ops succeed.

- [ ] **Step 3: Verify on the DB**

```bash
set -a && source .env && set +a
psql "$DATABASE_URL" -c "SELECT column_name FROM information_schema.columns WHERE table_schema='finanzas' AND table_name='gastos' AND column_name='metodo_pago';"
```
Expected: one row `metodo_pago` (the ADD COLUMN persisted).

- [ ] **Step 4: Present a clean SSE report** (reuse the parser pattern from prior runs): tool calls + the SQL each issued + results + final answer + tokens. Do NOT paste raw SSE. Note the SSE path.

- [ ] **Step 5: Commit**

```bash
git add tests/graphs/agents/sql_read_write_delete_e2e.json
git commit -m "test(sql): E2E read_write_delete agent (ADD COLUMN + DELETE allowed, schema injected)"
```

---

## Final verification

- [ ] `set -a && source .env && set +a && TEST_DATABASE_URL=$DATABASE_URL cargo test --lib -- --include-ignored 2>&1 | grep "test result"` — all pass (unit + the ignored introspection test).
- [ ] `cargo clippy --all-targets 2>&1 | tail -3` — clean.
- [ ] `cargo fmt`.
- [ ] ADP sweep: confirm no external impl of `SqlConnectionPort` (only `PgPoolAdapter` in-repo); the new preset value + trait method are additive. State this in the final report.

## Out of scope (do NOT build)
- Multi-column FK rendering (single-column FKs only; note it if a multi-column FK appears).
- Changing `read_only`/`read_write` semantics.
- Allowing any ALTER variant other than ADD COLUMN.
- SSE event for schema injection.
