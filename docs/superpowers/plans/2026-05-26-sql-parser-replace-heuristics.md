# SQL Node — Replace Regex/Heuristics with `sqlparser` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace all regex/substring heuristics across the SQL node with AST-based analysis using the `sqlparser` crate (PostgreSQL dialect), eliminating false positives (the `1.81` schema-extraction bug) and closing the multi-statement validation hole.

**Architecture:** Introduce a single shared `sql_ast` helper module that parses queries once with `sqlparser::dialect::PostgreSqlDialect` and exposes typed accessors (`operations`, `referenced_schemas`, `has_where`, `has_comment_on`, `created_table_name`, `created_function_name`, `function_comment`). All current consumers (validator, execution service, pool adapter, sql node) call into this module instead of re-parsing strings. Multi-statement queries are validated per-statement so `SELECT 1; DROP TABLE x;` no longer slips through.

**Tech Stack:** Rust, `sqlparser = "0.62"`, existing `colmena_dag_engine` crate (sqlx + tokio + serde_json already in tree).

---

## Findings from Pre-Plan Audit

### Heuristic / regex usage to replace (4 files, ~13 sites)

| File | Lines | Heuristic | Replace with |
|------|-------|-----------|--------------|
| [sql_static_validator.rs:14-41](../../../src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs:14) | `detect_operation` startswith chain | AST `match Statement::*` |
| [sql_static_validator.rs:44-54](../../../src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs:44) | `extract_schemas` regex `\b(\w+)\.(\w+)` (**the bug**) | Walk `ObjectName` nodes in AST |
| [sql_static_validator.rs:57-60](../../../src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs:57) | `has_where_clause` substring | `Statement::{Update,Delete}.selection.is_some()` |
| [sql_static_validator.rs:63-66](../../../src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs:63) | `has_comment` substring | `Statement::Comment { .. }` in statement list |
| [sql_static_validator.rs:170-176](../../../src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs:170) | `SELECT *` warning substring | `Select.projection contains SelectItem::Wildcard` |
| [sql_execution_service.rs:140-156](../../../src/libs/colmena/src/dag_engine/application/sql_execution_service.rs:140) | CREATE FUNCTION startswith → register | AST `Statement::CreateFunction` detection |
| [sql_execution_service.rs:168-174](../../../src/libs/colmena/src/dag_engine/application/sql_execution_service.rs:168) | `extract_function_name` regex | `Statement::CreateFunction.name` |
| [sql_execution_service.rs:177-182](../../../src/libs/colmena/src/dag_engine/application/sql_execution_service.rs:177) | `extract_comment` regex (quote-based, breaks on escaped quotes) | `Statement::Comment.comment` value |
| [sql_pool_adapter.rs:275-276](../../../src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs:275) | startswith SELECT/WITH → is_select | `Statement::{Query}` match (covers CTE) |
| [sql_pool_adapter.rs:306](../../../src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs:306) | substring "LIMIT" (false positive on `WHERE x='LIMIT'`) | `Query.limit.is_some()` |
| [sql_pool_adapter.rs:381-394](../../../src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs:381) | startswith CREATE FUNCTION/TABLE for output shape | AST statement-type dispatch |
| [nodes/sql.rs:232-248](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs:232) | `extract_create_table_name` regex | `Statement::CreateTable.name` |
| [nodes/sql.rs:390-391](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs:390) | startswith CREATE TABLE → apply RLS | AST `Statement::CreateTable` detection |

### Multi-row INSERT support (user question)

**Answer: YES, fully supported today.** Multi-row INSERT is standard Postgres:

```sql
INSERT INTO seb_data.productos (sku, name) VALUES
  ('A1', 'p1'), ('A2', 'p2'), ('A3', 'p3'), ... ('A100', 'p100');
```

Tracing the path:
1. **Validator**: `detect_operation` matches `INSERT` → `SqlOperation::Insert` → permission check → schema check. Number of VALUES rows is irrelevant. ✅
2. **Executor** ([sql_pool_adapter.rs:369](../../../src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs:369)): Non-SELECT → `execute()` → returns `rows_affected`. ✅
3. `max_rows` only caps SELECT result sets ([sql_pool_adapter.rs:358](../../../src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs:358)), not INSERT row counts.

**Effective limits:**
- `statement_timeout_ms` (default 30s) — could trip on very large inserts; bump in `runtime_limits` if needed.
- Postgres protocol message size (~1GB) — non-issue in practice.
- For 1000+ rows, prefer `INSERT … SELECT FROM unnest(…)` or multi-row VALUES; both work.

**Hole that this plan also fixes:** today multi-statement queries (`SELECT 1; DROP TABLE x;`) only validate the first statement because the validator looks at `query.starts_with(...)`. After this plan every statement in the script is validated independently. This is mentioned in Task 6.

### Policy decision destapped by the parser

With a real parser we now identify DDL the regex couldn't classify: `CREATE SCHEMA`, `CREATE INDEX`, `CREATE VIEW`, `GRANT/REVOKE`. Decision baked into Task 3: **block these explicitly** with a useful message ("create schemas via migration; create indexes via DBA"), same posture as today's `DROP/ALTER/TRUNCATE`. No new permission flag — keeping the surface conservative.

### File structure

- **New:** `src/libs/colmena/src/dag_engine/infrastructure/sql_ast.rs` — shared parsing module, sole user of `sqlparser`.
- **Modified:** `Cargo.toml`, `sql_static_validator.rs`, `sql_execution_service.rs`, `sql_pool_adapter.rs`, `nodes/sql.rs`, `infrastructure/mod.rs`.
- **Docs touched:** `docs/developer_guide/23_sql_node.md` (new policy section).

---

## Task 1: Add `sqlparser` dependency and skeleton module

**Files:**
- Modify: `src/libs/colmena/Cargo.toml`
- Create: `src/libs/colmena/src/dag_engine/infrastructure/sql_ast.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/mod.rs`

- [ ] **Step 1: Add dependency**

Open `src/libs/colmena/Cargo.toml` and add under `[dependencies]` (alphabetical order, near `sqlx`):

```toml
sqlparser = "0.62"
```

- [ ] **Step 2: Verify it compiles in isolation**

Run: `cargo check -p colmena_dag_engine`
Expected: `Finished` with no errors (just adds the dep).

- [ ] **Step 3: Create the skeleton module**

Create `src/libs/colmena/src/dag_engine/infrastructure/sql_ast.rs`:

```rust
//! Shared SQL AST helpers built on `sqlparser` (PostgreSQL dialect).
//!
//! All consumers of structural SQL analysis (validator, execution service,
//! pool adapter, sql node) MUST go through this module rather than rolling
//! their own regex/substring heuristics — that is how the `1.81` schema
//! false-positive bug got in.

use sqlparser::ast::Statement;
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::{Parser, ParserError};

/// Parse a SQL script into one or more statements using the PostgreSQL dialect.
/// Returns the full statement vector so callers can validate / inspect each.
pub fn parse(query: &str) -> Result<Vec<Statement>, ParserError> {
    Parser::parse_sql(&PostgreSqlDialect {}, query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_select() {
        let stmts = parse("SELECT 1").unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn parses_multi_statement() {
        let stmts = parse("SELECT 1; SELECT 2;").unwrap();
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn errors_on_garbage() {
        assert!(parse("this is not sql").is_err());
    }
}
```

- [ ] **Step 4: Register the module**

In `src/libs/colmena/src/dag_engine/infrastructure/mod.rs`, add (alphabetical with the other `sql_*` modules):

```rust
pub mod sql_ast;
```

- [ ] **Step 5: Run unit tests**

Run: `cargo test --lib -p colmena_dag_engine sql_ast`
Expected: 3 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/Cargo.toml \
        src/libs/colmena/src/dag_engine/infrastructure/sql_ast.rs \
        src/libs/colmena/src/dag_engine/infrastructure/mod.rs
git commit -m "feat(sql): add sqlparser dep and sql_ast helper module"
```

---

## Task 2: AST-based operation detection and schema extraction

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/sql_ast.rs`

This task fills the `sql_ast` module with the typed accessors every downstream consumer needs. We test it exhaustively here so later tasks only need to wire it up.

- [ ] **Step 1: Write failing tests for `classify` and `referenced_schemas`**

Add to `sql_ast.rs` tests module:

```rust
use crate::dag_engine::domain::sql_permissions::SqlOperation;

#[test]
fn classify_select() {
    let stmts = parse("SELECT id FROM s.t WHERE id=1").unwrap();
    assert_eq!(classify(&stmts[0]), Some(SqlOperation::Select));
}

#[test]
fn classify_insert() {
    let stmts = parse("INSERT INTO s.t (a) VALUES (1)").unwrap();
    assert_eq!(classify(&stmts[0]), Some(SqlOperation::Insert));
}

#[test]
fn classify_update_delete_create() {
    assert_eq!(
        classify(&parse("UPDATE s.t SET a=1 WHERE id=2").unwrap()[0]),
        Some(SqlOperation::Update)
    );
    assert_eq!(
        classify(&parse("DELETE FROM s.t WHERE id=2").unwrap()[0]),
        Some(SqlOperation::Delete)
    );
    assert_eq!(
        classify(&parse("CREATE TABLE s.t (id INT)").unwrap()[0]),
        Some(SqlOperation::CreateTable)
    );
    assert_eq!(
        classify(
            &parse("CREATE FUNCTION s.f() RETURNS VOID AS $$ BEGIN END; $$ LANGUAGE plpgsql")
                .unwrap()[0]
        ),
        Some(SqlOperation::CreateFunction)
    );
    assert_eq!(
        classify(&parse("TRUNCATE TABLE s.t").unwrap()[0]),
        Some(SqlOperation::Truncate)
    );
    assert_eq!(
        classify(&parse("DROP TABLE s.t").unwrap()[0]),
        Some(SqlOperation::Drop)
    );
    assert_eq!(
        classify(&parse("ALTER TABLE s.t ADD COLUMN x INT").unwrap()[0]),
        Some(SqlOperation::Alter)
    );
}

#[test]
fn classify_new_ddl_returns_unsupported() {
    // CREATE SCHEMA / INDEX / VIEW / GRANT / REVOKE are explicitly NOT mapped
    // to a SqlOperation — the validator will block them with a clear message.
    assert!(classify(&parse("CREATE SCHEMA foo").unwrap()[0]).is_none());
    assert!(classify(&parse("CREATE INDEX i ON s.t(a)").unwrap()[0]).is_none());
    assert!(classify(&parse("CREATE VIEW s.v AS SELECT 1").unwrap()[0]).is_none());
}

#[test]
fn referenced_schemas_ignores_numeric_literals() {
    // The bug that triggered this whole refactor: `1.81` was matched as
    // `schema='1', table='81'` by the old regex.
    let stmts = parse(
        "INSERT INTO seb_data.productos (sku, peso_kg) VALUES ('A1', 1.81)"
    ).unwrap();
    let schemas = referenced_schemas(&stmts[0]);
    assert_eq!(schemas, vec!["seb_data".to_string()]);
}

#[test]
fn referenced_schemas_finds_joins() {
    let stmts = parse(
        "SELECT u.id FROM core.users u JOIN audit.events e ON e.user_id = u.id"
    ).unwrap();
    let mut s = referenced_schemas(&stmts[0]);
    s.sort();
    assert_eq!(s, vec!["audit".to_string(), "core".to_string()]);
}

#[test]
fn referenced_schemas_handles_json_path() {
    // `data->>'key'` and `obj.field` outside identifier position must not be
    // counted as schema references.
    let stmts = parse(
        "SELECT data->>'foo' FROM public.t WHERE data->>'bar' = 'x'"
    ).unwrap();
    let schemas = referenced_schemas(&stmts[0]);
    assert_eq!(schemas, vec!["public".to_string()]);
}

#[test]
fn select_has_wildcard_detects_star() {
    let stmts = parse("SELECT * FROM s.t").unwrap();
    assert!(select_has_wildcard(&stmts[0]));
    let stmts = parse("SELECT id, name FROM s.t").unwrap();
    assert!(!select_has_wildcard(&stmts[0]));
}

#[test]
fn has_where_for_update_delete() {
    assert!(has_where(&parse("UPDATE s.t SET a=1 WHERE id=2").unwrap()[0]));
    assert!(!has_where(&parse("UPDATE s.t SET a=1").unwrap()[0]));
    assert!(has_where(&parse("DELETE FROM s.t WHERE id=2").unwrap()[0]));
    assert!(!has_where(&parse("DELETE FROM s.t").unwrap()[0]));
}

#[test]
fn script_has_comment_on_detects_multistatement() {
    let stmts = parse(
        "CREATE FUNCTION s.f() RETURNS VOID AS $$ BEGIN END; $$ LANGUAGE plpgsql;\
         COMMENT ON FUNCTION s.f() IS 'doc'"
    ).unwrap();
    assert!(script_has_comment_on(&stmts));
}

#[test]
fn created_table_name_extracts_schema_and_name() {
    let stmts = parse("CREATE TABLE seb_data.productos (id INT)").unwrap();
    let (schema, table) = created_table_name(&stmts[0]).unwrap();
    assert_eq!(schema, "seb_data");
    assert_eq!(table, "productos");

    // unqualified → defaults to "public"
    let stmts = parse("CREATE TABLE t (id INT)").unwrap();
    let (schema, table) = created_table_name(&stmts[0]).unwrap();
    assert_eq!(schema, "public");
    assert_eq!(table, "t");
}

#[test]
fn created_function_name_extracts() {
    let stmts = parse(
        "CREATE FUNCTION sandbox.my_func() RETURNS VOID AS $$ BEGIN END; $$ LANGUAGE plpgsql"
    ).unwrap();
    assert_eq!(
        created_function_name(&stmts[0]).unwrap(),
        "sandbox.my_func"
    );
}

#[test]
fn query_has_limit_detects_only_real_limit() {
    let stmts = parse("SELECT * FROM s.t LIMIT 10").unwrap();
    assert!(query_has_limit(&stmts[0]));
    // The old substring check matched literals like 'LIMIT' inside WHERE.
    let stmts = parse("SELECT * FROM s.t WHERE name = 'LIMIT'").unwrap();
    assert!(!query_has_limit(&stmts[0]));
}

#[test]
fn is_query_recognises_with_cte() {
    let stmts = parse("WITH x AS (SELECT 1) SELECT * FROM x").unwrap();
    assert!(is_query(&stmts[0]));
    let stmts = parse("SELECT 1").unwrap();
    assert!(is_query(&stmts[0]));
    let stmts = parse("INSERT INTO s.t (a) VALUES (1)").unwrap();
    assert!(!is_query(&stmts[0]));
}
```

- [ ] **Step 2: Run tests to verify all fail with "not defined"**

Run: `cargo check -p colmena_dag_engine --tests 2>&1 | grep -E "not found|undefined"`
Expected: errors for `classify`, `referenced_schemas`, `select_has_wildcard`, `has_where`, `script_has_comment_on`, `created_table_name`, `created_function_name`, `query_has_limit`, `is_query`.

- [ ] **Step 3: Implement the accessors**

Append to `sql_ast.rs` (above the `#[cfg(test)]` block):

```rust
use crate::dag_engine::domain::sql_permissions::SqlOperation;
use sqlparser::ast::{
    visit_relations, ObjectName, Query, Select, SelectItem, SetExpr,
};
use std::ops::ControlFlow;

/// Map a parsed statement to its `SqlOperation`, or `None` for statement kinds
/// we intentionally do not support (CREATE SCHEMA/INDEX/VIEW, GRANT, REVOKE,
/// COMMENT-only, etc.). The validator turns `None` into a precise block reason.
pub fn classify(stmt: &Statement) -> Option<SqlOperation> {
    match stmt {
        Statement::Query(_) => Some(SqlOperation::Select),
        Statement::Insert { .. } => Some(SqlOperation::Insert),
        Statement::Update { .. } => Some(SqlOperation::Update),
        Statement::Delete { .. } => Some(SqlOperation::Delete),
        Statement::CreateTable { .. } => Some(SqlOperation::CreateTable),
        Statement::CreateFunction { .. } => Some(SqlOperation::CreateFunction),
        Statement::Truncate { .. } => Some(SqlOperation::Truncate),
        Statement::Drop { .. } => Some(SqlOperation::Drop),
        Statement::AlterTable { .. }
        | Statement::AlterIndex { .. }
        | Statement::AlterView { .. } => Some(SqlOperation::Alter),
        _ => None,
    }
}

/// Collect every distinct schema referenced by table identifiers in the
/// statement. Walks the AST via `visit_relations`, so numeric literals, JSON
/// arrow operators, and `obj.method` calls are never miscounted as schemas.
pub fn referenced_schemas(stmt: &Statement) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let _: ControlFlow<()> = visit_relations(stmt, |name: &ObjectName| {
        // ObjectName is `Vec<Ident>` — 2+ parts means `schema.table[.col]`.
        if name.0.len() >= 2 {
            let schema = name.0[0].value.to_lowercase();
            if !found.contains(&schema) {
                found.push(schema);
            }
        }
        ControlFlow::Continue(())
    });
    found.sort();
    found
}

/// True when a SELECT has `*` or `t.*` in any projection slot.
pub fn select_has_wildcard(stmt: &Statement) -> bool {
    fn walk_query(q: &Query) -> bool {
        match &*q.body {
            SetExpr::Select(s) => walk_select(s),
            SetExpr::Query(inner) => walk_query(inner),
            SetExpr::SetOperation { left, right, .. } => {
                walk_set(left) || walk_set(right)
            }
            _ => false,
        }
    }
    fn walk_set(expr: &SetExpr) -> bool {
        match expr {
            SetExpr::Select(s) => walk_select(s),
            SetExpr::Query(q) => walk_query(q),
            SetExpr::SetOperation { left, right, .. } => walk_set(left) || walk_set(right),
            _ => false,
        }
    }
    fn walk_select(s: &Select) -> bool {
        s.projection.iter().any(|p| matches!(
            p,
            SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _)
        ))
    }
    match stmt {
        Statement::Query(q) => walk_query(q),
        _ => false,
    }
}

/// True if a DELETE/UPDATE has a WHERE clause. Always false for other kinds.
pub fn has_where(stmt: &Statement) -> bool {
    match stmt {
        Statement::Update { selection, .. } => selection.is_some(),
        Statement::Delete { selection, .. } => selection.is_some(),
        _ => false,
    }
}

/// True if any statement in the script is `COMMENT ON ...`.
/// Used to enforce that CREATE FUNCTION ships with a description.
pub fn script_has_comment_on(stmts: &[Statement]) -> bool {
    stmts.iter().any(|s| matches!(s, Statement::Comment { .. }))
}

/// For a `CREATE TABLE`, return `(schema, table)`. Defaults schema to `"public"`
/// when the table name is unqualified.
pub fn created_table_name(stmt: &Statement) -> Option<(String, String)> {
    if let Statement::CreateTable { name, .. } = stmt {
        return Some(split_object_name(name, "public"));
    }
    None
}

/// For a `CREATE FUNCTION`, return its fully qualified name as a single string
/// (matches the legacy `extract_function_name` return shape).
pub fn created_function_name(stmt: &Statement) -> Option<String> {
    if let Statement::CreateFunction { name, .. } = stmt {
        return Some(name.to_string());
    }
    None
}

/// True if the statement is a `SELECT` or a `WITH ... SELECT` (CTE).
pub fn is_query(stmt: &Statement) -> bool {
    matches!(stmt, Statement::Query(_))
}

/// True if the top-level query already has an explicit `LIMIT` clause.
pub fn query_has_limit(stmt: &Statement) -> bool {
    match stmt {
        Statement::Query(q) => q.limit.is_some(),
        _ => false,
    }
}

fn split_object_name(name: &ObjectName, default_schema: &str) -> (String, String) {
    match name.0.as_slice() {
        [single] => (default_schema.to_string(), single.value.clone()),
        [schema, table, ..] => (schema.value.clone(), table.value.clone()),
        [] => (default_schema.to_string(), String::new()),
    }
}
```

> **Note for the engineer:** `visit_relations`, `Statement` variant fields, and `Query.limit` exist in `sqlparser 0.62`. If a variant name shifts in a future version (e.g. `CreateTable` becoming a struct-wrapped variant), the compiler will tell you which line to adjust — match patterns are the only fragile surface.

- [ ] **Step 4: Run tests**

Run: `cargo test --lib -p colmena_dag_engine sql_ast`
Expected: all 12+ tests pass (3 from Task 1, the new ones from this task).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/sql_ast.rs
git commit -m "feat(sql_ast): AST-based classify/schema/where/comment/wildcard helpers"
```

---

## Task 3: Migrate `StaticRuleValidator` to use `sql_ast`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs`

- [ ] **Step 1: Add new failing tests at the bottom of the existing `tests` module**

Open `sql_static_validator.rs`, scroll to the `mod tests` block, and add:

```rust
#[test]
fn test_insert_with_decimal_literal_no_false_positive() {
    // Regression: the old regex matched `1.81` as schema="1", table="81".
    let v = StaticRuleValidator;
    let r = v.validate(
        "INSERT INTO seb_data.productos (sku, peso) VALUES ('A1', 1.81)",
        &SqlPermissions::from_config(Some(&serde_json::json!({
            "preset": "full",
            "allowed_schemas": ["seb_data"],
            "sandbox_schema": "seb_data"
        }))).unwrap(),
    );
    assert!(r.allowed, "decimal literal must not be misread as schema");
}

#[test]
fn test_create_schema_blocked_with_useful_message() {
    let v = StaticRuleValidator;
    let r = v.validate("CREATE SCHEMA foo", &full_perms());
    assert!(!r.allowed);
    assert!(
        r.block_reason.as_deref().unwrap().contains("CREATE SCHEMA"),
        "block reason should name the rejected op"
    );
}

#[test]
fn test_create_index_blocked() {
    let v = StaticRuleValidator;
    let r = v.validate(
        "CREATE INDEX idx ON production.users (email)",
        &full_perms(),
    );
    assert!(!r.allowed);
}

#[test]
fn test_unparseable_query_blocked() {
    let v = StaticRuleValidator;
    let r = v.validate("this is not sql", &full_perms());
    assert!(!r.allowed);
    assert!(r.block_reason.unwrap().to_lowercase().contains("parse"));
}

#[test]
fn test_multistatement_validates_each() {
    // The old validator looked at the first keyword and let DROP slip through.
    let v = StaticRuleValidator;
    let r = v.validate(
        "SELECT * FROM production.users; DROP TABLE production.users",
        &full_perms(),
    );
    assert!(!r.allowed, "DROP in second statement must be blocked");
    assert!(r.block_reason.unwrap().contains("DROP"));
}

#[test]
fn test_multirow_insert_allowed() {
    let v = StaticRuleValidator;
    let r = v.validate(
        "INSERT INTO public.t (a, b) VALUES (1, 'x'), (2, 'y'), (3, 'z')",
        &full_perms(),
    );
    assert!(r.allowed, "multi-row INSERT must work for bulk loads");
}
```

- [ ] **Step 2: Run tests to confirm new ones fail**

Run: `cargo test --lib -p colmena_dag_engine sql_static_validator`
Expected: 6 new tests fail; existing tests still pass.

- [ ] **Step 3: Rewrite the validator body**

Replace the entire `StaticRuleValidator` impl and `SqlValidatorPort` impl (delete lines 9 through 184 — keep the file header comment and the `tests` module). Paste:

```rust
use crate::dag_engine::domain::sql_permissions::{SqlOperation, SqlPermissions};
use crate::dag_engine::domain::sql_ports::{SqlValidatorPort, ValidationResult};
use crate::dag_engine::infrastructure::sql_ast;

/// Stateless validator that runs every parsed statement through static rules.
pub struct StaticRuleValidator;

impl SqlValidatorPort for StaticRuleValidator {
    fn validate(&self, query: &str, permissions: &SqlPermissions) -> ValidationResult {
        let stmts = match sql_ast::parse(query) {
            Ok(s) if !s.is_empty() => s,
            Ok(_) => {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some("Empty SQL script.".to_string()),
                    warnings: vec![],
                };
            }
            Err(e) => {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some(format!("Failed to parse SQL: {}", e)),
                    warnings: vec![],
                };
            }
        };

        let mut warnings: Vec<String> = Vec::new();
        let mut create_function_seen = false;

        for stmt in &stmts {
            // 1. Classify
            let operation = match sql_ast::classify(stmt) {
                Some(op) => op,
                None => {
                    let kind = stmt_kind_name(stmt);
                    return ValidationResult {
                        allowed: false,
                        block_reason: Some(format!(
                            "{} is not supported. Only SELECT, INSERT, UPDATE, DELETE, \
                             CREATE TABLE, and CREATE FUNCTION are allowed. Manage schemas, \
                             indexes, and views via migrations.",
                            kind
                        )),
                        warnings: vec![],
                    };
                }
            };

            // 2. Always-blocked DDL
            match &operation {
                SqlOperation::Truncate => {
                    return blocked(
                        "TRUNCATE is not allowed. Use DELETE with a WHERE clause instead.",
                    );
                }
                SqlOperation::Drop => {
                    return blocked(
                        "DROP is not allowed. You can only create objects in the sandbox schema.",
                    );
                }
                SqlOperation::Alter => {
                    return blocked("ALTER is not allowed on any schema.");
                }
                _ => {}
            }

            // 3. Permission preset
            if !permissions.is_allowed(&operation) {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some(format!(
                        "{:?} is not permitted by the current permission preset. \
                         Allowed operations: {}",
                        operation,
                        permissions.describe_for_llm()
                    )),
                    warnings: vec![],
                };
            }

            // 4. Schema allowlist
            for schema in sql_ast::referenced_schemas(stmt) {
                if !permissions.is_schema_allowed(&schema) {
                    return ValidationResult {
                        allowed: false,
                        block_reason: Some(format!(
                            "Access to schema '{}' is not allowed. \
                             Allowed schemas: check your permissions config.",
                            schema
                        )),
                        warnings: vec![],
                    };
                }
            }

            // 5. DELETE/UPDATE without WHERE
            if matches!(operation, SqlOperation::Delete | SqlOperation::Update)
                && !sql_ast::has_where(stmt)
            {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some(format!(
                        "{:?} without a WHERE clause is not allowed. \
                         Specify which rows to affect.",
                        operation
                    )),
                    warnings: vec![],
                };
            }

            if matches!(operation, SqlOperation::CreateFunction) {
                create_function_seen = true;
            }

            // 6. SELECT * warning (non-blocking)
            if sql_ast::select_has_wildcard(stmt) {
                warnings.push(
                    "Prefer selecting specific columns instead of SELECT * to reduce \
                     data transfer and improve clarity.".to_string()
                );
            }
        }

        // 7. CREATE FUNCTION requires a COMMENT ON somewhere in the script
        if create_function_seen && !sql_ast::script_has_comment_on(&stmts) {
            return blocked(
                "CREATE FUNCTION requires a COMMENT ON FUNCTION statement describing \
                 the function's purpose. Include it in the same query.",
            );
        }

        ValidationResult {
            allowed: true,
            block_reason: None,
            warnings,
        }
    }
}

fn blocked(reason: &str) -> ValidationResult {
    ValidationResult {
        allowed: false,
        block_reason: Some(reason.to_string()),
        warnings: vec![],
    }
}

fn stmt_kind_name(stmt: &sqlparser::ast::Statement) -> &'static str {
    use sqlparser::ast::Statement::*;
    match stmt {
        CreateSchema { .. } => "CREATE SCHEMA",
        CreateIndex { .. } => "CREATE INDEX",
        CreateView { .. } => "CREATE VIEW",
        Grant { .. } => "GRANT",
        Revoke { .. } => "REVOKE",
        Comment { .. } => "Stand-alone COMMENT",
        _ => "This statement type",
    }
}
```

- [ ] **Step 4: Run all validator tests**

Run: `cargo test --lib -p colmena_dag_engine sql_static_validator`
Expected: all original tests + 6 new tests pass.

- [ ] **Step 5: Confirm the user's original bug is fixed**

Run: `cargo test --lib -p colmena_dag_engine sql_static_validator::tests::test_insert_with_decimal_literal_no_false_positive`
Expected: PASS — this is the `1.81` case.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs
git commit -m "refactor(sql): replace validator heuristics with sql_ast (fixes 1.81 false positive)"
```

---

## Task 4: Migrate `SqlExecutionService` regex extractors

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/sql_execution_service.rs`

- [ ] **Step 1: Locate the heuristic block**

Open the file and find:
- Lines 139-156: `let trimmed = query.trim_start().to_uppercase(); if trimmed.starts_with("CREATE FUNCTION") ...`
- Lines 167-182: `extract_function_name` and `extract_comment` helpers

- [ ] **Step 2: Replace the CREATE FUNCTION detection with AST walk**

Change the block at lines 139-156 from the regex-based logic to:

```rust
// If any statement is CREATE FUNCTION, register it.
// Parsing here is cheap (same query already parsed during validation) — accept
// the duplicate parse for now; can be plumbed through a shared cache later.
if let Ok(stmts) = crate::dag_engine::infrastructure::sql_ast::parse(query) {
    for stmt in &stmts {
        if let Some(func_name) =
            crate::dag_engine::infrastructure::sql_ast::created_function_name(stmt)
        {
            let comment = extract_comment_from_stmts(&stmts).unwrap_or_default();
            let info = crate::dag_engine::domain::sql_ports::FunctionInfo {
                function_name: func_name,
                schema_name: permissions.sandbox_schema().to_string(),
                parameters: None,
                return_type: None,
                description: comment,
            };
            let _ = self.registry.register_function(&info, session_id).await;
        }
    }
}
```

- [ ] **Step 3: Replace `extract_function_name` and `extract_comment` helpers**

Delete the two regex-based helpers (lines 168-182) and replace with:

```rust
/// Pull the comment text out of the first `COMMENT ON ... IS '<text>'`
/// statement in the script. Uses sqlparser's AST instead of a quote-based
/// regex (which used to break on escaped quotes inside the comment body).
fn extract_comment_from_stmts(stmts: &[sqlparser::ast::Statement]) -> Option<String> {
    for stmt in stmts {
        if let sqlparser::ast::Statement::Comment { comment, .. } = stmt {
            return Some(comment.clone());
        }
    }
    None
}
```

- [ ] **Step 4: Add a regression test for escaped quotes**

In the same file's test module (or create one if none exists), add:

```rust
#[cfg(test)]
mod ast_extract_tests {
    use super::*;
    use crate::dag_engine::infrastructure::sql_ast;

    #[test]
    fn extract_comment_handles_apostrophe_in_text() {
        // The old regex `'([^']*)'` truncated at the first apostrophe.
        let stmts = sql_ast::parse(
            "COMMENT ON FUNCTION s.f() IS 'It''s a great function'",
        )
        .unwrap();
        assert_eq!(
            extract_comment_from_stmts(&stmts),
            Some("It's a great function".to_string())
        );
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib -p colmena_dag_engine sql_execution_service`
Expected: PASS, including the new apostrophe regression test.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/application/sql_execution_service.rs
git commit -m "refactor(sql_exec): replace function_name/comment regex with AST extraction"
```

---

## Task 5: Migrate `PgPoolAdapter` execution-path heuristics

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs`

This task removes the three heuristics that decide (a) SELECT-vs-write path, (b) whether to append `LIMIT`, (c) what shape to use for the `output` field on DDL.

- [ ] **Step 1: Parse once at the top of `execute_query`**

In `impl SqlConnectionPort for PgPoolAdapter`, at the top of `execute_query` (right after `let work_mem = self.work_mem_mb;`, line ~273), insert:

```rust
let parsed = crate::dag_engine::infrastructure::sql_ast::parse(query).ok();
// First statement classifier — execution-path decisions only consider the
// outer shape, but the validator (which ran before us) checked every statement.
let first_stmt = parsed.as_ref().and_then(|s| s.first());
let is_select = first_stmt.map(crate::dag_engine::infrastructure::sql_ast::is_query)
    .unwrap_or(false);
let already_has_limit = first_stmt
    .map(crate::dag_engine::infrastructure::sql_ast::query_has_limit)
    .unwrap_or(false);
```

Then **remove** the old lines 275-276:

```rust
// DELETE THESE TWO LINES
let trimmed = query.trim_start().to_uppercase();
let is_select = trimmed.starts_with("SELECT") || trimmed.starts_with("WITH");
```

- [ ] **Step 2: Replace the LIMIT check**

On line ~306, change:

```rust
let limited_query = if max_rows > 0 && !trimmed.contains("LIMIT") {
```

to:

```rust
let limited_query = if max_rows > 0 && !already_has_limit {
```

- [ ] **Step 3: Replace the CREATE TABLE / CREATE FUNCTION output shape**

On lines 381-394, replace the `trimmed.starts_with(...)` chain with AST-based dispatch:

```rust
use sqlparser::ast::Statement;
let (output, row_count) = match first_stmt {
    Some(Statement::CreateFunction { .. }) => {
        (json!({ "created": true }), 0u64)
    }
    Some(Statement::CreateTable { .. }) => {
        (json!({ "created": true, "type": "table" }), 0u64)
    }
    _ => (json!({ "rows_affected": rows_affected }), rows_affected),
};
Ok(QueryResult {
    output,
    row_count,
    truncated: false,
})
```

(Replace the entire `if/else if/else` block, which is the last block inside the non-SELECT branch.)

- [ ] **Step 4: Remove `trimmed` if no longer used**

Search the function for remaining uses of `trimmed`. If there are none, the variable is gone — the compiler's unused-variable warning (the crate denies warnings) will tell you. If anything still uses `trimmed.to_uppercase()`, port it to the parsed AST.

- [ ] **Step 5: Build and run pool-adapter tests**

Run: `cargo build -p colmena_dag_engine 2>&1 | tail -20`
Expected: no warnings, no errors.

Run: `cargo test --lib -p colmena_dag_engine sql_pool_adapter`
Expected: existing tests pass. (Most pool-adapter tests need a real DB; they should be `#[ignore]` per the CLAUDE.md convention.)

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs
git commit -m "refactor(sql_pool): use sql_ast for is_select / LIMIT / DDL shape detection"
```

---

## Task 6: Migrate `SqlNode::extract_create_table_name` + post-execution RLS check

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`

- [ ] **Step 1: Replace `extract_create_table_name`**

Delete the entire `extract_create_table_name` function (lines ~230-248) and its `use std::sync::OnceLock;` if no other use remains.

The function is replaced by a single call site change in the next step — no shim needed.

- [ ] **Step 2: Replace post-CREATE TABLE RLS block**

Find the block at lines 389-407:

```rust
let trimmed_upper = query.trim_start().to_uppercase();
if trimmed_upper.starts_with("CREATE TABLE") && permissions.auto_rls() {
    if let Some((schema, table)) = Self::extract_create_table_name(query) {
        // ... apply RLS
    }
}
```

Replace with:

```rust
if permissions.auto_rls() {
    if let Ok(stmts) = crate::dag_engine::infrastructure::sql_ast::parse(query) {
        for stmt in &stmts {
            if let Some((schema, table)) =
                crate::dag_engine::infrastructure::sql_ast::created_table_name(stmt)
            {
                println!(
                    "[SqlNode] CREATE TABLE detected — applying RLS to {}.{}",
                    schema, table
                );
                if let Err(e) = adapter
                    .setup_rls_for_new_table(&schema, &table, permissions.tenant_column())
                    .await
                {
                    println!(
                        "[SqlNode] RLS setup warning for new table {}.{}: {}",
                        schema, table, e
                    );
                }
            }
        }
    }
}
```

- [ ] **Step 3: Build the crate**

Run: `cargo build -p colmena_dag_engine 2>&1 | tail -10`
Expected: no warnings, no errors. (Crate denies warnings; an unused import or dead `OnceLock` will fail.)

- [ ] **Step 4: Run tests touching the SQL node**

Run: `cargo test --lib -p colmena_dag_engine sql`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs
git commit -m "refactor(sql_node): replace CREATE TABLE regex with sql_ast walk"
```

---

## Task 7: End-to-end verification against a real Postgres

**Files:**
- No source changes. Manual verification only.

This task exercises the user's original failing flow against a live database to confirm the bug is gone in production code paths (not just unit tests).

- [ ] **Step 1: Source environment**

Run:
```bash
set -a; source .env; set +a
```

Confirm `DATABASE_URL` and the LLM API keys are set: `env | grep -E "DATABASE_URL|OPENAI_API_KEY"`.

- [ ] **Step 2: Pick or create a test graph**

Find an existing sql_query test graph: `ls tests/graphs/`. If none has `sql_query`, create a minimal one at `tests/graphs/agents/sql_insert_multirow.json` that:

- Wires a `trigger` node → a `sql_query` node with `permissions.preset = "full"`, `allowed_schemas` including a sandbox schema (e.g. `seb_data`), `sandbox_schema = "seb_data"`.
- Provides as input the user's original failing INSERT (single-row with `1.81`) plus a multi-row INSERT (10 rows) to confirm both paths.

- [ ] **Step 3: Run the graph**

Run:
```bash
cargo run --bin dag_engine -- run tests/graphs/agents/sql_insert_multirow.json \
  --agent-session-id sql_parser_test_001
```

Expected:
- The previously failing single-row INSERT with `1.81` now succeeds (no `"Access to schema '1' is not allowed"` error).
- The multi-row INSERT with 10 VALUES rows succeeds and the node returns `{ "rows_affected": 10 }`.

- [ ] **Step 4: Run regression check on `CREATE SCHEMA`**

Edit the test graph's query to `CREATE SCHEMA foo` and re-run. Expected output:

```json
{ "error": "BLOCKED by static validator (static_validator): CREATE SCHEMA is not supported. ...",
  "source": "static_validator" }
```

Confirms the new explicit policy message replaces the old "Could not determine SQL operation type."

- [ ] **Step 5: Run full test suite to confirm nothing else broke**

Run:
```bash
cargo test --verbose -p colmena_dag_engine 2>&1 | tail -30
```

Expected: all tests pass (or only pre-existing `#[ignore]`d ones skipped).

- [ ] **Step 6: Run clippy and fmt**

Run:
```bash
cargo clippy -p colmena_dag_engine -- -D warnings
cargo fmt --check
```

Expected: no warnings, no diff.

- [ ] **Step 7: Commit the test graph (if newly created)**

```bash
git add tests/graphs/agents/sql_insert_multirow.json
git commit -m "test(sql): add multirow INSERT graph covering 1.81 regression"
```

---

## Task 8: Document the new behavior

**Files:**
- Modify: `docs/developer_guide/23_sql_node.md`
- Modify: `CLAUDE.md` (one-line note in the "Current Status" section)

- [ ] **Step 1: Add a "Supported statement types" section to the SQL node guide**

Open `docs/developer_guide/23_sql_node.md` and find the section that lists permission presets / operations. Insert (after that section, before the "Sandbox Schema" section at line ~381):

```markdown
## Supported Statement Types

Queries are parsed with the PostgreSQL dialect of the `sqlparser` crate.
Each statement in a script (multi-statement queries are supported and validated
per-statement) must be one of:

| Operation | Notes |
|-----------|-------|
| `SELECT` / `WITH ... SELECT` | Subject to `max_rows`; auto-appends `LIMIT max_rows + 1` only when no explicit LIMIT is present. |
| `INSERT` | Multi-row VALUES and INSERT … SELECT both supported. No row-count cap (subject to `statement_timeout_ms`). |
| `UPDATE` | Requires a `WHERE` clause. |
| `DELETE` | Requires a `WHERE` clause. |
| `CREATE TABLE` | Inside `allowed_schemas` only. If `auto_rls` is on, RLS is applied post-creation. |
| `CREATE FUNCTION` | Must ship with a `COMMENT ON FUNCTION` statement in the same script. Registered in the function registry. |

Always blocked: `DROP`, `ALTER`, `TRUNCATE`, `CREATE SCHEMA`, `CREATE INDEX`,
`CREATE VIEW`, `GRANT`, `REVOKE`. Manage schema/index/view/grant lifecycle via
your migration tooling, not via the LLM.

If `sqlparser` cannot parse the query, the node returns a `Failed to parse SQL: …`
error and refuses to execute — there is no fallback to lenient string matching.
```

- [ ] **Step 2: Update CLAUDE.md status note**

In `CLAUDE.md`, under "Current Status", append a bullet:

```markdown
- **SQL node parser hardened 2026-05-26** — all regex/substring heuristics replaced by `sqlparser` AST analysis (`infrastructure/sql_ast.rs`). Fixes a false positive where decimal literals like `1.81` were misread as schema references. Multi-statement queries are now validated per-statement (closes a hole where `SELECT 1; DROP TABLE x;` slipped through). New DDL kinds (`CREATE SCHEMA`/`INDEX`/`VIEW`) are explicitly blocked with clear messages.
```

- [ ] **Step 3: Commit**

```bash
git add docs/developer_guide/23_sql_node.md CLAUDE.md
git commit -m "docs(sql): document sqlparser migration and supported statement types"
```

---

## Final Verification

- [ ] All tasks committed
- [ ] `cargo test --verbose -p colmena_dag_engine` passes
- [ ] `cargo clippy -p colmena_dag_engine -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] Original user bug (`1.81` in INSERT) reproduces no longer (Task 7 Step 3)
- [ ] Multi-row INSERT confirmed working (Task 7 Step 3)
- [ ] CLAUDE.md sweep against ADP worker: confirm no public API on the `SqlNode` / `SqlValidatorPort` / `SqlConnectionPort` traits changed. (None did — we only changed implementations. But run `cargo check` in `/Users/danielgarcia/startti/adp/apps/service/ia/platform/worker` after pointing it at this colmena rev as a paranoia check.)
