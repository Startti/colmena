# SQL Node (`sql_query`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a PostgreSQL-backed `sql_query` DAG node usable as an LLM tool, with granular permissions, hybrid validation (static rules + optional LLM critic), a sandbox function registry, and auto-injection of DB metadata into tool descriptions.

**Architecture:** Hexagonal — thin `SqlNode` orchestrator delegating to domain traits (`SqlConnectionPort`, `SqlValidatorPort`, `SqlCriticPort`, `FunctionRegistryPort`) with infrastructure adapters. New `InitializableNode` trait enables pre-execution setup (pool creation, metadata loading).

**Tech Stack:** Rust, sqlx (already in Cargo.toml), PostgreSQL, async-trait, serde, serde_json, regex

**Spec:** `docs/superpowers/specs/2026-04-17-sql-node-design.md`

---

## File Map

| File | Layer | Responsibility |
|------|-------|----------------|
| `dag_engine/domain/sql_permissions.rs` | Domain | `PermissionPreset`, `SqlOperation`, `SqlPermissions` struct with preset+deny resolution |
| `dag_engine/domain/sql_errors.rs` | Domain | `SqlNodeError` enum (Blocked, ValidationFailed, ConnectionError, CriticRejected) |
| `dag_engine/domain/sql_ports.rs` | Domain | Traits: `SqlConnectionPort`, `SqlValidatorPort`, `SqlCriticPort`, `FunctionRegistryPort` |
| `dag_engine/domain/initializable_node.rs` | Domain | `InitializableNode` trait + `InitContext` struct |
| `dag_engine/application/sql_execution_service.rs` | Application | Orchestrates validate → critic → execute → feedback pipeline |
| `dag_engine/infrastructure/nodes/sql.rs` | Infrastructure | `SqlNode` impl `ExecutableNode` + `InitializableNode` |
| `dag_engine/infrastructure/sql_pool_adapter.rs` | Infrastructure | `PgPoolAdapter` impl `SqlConnectionPort` — pool, runtime limits, query execution |
| `dag_engine/infrastructure/sql_static_validator.rs` | Infrastructure | `StaticRuleValidator` impl `SqlValidatorPort` — regex/pattern rules |
| `dag_engine/infrastructure/sql_llm_critic.rs` | Infrastructure | `LlmCriticAdapter` impl `SqlCriticPort` — delegates to LLM for security+optimization review |
| `dag_engine/infrastructure/sql_function_registry.rs` | Infrastructure | `PgRegistryAdapter` impl `FunctionRegistryPort` — CRUD on `sandbox.function_registry` |

**Files to modify:**
| File | Change |
|------|--------|
| `dag_engine/domain/mod.rs` | Add `pub mod sql_permissions; pub mod sql_errors; pub mod sql_ports; pub mod initializable_node;` |
| `dag_engine/application/mod.rs` | Add `pub mod sql_execution_service;` |
| `dag_engine/infrastructure/nodes/mod.rs` | Add `pub mod sql;` |
| `dag_engine/infrastructure/registry.rs` | Register `SqlNode` as `"sql_query"` |
| `dag_engine/domain/observer.rs` | Add `SqlValidation` and `SqlCriticResult` variants to `NodeEvent` |

---

## Task 1: Domain — Permissions Model

**Files:**
- Create: `src/libs/colmena/src/dag_engine/domain/sql_permissions.rs`
- Modify: `src/libs/colmena/src/dag_engine/domain/mod.rs`

- [ ] **Step 1: Write the test for permission preset resolution**

In `sql_permissions.rs`, add an inline test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_only_preset() {
        let perms = SqlPermissions::from_config(None).unwrap();
        assert!(perms.is_allowed(&SqlOperation::Select));
        assert!(!perms.is_allowed(&SqlOperation::Insert));
        assert!(!perms.is_allowed(&SqlOperation::Update));
        assert!(!perms.is_allowed(&SqlOperation::Delete));
        assert!(!perms.is_allowed(&SqlOperation::CreateFunction));
    }

    #[test]
    fn test_read_write_preset() {
        let config = serde_json::json!({ "preset": "read_write" });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert!(perms.is_allowed(&SqlOperation::Select));
        assert!(perms.is_allowed(&SqlOperation::Insert));
        assert!(perms.is_allowed(&SqlOperation::Update));
        assert!(!perms.is_allowed(&SqlOperation::Delete));
        assert!(!perms.is_allowed(&SqlOperation::CreateFunction));
    }

    #[test]
    fn test_full_preset_with_deny() {
        let config = serde_json::json!({
            "preset": "full",
            "deny": ["delete"]
        });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert!(perms.is_allowed(&SqlOperation::Select));
        assert!(perms.is_allowed(&SqlOperation::Insert));
        assert!(perms.is_allowed(&SqlOperation::Update));
        assert!(!perms.is_allowed(&SqlOperation::Delete));
        assert!(perms.is_allowed(&SqlOperation::CreateFunction));
    }

    #[test]
    fn test_truncate_always_blocked() {
        let config = serde_json::json!({ "preset": "full" });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert!(!perms.is_allowed(&SqlOperation::Truncate));
    }

    #[test]
    fn test_allowed_schemas() {
        let config = serde_json::json!({
            "preset": "read_only",
            "allowed_schemas": ["production", "analytics"]
        });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert!(perms.is_schema_allowed("production"));
        assert!(perms.is_schema_allowed("analytics"));
        assert!(!perms.is_schema_allowed("secret_data"));
        // information_schema and pg_catalog always allowed (introspection)
        assert!(perms.is_schema_allowed("information_schema"));
        assert!(perms.is_schema_allowed("pg_catalog"));
    }

    #[test]
    fn test_sandbox_schema_defaults() {
        let config = serde_json::json!({
            "preset": "full",
            "allowed_schemas": ["production", "sandbox"]
        });
        let perms = SqlPermissions::from_config(Some(&config)).unwrap();
        assert_eq!(perms.sandbox_schema(), "sandbox");
    }

    #[test]
    fn test_no_config_defaults_read_only() {
        let perms = SqlPermissions::from_config(None).unwrap();
        assert!(perms.is_allowed(&SqlOperation::Select));
        assert!(!perms.is_allowed(&SqlOperation::Insert));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib sql_permissions -- --nocapture 2>&1 | head -30`
Expected: FAIL — module doesn't exist yet.

- [ ] **Step 3: Implement the permissions model**

```rust
//! Granular permission model for the SQL node.
//!
//! Permissions are configured via presets (`read_only`, `read_write`, `full`) with an
//! optional `deny` list for fine-tuning. When no permissions config is provided, defaults
//! to `read_only` (principle of least privilege).

use serde_json::Value;
use std::collections::HashSet;

/// SQL operations that can be allowed or denied.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SqlOperation {
    Select,
    Insert,
    Update,
    Delete,
    CreateFunction,
    /// Always blocked — no preset enables this.
    Truncate,
    /// Always blocked on protected schemas.
    Drop,
    /// Always blocked on protected schemas.
    Alter,
}

impl SqlOperation {
    /// Parse an operation name from a string (used for `deny` list parsing).
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "select" => Some(Self::Select),
            "insert" => Some(Self::Insert),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            "create_function" => Some(Self::CreateFunction),
            "truncate" => Some(Self::Truncate),
            "drop" => Some(Self::Drop),
            "alter" => Some(Self::Alter),
            _ => None,
        }
    }
}

/// Permission presets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionPreset {
    ReadOnly,
    ReadWrite,
    Full,
}

impl PermissionPreset {
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "read_only" => Ok(Self::ReadOnly),
            "read_write" => Ok(Self::ReadWrite),
            "full" => Ok(Self::Full),
            other => Err(format!("Unknown permission preset: '{}'", other)),
        }
    }

    fn allowed_operations(&self) -> HashSet<SqlOperation> {
        match self {
            Self::ReadOnly => {
                let mut set = HashSet::new();
                set.insert(SqlOperation::Select);
                set
            }
            Self::ReadWrite => {
                let mut set = HashSet::new();
                set.insert(SqlOperation::Select);
                set.insert(SqlOperation::Insert);
                set.insert(SqlOperation::Update);
                set
            }
            Self::Full => {
                let mut set = HashSet::new();
                set.insert(SqlOperation::Select);
                set.insert(SqlOperation::Insert);
                set.insert(SqlOperation::Update);
                set.insert(SqlOperation::Delete);
                set.insert(SqlOperation::CreateFunction);
                set
            }
        }
    }
}

/// Resolved permissions for a SQL node instance.
#[derive(Debug, Clone)]
pub struct SqlPermissions {
    allowed_ops: HashSet<SqlOperation>,
    allowed_schemas: HashSet<String>,
    sandbox_schema: String,
}

/// Schemas that are always accessible for introspection (not configurable).
const INTROSPECTION_SCHEMAS: &[&str] = &["information_schema", "pg_catalog"];

impl SqlPermissions {
    /// Build permissions from the JSON config `permissions` object.
    /// If `config` is `None`, defaults to `read_only` with no schema restrictions.
    pub fn from_config(config: Option<&Value>) -> Result<Self, String> {
        let config = match config {
            Some(c) => c,
            None => {
                return Ok(Self {
                    allowed_ops: PermissionPreset::ReadOnly.allowed_operations(),
                    allowed_schemas: HashSet::new(),
                    sandbox_schema: "sandbox".to_string(),
                });
            }
        };

        // Parse preset (default: read_only)
        let preset_str = config
            .get("preset")
            .and_then(|v| v.as_str())
            .unwrap_or("read_only");
        let preset = PermissionPreset::from_str(preset_str)?;
        let mut allowed_ops = preset.allowed_operations();

        // Apply deny list
        if let Some(deny_arr) = config.get("deny").and_then(|v| v.as_array()) {
            for deny_val in deny_arr {
                if let Some(deny_str) = deny_val.as_str() {
                    if let Some(op) = SqlOperation::from_str_loose(deny_str) {
                        allowed_ops.remove(&op);
                    }
                }
            }
        }

        // Parse allowed_schemas
        let allowed_schemas: HashSet<String> = config
            .get("allowed_schemas")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Parse sandbox_schema (default: "sandbox")
        let sandbox_schema = config
            .get("sandbox_schema")
            .and_then(|v| v.as_str())
            .unwrap_or("sandbox")
            .to_string();

        Ok(Self {
            allowed_ops,
            allowed_schemas,
            sandbox_schema,
        })
    }

    /// Check if an operation is allowed.
    pub fn is_allowed(&self, op: &SqlOperation) -> bool {
        // Truncate, Drop, Alter are never directly allowed via presets
        match op {
            SqlOperation::Truncate => false,
            _ => self.allowed_ops.contains(op),
        }
    }

    /// Check if a schema is accessible.
    /// Introspection schemas (information_schema, pg_catalog) are always allowed.
    /// If `allowed_schemas` is empty, all schemas are allowed (no restriction).
    pub fn is_schema_allowed(&self, schema: &str) -> bool {
        if INTROSPECTION_SCHEMAS.contains(&schema) {
            return true;
        }
        if self.allowed_schemas.is_empty() {
            return true;
        }
        self.allowed_schemas.contains(schema)
    }

    /// The sandbox schema name where the agent can create functions/tables.
    pub fn sandbox_schema(&self) -> &str {
        &self.sandbox_schema
    }

    /// Return a human-readable summary for LLM context injection.
    pub fn describe_for_llm(&self) -> String {
        let ops: Vec<&str> = [
            (SqlOperation::Select, "SELECT"),
            (SqlOperation::Insert, "INSERT"),
            (SqlOperation::Update, "UPDATE"),
            (SqlOperation::Delete, "DELETE"),
            (SqlOperation::CreateFunction, "CREATE FUNCTION"),
        ]
        .iter()
        .filter(|(op, _)| self.allowed_ops.contains(op))
        .map(|(_, name)| *name)
        .collect();

        format!("Permissions: {} | Schemas: {}",
            ops.join(", "),
            if self.allowed_schemas.is_empty() {
                "all".to_string()
            } else {
                self.allowed_schemas.iter().cloned().collect::<Vec<_>>().join(", ")
            }
        )
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib sql_permissions -- --nocapture`
Expected: All 7 tests PASS.

- [ ] **Step 5: Register the module**

In `src/libs/colmena/src/dag_engine/domain/mod.rs`, add:
```rust
pub mod sql_permissions;
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check 2>&1 | tail -5`
Expected: No errors.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/sql_permissions.rs \
        src/libs/colmena/src/dag_engine/domain/mod.rs
git commit -m "feat(sql_node): add SqlPermissions domain model with presets and deny override"
```

---

## Task 2: Domain — Error Types

**Files:**
- Create: `src/libs/colmena/src/dag_engine/domain/sql_errors.rs`
- Modify: `src/libs/colmena/src/dag_engine/domain/mod.rs`

- [ ] **Step 1: Write the error enum**

```rust
//! Error types for the SQL node validation and execution pipeline.

use std::fmt;

/// Errors produced by the SQL node pipeline (validation, critic, execution).
#[derive(Debug)]
pub enum SqlNodeError {
    /// Query blocked by static validator rules.
    Blocked {
        rule: String,
        message: String,
    },
    /// Query blocked by LLM critic (security concern).
    CriticRejected {
        reason: String,
    },
    /// Could not connect to PostgreSQL or pool creation failed.
    ConnectionError(String),
    /// Query execution failed at the PostgreSQL level.
    ExecutionError(String),
    /// Permission configuration is invalid.
    ConfigError(String),
}

impl fmt::Display for SqlNodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked { rule, message } => {
                write!(f, "BLOCKED by static validator ({}): {}", rule, message)
            }
            Self::CriticRejected { reason } => {
                write!(f, "BLOCKED by LLM critic: {}", reason)
            }
            Self::ConnectionError(msg) => write!(f, "SQL connection error: {}", msg),
            Self::ExecutionError(msg) => write!(f, "SQL execution error: {}", msg),
            Self::ConfigError(msg) => write!(f, "SQL config error: {}", msg),
        }
    }
}

impl std::error::Error for SqlNodeError {}
```

- [ ] **Step 2: Register the module**

In `src/libs/colmena/src/dag_engine/domain/mod.rs`, add:
```rust
pub mod sql_errors;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check 2>&1 | tail -5`
Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/sql_errors.rs \
        src/libs/colmena/src/dag_engine/domain/mod.rs
git commit -m "feat(sql_node): add SqlNodeError domain error types"
```

---

## Task 3: Domain — Ports (Traits)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/domain/sql_ports.rs`
- Modify: `src/libs/colmena/src/dag_engine/domain/mod.rs`

- [ ] **Step 1: Define the four port traits**

```rust
//! Domain ports (traits) for the SQL node's hexagonal architecture.
//!
//! Each trait defines a capability boundary. Infrastructure adapters implement
//! these traits; the application service and node depend only on the traits.

use crate::dag_engine::domain::sql_errors::SqlNodeError;
use crate::dag_engine::domain::sql_permissions::SqlPermissions;
use serde_json::Value;
use std::collections::HashMap;

/// Result of static or LLM validation.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether the query is allowed to execute.
    pub allowed: bool,
    /// If blocked, the reason.
    pub block_reason: Option<String>,
    /// Non-blocking warnings (e.g., "SELECT * detected").
    pub warnings: Vec<String>,
}

/// Result of LLM critic analysis.
#[derive(Debug, Clone)]
pub struct CriticResult {
    /// Security assessment: true = safe, false = blocked.
    pub security_ok: bool,
    /// If blocked, explanation from the LLM.
    pub security_reason: Option<String>,
    /// Optimization suggestions (non-blocking).
    pub optimization_hints: Vec<String>,
}

/// Metadata about a registered function in the sandbox.
#[derive(Debug, Clone)]
pub struct FunctionInfo {
    pub function_name: String,
    pub schema_name: String,
    pub parameters: Option<String>,
    pub return_type: Option<String>,
    pub description: String,
}

/// Metadata about a table (name + optional comment).
#[derive(Debug, Clone)]
pub struct TableInfo {
    pub schema_name: String,
    pub table_name: String,
    pub description: Option<String>,
}

/// Port for managing the PostgreSQL connection pool and executing queries.
#[async_trait::async_trait]
pub trait SqlConnectionPort: Send + Sync {
    /// Create a connection pool and apply runtime limits.
    async fn connect(
        &self,
        connection_url: &str,
        statement_timeout_ms: u64,
        work_mem_mb: u64,
    ) -> Result<(), SqlNodeError>;

    /// Execute a SQL query and return results as JSON.
    /// For SELECT: returns array of row objects.
    /// For INSERT/UPDATE/DELETE: returns `{ "rows_affected": N }`.
    /// For CREATE FUNCTION: returns `{ "created": true, "name": "...", "schema": "..." }`.
    async fn execute_query(
        &self,
        query: &str,
        max_rows: u64,
    ) -> Result<QueryResult, SqlNodeError>;

    /// Load table metadata (names + comments) for the given schemas.
    async fn load_table_metadata(
        &self,
        schemas: &[String],
    ) -> Result<Vec<TableInfo>, SqlNodeError>;

    /// Check if the pool is connected and ready.
    fn is_connected(&self) -> bool;
}

/// Result of a SQL query execution.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// The result data (rows array, or mutation result object).
    pub output: Value,
    /// Number of rows returned or affected.
    pub row_count: u64,
    /// Whether the result was truncated due to max_rows.
    pub truncated: bool,
}

/// Port for static SQL validation rules.
pub trait SqlValidatorPort: Send + Sync {
    /// Validate a SQL query against static rules and permissions.
    fn validate(
        &self,
        query: &str,
        permissions: &SqlPermissions,
    ) -> ValidationResult;
}

/// Port for LLM-based SQL critic (optional, activated by config flag).
#[async_trait::async_trait]
pub trait SqlCriticPort: Send + Sync {
    /// Analyze a SQL query for security risks and optimization opportunities.
    async fn analyze(
        &self,
        query: &str,
        schema_context: &str,
    ) -> Result<CriticResult, SqlNodeError>;
}

/// Port for managing the function registry in the sandbox schema.
#[async_trait::async_trait]
pub trait FunctionRegistryPort: Send + Sync {
    /// Ensure the sandbox schema and registry tables exist.
    async fn ensure_schema(&self) -> Result<(), SqlNodeError>;

    /// Register a newly created function.
    async fn register_function(
        &self,
        info: &FunctionInfo,
        session_id: &str,
    ) -> Result<(), SqlNodeError>;

    /// Load all registered functions.
    async fn list_functions(&self) -> Result<Vec<FunctionInfo>, SqlNodeError>;

    /// Record a feedback entry (warning or optimization hint).
    async fn record_feedback(
        &self,
        session_id: &str,
        query: &str,
        feedback_type: &str,
        source: &str,
        message: &str,
    ) -> Result<(), SqlNodeError>;
}
```

- [ ] **Step 2: Register the module**

In `src/libs/colmena/src/dag_engine/domain/mod.rs`, add:
```rust
pub mod sql_ports;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check 2>&1 | tail -5`
Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/sql_ports.rs \
        src/libs/colmena/src/dag_engine/domain/mod.rs
git commit -m "feat(sql_node): add domain port traits for SQL node services"
```

---

## Task 4: Domain — InitializableNode Trait

**Files:**
- Create: `src/libs/colmena/src/dag_engine/domain/initializable_node.rs`
- Modify: `src/libs/colmena/src/dag_engine/domain/mod.rs`

- [ ] **Step 1: Define the trait and context struct**

```rust
//! Optional initialization trait for nodes that need pre-execution setup.
//!
//! Nodes that implement this trait get `initialize()` called once before the first
//! execution within a DAG run. Use this for creating connection pools, loading metadata,
//! or any expensive one-time setup.

use serde_json::Value;
use std::error::Error as StdError;

/// Context returned by `InitializableNode::initialize()`.
/// Contains metadata that can enrich the tool description sent to the LLM.
#[derive(Debug, Clone, Default)]
pub struct InitContext {
    /// Additional text to append to the tool's description.
    /// Used to inject database schema info, available functions, etc.
    pub description_supplement: Option<String>,
}

/// Optional trait for nodes that require one-time initialization before execution.
///
/// The DAG engine checks if a node implements this trait (via downcast) and calls
/// `initialize()` once before the first `execute()` call in a given DAG run.
#[async_trait::async_trait]
pub trait InitializableNode: Send + Sync {
    /// Perform one-time setup. Called before the first `execute()`.
    ///
    /// # Arguments
    /// * `config` - The node's static configuration from the graph JSON.
    ///
    /// # Returns
    /// An `InitContext` whose `description_supplement` is appended to the tool description.
    async fn initialize(
        &self,
        config: &Value,
    ) -> Result<InitContext, Box<dyn StdError + Send + Sync>>;
}
```

- [ ] **Step 2: Register the module**

In `src/libs/colmena/src/dag_engine/domain/mod.rs`, add:
```rust
pub mod initializable_node;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check 2>&1 | tail -5`
Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/initializable_node.rs \
        src/libs/colmena/src/dag_engine/domain/mod.rs
git commit -m "feat(sql_node): add InitializableNode trait for pre-execution setup"
```

---

## Task 5: Infrastructure — Static Validator

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs`

- [ ] **Step 1: Write tests for the static validator**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::sql_permissions::SqlPermissions;

    fn read_only_perms() -> SqlPermissions {
        SqlPermissions::from_config(Some(&serde_json::json!({
            "preset": "read_only",
            "allowed_schemas": ["production"]
        }))).unwrap()
    }

    fn full_perms() -> SqlPermissions {
        SqlPermissions::from_config(Some(&serde_json::json!({
            "preset": "full",
            "allowed_schemas": ["production", "sandbox"],
            "sandbox_schema": "sandbox"
        }))).unwrap()
    }

    #[test]
    fn test_select_allowed() {
        let v = StaticRuleValidator;
        let r = v.validate("SELECT id, name FROM production.users WHERE id = 1", &read_only_perms());
        assert!(r.allowed);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn test_select_star_warns() {
        let v = StaticRuleValidator;
        let r = v.validate("SELECT * FROM production.users WHERE id = 1", &read_only_perms());
        assert!(r.allowed);
        assert!(r.warnings.iter().any(|w| w.contains("SELECT *")));
    }

    #[test]
    fn test_insert_blocked_on_read_only() {
        let v = StaticRuleValidator;
        let r = v.validate("INSERT INTO production.users (name) VALUES ('test')", &read_only_perms());
        assert!(!r.allowed);
        assert!(r.block_reason.unwrap().contains("INSERT"));
    }

    #[test]
    fn test_delete_without_where_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate("DELETE FROM production.orders", &full_perms());
        assert!(!r.allowed);
        assert!(r.block_reason.unwrap().contains("WHERE"));
    }

    #[test]
    fn test_delete_with_where_allowed() {
        let v = StaticRuleValidator;
        let r = v.validate("DELETE FROM production.orders WHERE id = 5", &full_perms());
        assert!(r.allowed);
    }

    #[test]
    fn test_update_without_where_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate("UPDATE production.orders SET status = 'done'", &full_perms());
        assert!(!r.allowed);
        assert!(r.block_reason.unwrap().contains("WHERE"));
    }

    #[test]
    fn test_truncate_always_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate("TRUNCATE TABLE production.orders", &full_perms());
        assert!(!r.allowed);
        assert!(r.block_reason.unwrap().contains("TRUNCATE"));
    }

    #[test]
    fn test_drop_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate("DROP TABLE production.users", &full_perms());
        assert!(!r.allowed);
    }

    #[test]
    fn test_schema_not_allowed() {
        let v = StaticRuleValidator;
        let r = v.validate("SELECT * FROM secret.passwords", &read_only_perms());
        assert!(!r.allowed);
        assert!(r.block_reason.unwrap().contains("schema"));
    }

    #[test]
    fn test_introspection_always_allowed() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'production'",
            &read_only_perms(),
        );
        assert!(r.allowed);
    }

    #[test]
    fn test_create_function_without_comment_blocked() {
        let v = StaticRuleValidator;
        let r = v.validate(
            "CREATE FUNCTION sandbox.my_func() RETURNS void AS $$ BEGIN END; $$ LANGUAGE plpgsql",
            &full_perms(),
        );
        assert!(!r.allowed);
        assert!(r.block_reason.unwrap().contains("COMMENT"));
    }

    #[test]
    fn test_create_function_with_comment_allowed() {
        let v = StaticRuleValidator;
        let query = "CREATE FUNCTION sandbox.my_func() RETURNS void AS $$ BEGIN END; $$ LANGUAGE plpgsql; COMMENT ON FUNCTION sandbox.my_func() IS 'Does something'";
        let r = v.validate(query, &full_perms());
        assert!(r.allowed);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib sql_static_validator -- --nocapture 2>&1 | head -15`
Expected: FAIL — module doesn't exist.

- [ ] **Step 3: Implement the static validator**

```rust
//! Static rule validator for SQL queries.
//!
//! Validates queries against permissions and pattern-based safety rules.
//! All checks run synchronously in <1ms with zero external dependencies.

use crate::dag_engine::domain::sql_permissions::{SqlOperation, SqlPermissions};
use crate::dag_engine::domain::sql_ports::{SqlValidatorPort, ValidationResult};
use regex::Regex;

/// Stateless validator that checks SQL queries against static rules.
pub struct StaticRuleValidator;

impl StaticRuleValidator {
    /// Detect the primary SQL operation from a query string.
    fn detect_operation(query: &str) -> Option<SqlOperation> {
        let trimmed = query.trim_start();
        let upper = trimmed.to_uppercase();

        if upper.starts_with("SELECT") {
            Some(SqlOperation::Select)
        } else if upper.starts_with("INSERT") {
            Some(SqlOperation::Insert)
        } else if upper.starts_with("UPDATE") {
            Some(SqlOperation::Update)
        } else if upper.starts_with("DELETE") {
            Some(SqlOperation::Delete)
        } else if upper.starts_with("CREATE FUNCTION") || upper.starts_with("CREATE OR REPLACE FUNCTION") {
            Some(SqlOperation::CreateFunction)
        } else if upper.starts_with("TRUNCATE") {
            Some(SqlOperation::Truncate)
        } else if upper.starts_with("DROP") {
            Some(SqlOperation::Drop)
        } else if upper.starts_with("ALTER") {
            Some(SqlOperation::Alter)
        } else {
            None
        }
    }

    /// Extract schema references from the query (simple heuristic: `schema.table` patterns).
    fn extract_schemas(query: &str) -> Vec<String> {
        let re = Regex::new(r"(?i)\b(\w+)\.(\w+)").unwrap();
        let mut schemas = Vec::new();
        for cap in re.captures_iter(query) {
            let schema = cap[1].to_lowercase();
            // Filter out common false positives
            if !["pg_catalog", "information_schema"].contains(&schema.as_str()) || true {
                schemas.push(schema);
            }
        }
        schemas.sort();
        schemas.dedup();
        schemas
    }

    /// Check if query contains a WHERE clause (for DELETE/UPDATE safety).
    fn has_where_clause(query: &str) -> bool {
        let upper = query.to_uppercase();
        upper.contains("WHERE")
    }

    /// Check if a CREATE statement includes a COMMENT ON statement.
    fn has_comment(query: &str) -> bool {
        let upper = query.to_uppercase();
        upper.contains("COMMENT ON")
    }
}

impl SqlValidatorPort for StaticRuleValidator {
    fn validate(
        &self,
        query: &str,
        permissions: &SqlPermissions,
    ) -> ValidationResult {
        let mut warnings: Vec<String> = Vec::new();

        // 1. Detect operation
        let operation = match Self::detect_operation(query) {
            Some(op) => op,
            None => {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some("Could not determine SQL operation type. Only SELECT, INSERT, UPDATE, DELETE, and CREATE FUNCTION are supported.".to_string()),
                    warnings: vec![],
                };
            }
        };

        // 2. Check if operation is always blocked
        match &operation {
            SqlOperation::Truncate => {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some("TRUNCATE is not allowed. Use DELETE with a WHERE clause instead.".to_string()),
                    warnings: vec![],
                };
            }
            SqlOperation::Drop => {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some("DROP is not allowed. You can only create objects in the sandbox schema.".to_string()),
                    warnings: vec![],
                };
            }
            SqlOperation::Alter => {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some("ALTER is not allowed on any schema.".to_string()),
                    warnings: vec![],
                };
            }
            _ => {}
        }

        // 3. Check permission for this operation
        if !permissions.is_allowed(&operation) {
            return ValidationResult {
                allowed: false,
                block_reason: Some(format!(
                    "{:?} is not permitted by the current permission preset. Allowed operations: {}",
                    operation,
                    permissions.describe_for_llm()
                )),
                warnings: vec![],
            };
        }

        // 4. Check schema access
        let schemas = Self::extract_schemas(query);
        for schema in &schemas {
            if !permissions.is_schema_allowed(schema) {
                return ValidationResult {
                    allowed: false,
                    block_reason: Some(format!(
                        "Access to schema '{}' is not allowed. Allowed schemas: check your permissions config.",
                        schema
                    )),
                    warnings: vec![],
                };
            }
        }

        // 5. DELETE/UPDATE without WHERE
        if matches!(operation, SqlOperation::Delete | SqlOperation::Update)
            && !Self::has_where_clause(query)
        {
            return ValidationResult {
                allowed: false,
                block_reason: Some(format!(
                    "{:?} without a WHERE clause is not allowed. Specify which rows to affect.",
                    operation
                )),
                warnings: vec![],
            };
        }

        // 6. CREATE without COMMENT
        if matches!(operation, SqlOperation::CreateFunction) && !Self::has_comment(query) {
            return ValidationResult {
                allowed: false,
                block_reason: Some(
                    "CREATE FUNCTION requires a COMMENT ON FUNCTION statement describing the function's purpose. Include it in the same query.".to_string()
                ),
                warnings: vec![],
            };
        }

        // 7. Warnings (non-blocking)
        let upper = query.to_uppercase();
        if upper.contains("SELECT *") || upper.contains("SELECT  *") {
            warnings.push(
                "Prefer selecting specific columns instead of SELECT * to reduce data transfer and improve clarity.".to_string()
            );
        }

        ValidationResult {
            allowed: true,
            block_reason: None,
            warnings,
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib sql_static_validator -- --nocapture`
Expected: All 12 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs
git commit -m "feat(sql_node): implement StaticRuleValidator with blocking and warning rules"
```

---

## Task 6: Infrastructure — PostgreSQL Pool Adapter

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs`

- [ ] **Step 1: Implement the pool adapter**

```rust
//! PostgreSQL connection pool adapter.
//!
//! Wraps `sqlx::PgPool` and implements `SqlConnectionPort`. Manages connection pooling,
//! runtime limits (`statement_timeout`, `work_mem`), and query execution with row capping.

use crate::dag_engine::domain::sql_errors::SqlNodeError;
use crate::dag_engine::domain::sql_ports::{QueryResult, SqlConnectionPort, TableInfo};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Adapter that manages a PostgreSQL connection pool.
pub struct PgPoolAdapter {
    pool: Arc<RwLock<Option<PgPool>>>,
    statement_timeout_ms: Arc<RwLock<u64>>,
    work_mem_mb: Arc<RwLock<u64>>,
}

impl PgPoolAdapter {
    pub fn new() -> Self {
        Self {
            pool: Arc::new(RwLock::new(None)),
            statement_timeout_ms: Arc::new(RwLock::new(30_000)),
            work_mem_mb: Arc::new(RwLock::new(64)),
        }
    }

    /// Get a reference to the pool, applying runtime limits on checkout.
    async fn get_pool(&self) -> Result<PgPool, SqlNodeError> {
        let guard = self.pool.read().await;
        guard.clone().ok_or_else(|| {
            SqlNodeError::ConnectionError("Pool not initialized. Call connect() first.".to_string())
        })
    }
}

#[async_trait::async_trait]
impl SqlConnectionPort for PgPoolAdapter {
    async fn connect(
        &self,
        connection_url: &str,
        statement_timeout_ms: u64,
        work_mem_mb: u64,
    ) -> Result<(), SqlNodeError> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(connection_url)
            .await
            .map_err(|e| SqlNodeError::ConnectionError(format!("Failed to create pool: {}", e)))?;

        // Apply runtime limits to verify connectivity
        sqlx::query(&format!("SET statement_timeout = {}", statement_timeout_ms))
            .execute(&pool)
            .await
            .map_err(|e| {
                SqlNodeError::ConnectionError(format!("Failed to set statement_timeout: {}", e))
            })?;

        sqlx::query(&format!("SET work_mem = '{}MB'", work_mem_mb))
            .execute(&pool)
            .await
            .map_err(|e| {
                SqlNodeError::ConnectionError(format!("Failed to set work_mem: {}", e))
            })?;

        *self.pool.write().await = Some(pool);
        *self.statement_timeout_ms.write().await = statement_timeout_ms;
        *self.work_mem_mb.write().await = work_mem_mb;
        Ok(())
    }

    async fn execute_query(
        &self,
        query: &str,
        max_rows: u64,
    ) -> Result<QueryResult, SqlNodeError> {
        let pool = self.get_pool().await?;
        let timeout_ms = *self.statement_timeout_ms.read().await;
        let work_mem = *self.work_mem_mb.read().await;

        // Apply runtime limits per query via a transaction-scoped SET LOCAL
        let trimmed = query.trim_start().to_uppercase();
        let is_select = trimmed.starts_with("SELECT")
            || trimmed.starts_with("WITH"); // CTEs

        if is_select {
            // For SELECT: wrap with LIMIT to enforce max_rows, use raw query to get JSON rows
            let limited_query = if max_rows > 0
                && !trimmed.contains("LIMIT")
            {
                // Add LIMIT max_rows + 1 to detect truncation
                format!(
                    "SET LOCAL statement_timeout = {}; SET LOCAL work_mem = '{}MB'; {} LIMIT {}",
                    timeout_ms,
                    work_mem,
                    query.trim_end_matches(';'),
                    max_rows + 1
                )
            } else {
                format!(
                    "SET LOCAL statement_timeout = {}; SET LOCAL work_mem = '{}MB'; {}",
                    timeout_ms, work_mem, query
                )
            };

            // Execute and collect rows as JSON
            let rows = sqlx::query(&limited_query)
                .fetch_all(&pool)
                .await
                .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;

            let mut json_rows: Vec<Value> = Vec::new();
            for row in &rows {
                let mut obj = serde_json::Map::new();
                for col in row.columns() {
                    let name = col.name();
                    let val: Value = match col.type_info().to_string().as_str() {
                        "INT4" | "INT8" | "INT2" => {
                            row.try_get::<i64, _>(name)
                                .map(|v| json!(v))
                                .unwrap_or(Value::Null)
                        }
                        "FLOAT4" | "FLOAT8" | "NUMERIC" => {
                            row.try_get::<f64, _>(name)
                                .map(|v| json!(v))
                                .unwrap_or(Value::Null)
                        }
                        "BOOL" => {
                            row.try_get::<bool, _>(name)
                                .map(|v| json!(v))
                                .unwrap_or(Value::Null)
                        }
                        _ => {
                            // Default: try as string
                            row.try_get::<String, _>(name)
                                .map(|v| json!(v))
                                .unwrap_or(Value::Null)
                        }
                    };
                    obj.insert(name.to_string(), val);
                }
                json_rows.push(Value::Object(obj));
            }

            let truncated = max_rows > 0 && json_rows.len() as u64 > max_rows;
            if truncated {
                json_rows.truncate(max_rows as usize);
            }

            let row_count = json_rows.len() as u64;
            Ok(QueryResult {
                output: Value::Array(json_rows),
                row_count,
                truncated,
            })
        } else {
            // For mutations: execute and return rows_affected
            let result = sqlx::query(query)
                .execute(&pool)
                .await
                .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;

            let rows_affected = result.rows_affected();

            // Detect CREATE FUNCTION
            if trimmed.starts_with("CREATE FUNCTION")
                || trimmed.starts_with("CREATE OR REPLACE FUNCTION")
            {
                Ok(QueryResult {
                    output: json!({ "created": true }),
                    row_count: 0,
                    truncated: false,
                })
            } else {
                Ok(QueryResult {
                    output: json!({ "rows_affected": rows_affected }),
                    row_count: rows_affected,
                    truncated: false,
                })
            }
        }
    }

    async fn load_table_metadata(
        &self,
        schemas: &[String],
    ) -> Result<Vec<TableInfo>, SqlNodeError> {
        let pool = self.get_pool().await?;

        if schemas.is_empty() {
            return Ok(vec![]);
        }

        // Build parameterized query for schema list
        let placeholders: Vec<String> = schemas.iter().enumerate().map(|(i, _)| format!("${}", i + 1)).collect();
        let query = format!(
            "SELECT t.table_schema, t.table_name, \
             pg_catalog.obj_description(c.oid) as description \
             FROM information_schema.tables t \
             LEFT JOIN pg_catalog.pg_class c ON c.relname = t.table_name \
             LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace AND n.nspname = t.table_schema \
             WHERE t.table_schema IN ({}) \
             AND t.table_type = 'BASE TABLE' \
             ORDER BY t.table_schema, t.table_name",
            placeholders.join(", ")
        );

        let mut q = sqlx::query(&query);
        for schema in schemas {
            q = q.bind(schema);
        }

        let rows = q.fetch_all(&pool).await.map_err(|e| {
            SqlNodeError::ExecutionError(format!("Failed to load table metadata: {}", e))
        })?;

        let mut tables = Vec::new();
        for row in rows {
            tables.push(TableInfo {
                schema_name: row.try_get("table_schema").unwrap_or_default(),
                table_name: row.try_get("table_name").unwrap_or_default(),
                description: row.try_get("description").ok(),
            });
        }
        Ok(tables)
    }

    fn is_connected(&self) -> bool {
        // Use try_read to avoid blocking
        self.pool
            .try_read()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check 2>&1 | tail -10`
Expected: No errors. (Integration tests require a real PostgreSQL — unit tests are in later tasks.)

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs
git commit -m "feat(sql_node): implement PgPoolAdapter with connection pooling and query execution"
```

---

## Task 7: Infrastructure — Function Registry Adapter

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/sql_function_registry.rs`

- [ ] **Step 1: Implement the registry adapter**

```rust
//! PostgreSQL-backed function registry for the sandbox schema.
//!
//! Manages `sandbox.function_registry` and `sandbox.query_feedback` tables.
//! Tables are created automatically on first use via `ensure_schema()`.

use crate::dag_engine::domain::sql_errors::SqlNodeError;
use crate::dag_engine::domain::sql_ports::{FunctionInfo, FunctionRegistryPort};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Adapter that stores function metadata and query feedback in PostgreSQL.
pub struct PgRegistryAdapter {
    pool: Arc<RwLock<Option<PgPool>>>,
    sandbox_schema: String,
}

impl PgRegistryAdapter {
    pub fn new(pool: Arc<RwLock<Option<PgPool>>>, sandbox_schema: String) -> Self {
        Self {
            pool,
            sandbox_schema,
        }
    }

    async fn get_pool(&self) -> Result<PgPool, SqlNodeError> {
        let guard = self.pool.read().await;
        guard.clone().ok_or_else(|| {
            SqlNodeError::ConnectionError("Pool not initialized.".to_string())
        })
    }
}

#[async_trait::async_trait]
impl FunctionRegistryPort for PgRegistryAdapter {
    async fn ensure_schema(&self) -> Result<(), SqlNodeError> {
        let pool = self.get_pool().await?;

        let schema = &self.sandbox_schema;

        let ddl = format!(
            r#"
            CREATE SCHEMA IF NOT EXISTS {schema};

            CREATE TABLE IF NOT EXISTS {schema}.function_registry (
                id SERIAL PRIMARY KEY,
                function_name TEXT NOT NULL,
                schema_name TEXT NOT NULL DEFAULT '{schema}',
                parameters TEXT,
                return_type TEXT,
                description TEXT NOT NULL,
                created_by_session TEXT,
                created_at TIMESTAMPTZ DEFAULT NOW(),
                last_used_at TIMESTAMPTZ,
                usage_count INT DEFAULT 0,
                UNIQUE(schema_name, function_name)
            );

            COMMENT ON TABLE {schema}.function_registry
            IS 'Registry of SQL functions created by AI agents in the sandbox schema';

            CREATE TABLE IF NOT EXISTS {schema}.query_feedback (
                id SERIAL PRIMARY KEY,
                session_id TEXT NOT NULL,
                query_text TEXT NOT NULL,
                feedback_type TEXT NOT NULL,
                source TEXT NOT NULL,
                message TEXT NOT NULL,
                created_at TIMESTAMPTZ DEFAULT NOW()
            );

            COMMENT ON TABLE {schema}.query_feedback
            IS 'Feedback history from static validator and LLM critic on agent queries';
            "#,
            schema = schema
        );

        sqlx::query(&ddl)
            .execute(&pool)
            .await
            .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to create sandbox schema: {}", e)))?;

        Ok(())
    }

    async fn register_function(
        &self,
        info: &FunctionInfo,
        session_id: &str,
    ) -> Result<(), SqlNodeError> {
        let pool = self.get_pool().await?;
        let schema = &self.sandbox_schema;

        sqlx::query(&format!(
            "INSERT INTO {}.function_registry \
             (function_name, schema_name, parameters, return_type, description, created_by_session) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (schema_name, function_name) \
             DO UPDATE SET parameters = $3, return_type = $4, description = $5, \
             created_by_session = $6, created_at = NOW()",
            schema
        ))
        .bind(&info.function_name)
        .bind(&info.schema_name)
        .bind(&info.parameters)
        .bind(&info.return_type)
        .bind(&info.description)
        .bind(session_id)
        .execute(&pool)
        .await
        .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to register function: {}", e)))?;

        Ok(())
    }

    async fn list_functions(&self) -> Result<Vec<FunctionInfo>, SqlNodeError> {
        let pool = self.get_pool().await?;
        let schema = &self.sandbox_schema;

        let rows = sqlx::query(&format!(
            "SELECT function_name, schema_name, parameters, return_type, description \
             FROM {}.function_registry ORDER BY function_name",
            schema
        ))
        .fetch_all(&pool)
        .await
        .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to list functions: {}", e)))?;

        let mut functions = Vec::new();
        for row in rows {
            functions.push(FunctionInfo {
                function_name: row.try_get("function_name").unwrap_or_default(),
                schema_name: row.try_get("schema_name").unwrap_or_default(),
                parameters: row.try_get("parameters").ok(),
                return_type: row.try_get("return_type").ok(),
                description: row.try_get("description").unwrap_or_default(),
            });
        }
        Ok(functions)
    }

    async fn record_feedback(
        &self,
        session_id: &str,
        query: &str,
        feedback_type: &str,
        source: &str,
        message: &str,
    ) -> Result<(), SqlNodeError> {
        let pool = self.get_pool().await?;
        let schema = &self.sandbox_schema;

        sqlx::query(&format!(
            "INSERT INTO {}.query_feedback (session_id, query_text, feedback_type, source, message) \
             VALUES ($1, $2, $3, $4, $5)",
            schema
        ))
        .bind(session_id)
        .bind(query)
        .bind(feedback_type)
        .bind(source)
        .bind(message)
        .execute(&pool)
        .await
        .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to record feedback: {}", e)))?;

        Ok(())
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check 2>&1 | tail -10`
Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/sql_function_registry.rs
git commit -m "feat(sql_node): implement PgRegistryAdapter for sandbox function registry"
```

---

## Task 8: Infrastructure — LLM Critic Adapter

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs`

- [ ] **Step 1: Implement the LLM critic adapter**

```rust
//! LLM-based SQL critic adapter.
//!
//! Sends SQL queries to a secondary LLM for security and optimization analysis.
//! Activated only when `guardrail_llm.enabled: true` in the node config.
//!
//! Uses `LlmProviderFactory` to create a provider adapter and `LlmRepository::call()`
//! to make a single non-streaming request. No conversation persistence needed.

use crate::dag_engine::domain::sql_errors::SqlNodeError;
use crate::dag_engine::domain::sql_ports::{CriticResult, SqlCriticPort};
use crate::llm::domain::{LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderKind};
use crate::llm::infrastructure::LlmProviderFactory;
use std::str::FromStr;

/// Adapter that uses an LLM to analyze SQL queries for security and optimization.
pub struct LlmCriticAdapter {
    provider: String,
    model: String,
    api_key: String,
}

impl LlmCriticAdapter {
    pub fn new(provider: String, model: String, api_key: String) -> Self {
        Self {
            provider,
            model,
            api_key,
        }
    }
}

const CRITIC_SYSTEM_PROMPT: &str = r#"You are a PostgreSQL security and optimization reviewer. Analyze the SQL query provided and respond in EXACTLY this JSON format:

{
  "security": "ok" or "block",
  "security_reason": "explanation if blocked, null if ok",
  "optimization_hints": ["hint1", "hint2"]
}

SECURITY rules (respond "block" if ANY apply):
- Mass UPDATE/DELETE affecting potentially thousands of rows without clear business justification
- Queries that could leak sensitive data (selecting password, token, secret columns)
- Queries that modify data in ways that represent business decisions requiring human review
- SQL injection patterns or dynamic SQL construction

OPTIMIZATION hints (non-blocking suggestions):
- Missing LIMIT on large result sets
- SELECT * when specific columns would suffice
- Missing index suggestions based on WHERE/JOIN columns
- Subqueries that could be CTEs
- Unnecessary ORDER BY on large datasets

Respond ONLY with the JSON object, no other text."#;

#[async_trait::async_trait]
impl SqlCriticPort for LlmCriticAdapter {
    async fn analyze(
        &self,
        query: &str,
        schema_context: &str,
    ) -> Result<CriticResult, SqlNodeError> {
        let user_message = format!(
            "Schema context:\n{}\n\nQuery to analyze:\n{}",
            schema_context, query
        );

        // Create provider and config
        let provider_kind = ProviderKind::from_str(&self.provider)
            .map_err(|e| SqlNodeError::ConfigError(format!("Invalid critic provider: {}", e)))?;

        let llm_provider = LlmProvider::new(provider_kind.clone(), self.api_key.clone(), Some(self.model.clone()))
            .map_err(|e| SqlNodeError::ConfigError(format!("Invalid critic LLM config: {}", e)))?;

        let config = LlmConfig::new(llm_provider)
            .with_temperature(0.0)
            .map_err(|e| SqlNodeError::ConfigError(format!("{}", e)))?
            .with_max_tokens(500)
            .map_err(|e| SqlNodeError::ConfigError(format!("{}", e)))?;

        // Build messages
        let messages = vec![
            LlmMessage::system(CRITIC_SYSTEM_PROMPT.to_string())
                .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to create system message: {}", e)))?,
            LlmMessage::user(user_message)
                .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to create user message: {}", e)))?,
        ];

        // Build and send request
        let request = LlmRequest::new(messages, config, false)
            .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to create LLM request: {}", e)))?;

        let llm_repo = LlmProviderFactory::create(provider_kind);

        let response = llm_repo
            .call(request)
            .await
            .map_err(|e| SqlNodeError::ExecutionError(format!("LLM critic call failed: {}", e)))?;

        // Parse the JSON response
        let content = response.content().trim();
        let parsed: serde_json::Value = serde_json::from_str(content).unwrap_or_else(|_| {
            // If parsing fails, assume OK (fail-open for critic)
            serde_json::json!({
                "security": "ok",
                "security_reason": null,
                "optimization_hints": []
            })
        });

        let security_ok = parsed
            .get("security")
            .and_then(|v| v.as_str())
            .map(|s| s == "ok")
            .unwrap_or(true);

        let security_reason = parsed
            .get("security_reason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let optimization_hints: Vec<String> = parsed
            .get("optimization_hints")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        Ok(CriticResult {
            security_ok,
            security_reason,
            optimization_hints,
        })
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check 2>&1 | tail -10`
Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs
git commit -m "feat(sql_node): implement LlmCriticAdapter for SQL security and optimization review"
```

---

## Task 9: Application — SQL Execution Service

**Files:**
- Create: `src/libs/colmena/src/dag_engine/application/sql_execution_service.rs`
- Modify: `src/libs/colmena/src/dag_engine/application/mod.rs`

- [ ] **Step 1: Implement the execution service**

```rust
//! Orchestrates the SQL execution pipeline: validate → critic → execute → feedback.
//!
//! This is the application-layer use case. It depends only on domain ports (traits),
//! not on infrastructure adapters.

use crate::dag_engine::domain::sql_errors::SqlNodeError;
use crate::dag_engine::domain::sql_permissions::SqlPermissions;
use crate::dag_engine::domain::sql_ports::{
    FunctionRegistryPort, QueryResult, SqlConnectionPort, SqlCriticPort, SqlValidatorPort,
};
use serde_json::{json, Value};
use std::sync::Arc;

/// Orchestrates the full SQL execution pipeline.
pub struct SqlExecutionService {
    connection: Arc<dyn SqlConnectionPort>,
    validator: Arc<dyn SqlValidatorPort>,
    critic: Option<Arc<dyn SqlCriticPort>>,
    registry: Arc<dyn FunctionRegistryPort>,
}

/// Full result of a SQL execution, including metadata and feedback.
#[derive(Debug)]
pub struct SqlExecutionResult {
    pub output: Value,
    pub row_count: u64,
    pub truncated: bool,
    pub warnings: Vec<String>,
    pub optimization_hints: Vec<String>,
}

impl SqlExecutionResult {
    /// Convert to the JSON format returned to the LLM.
    pub fn to_json(&self) -> Value {
        let mut result = json!({
            "output": self.output,
            "row_count": self.row_count,
            "truncated": self.truncated,
        });

        if !self.warnings.is_empty() {
            result["warnings"] = json!(self.warnings);
        }
        if !self.optimization_hints.is_empty() {
            result["optimization_hints"] = json!(self.optimization_hints);
        }

        result
    }
}

impl SqlExecutionService {
    pub fn new(
        connection: Arc<dyn SqlConnectionPort>,
        validator: Arc<dyn SqlValidatorPort>,
        critic: Option<Arc<dyn SqlCriticPort>>,
        registry: Arc<dyn FunctionRegistryPort>,
    ) -> Self {
        Self {
            connection,
            validator,
            critic,
            registry,
        }
    }

    /// Execute the full pipeline: validate → critic → execute → post-process.
    pub async fn execute(
        &self,
        query: &str,
        permissions: &SqlPermissions,
        max_rows: u64,
        session_id: &str,
        schema_context: &str,
    ) -> Result<SqlExecutionResult, SqlNodeError> {
        // Stage 1: Static validation
        let validation = self.validator.validate(query, permissions);

        if !validation.allowed {
            let reason = validation.block_reason.unwrap_or_default();
            // Record the blocked query as feedback
            let _ = self.registry.record_feedback(
                session_id,
                query,
                "blocked",
                "static_validator",
                &reason,
            ).await;

            return Err(SqlNodeError::Blocked {
                rule: "static_validator".to_string(),
                message: reason,
            });
        }

        let mut all_warnings = validation.warnings;

        // Stage 2: LLM Critic (optional)
        let mut optimization_hints: Vec<String> = Vec::new();

        if let Some(critic) = &self.critic {
            let critic_result = critic.analyze(query, schema_context).await?;

            if !critic_result.security_ok {
                let reason = critic_result.security_reason.unwrap_or_else(|| {
                    "Query blocked by LLM security review.".to_string()
                });

                let _ = self.registry.record_feedback(
                    session_id,
                    query,
                    "blocked",
                    "llm_critic",
                    &reason,
                ).await;

                return Err(SqlNodeError::CriticRejected { reason });
            }

            optimization_hints = critic_result.optimization_hints;
        }

        // Stage 3: Execute
        let result = self.connection.execute_query(query, max_rows).await?;

        // Stage 4: Post-execution
        // Record warnings and optimization hints as feedback
        for warning in &all_warnings {
            let _ = self.registry.record_feedback(
                session_id,
                query,
                "warning",
                "static_validator",
                warning,
            ).await;
        }
        for hint in &optimization_hints {
            let _ = self.registry.record_feedback(
                session_id,
                query,
                "optimization",
                "llm_critic",
                hint,
            ).await;
        }

        // If CREATE FUNCTION, register in the function registry
        let trimmed = query.trim_start().to_uppercase();
        if trimmed.starts_with("CREATE FUNCTION") || trimmed.starts_with("CREATE OR REPLACE FUNCTION") {
            // Extract function name from query (basic heuristic)
            if let Some(func_name) = Self::extract_function_name(query) {
                let comment = Self::extract_comment(query).unwrap_or_default();
                let info = crate::dag_engine::domain::sql_ports::FunctionInfo {
                    function_name: func_name,
                    schema_name: permissions.sandbox_schema().to_string(),
                    parameters: None, // Could be parsed but keeping simple for v1
                    return_type: None,
                    description: comment,
                };
                let _ = self.registry.register_function(&info, session_id).await;
            }
        }

        Ok(SqlExecutionResult {
            output: result.output,
            row_count: result.row_count,
            truncated: result.truncated,
            warnings: all_warnings,
            optimization_hints,
        })
    }

    /// Extract function name from a CREATE FUNCTION statement (basic heuristic).
    fn extract_function_name(query: &str) -> Option<String> {
        let re = regex::Regex::new(r"(?i)CREATE\s+(?:OR\s+REPLACE\s+)?FUNCTION\s+(\S+)\s*\(").ok()?;
        re.captures(query)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    /// Extract COMMENT text from a combined CREATE + COMMENT statement.
    fn extract_comment(query: &str) -> Option<String> {
        let re = regex::Regex::new(r"(?i)COMMENT\s+ON\s+FUNCTION\s+\S+\s+IS\s+'([^']*)'").ok()?;
        re.captures(query)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }
}
```

- [ ] **Step 2: Register the module**

In `src/libs/colmena/src/dag_engine/application/mod.rs`, add:
```rust
pub mod sql_execution_service;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check 2>&1 | tail -10`
Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/application/sql_execution_service.rs \
        src/libs/colmena/src/dag_engine/application/mod.rs
git commit -m "feat(sql_node): implement SqlExecutionService orchestrating validate-critic-execute pipeline"
```

---

## Task 10: Infrastructure — SqlNode (ExecutableNode + InitializableNode)

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`

- [ ] **Step 1: Implement SqlNode**

```rust
//! SQL query node — executes PostgreSQL queries from a DAG.
//!
//! ## As an LLM tool (via `tool_configurations`)
//! The primary use case. Configure `connection_url`, `permissions`, `runtime_limits`,
//! and `guardrail_*` as fixed values in `node_schema`. The LLM only sees and provides
//! the `query` parameter.
//!
//! ## Outputs
//! Returns `{ "output": ..., "row_count": N, "truncated": bool }`.
//! For SELECT: `output` is an array of row objects.
//! For mutations: `output` is `{ "rows_affected": N }`.

use crate::dag_engine::application::sql_execution_service::{SqlExecutionService, SqlExecutionResult};
use crate::dag_engine::domain::initializable_node::{InitContext, InitializableNode};
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::sql_permissions::SqlPermissions;
use crate::dag_engine::domain::sql_ports::{FunctionInfo, TableInfo};
use crate::dag_engine::infrastructure::sql_function_registry::PgRegistryAdapter;
use crate::dag_engine::infrastructure::sql_llm_critic::LlmCriticAdapter;
use crate::dag_engine::infrastructure::sql_pool_adapter::PgPoolAdapter;
use crate::dag_engine::infrastructure::sql_static_validator::StaticRuleValidator;
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct SqlNode {
    pool_adapter: Arc<PgPoolAdapter>,
    /// Shared pool reference — also used by the registry adapter.
    pool_lock: Arc<RwLock<Option<sqlx::PgPool>>>,
    initialized: Arc<RwLock<bool>>,
    /// Cached metadata for LLM description injection.
    cached_description: Arc<RwLock<Option<String>>>,
}

impl SqlNode {
    pub fn new() -> Self {
        let pool_adapter = Arc::new(PgPoolAdapter::new());
        Self {
            pool_adapter,
            pool_lock: Arc::new(RwLock::new(None)),
            initialized: Arc::new(RwLock::new(false)),
            cached_description: Arc::new(RwLock::new(None)),
        }
    }

    /// Resolve `${ENV_VAR}` in a string.
    fn resolve_env_vars(input: &str) -> Result<String, String> {
        let mut result = String::new();
        let mut last_end = 0;
        while let Some(start) = input[last_end..].find("${") {
            let absolute_start = last_end + start;
            result.push_str(&input[last_end..absolute_start]);
            if let Some(end) = input[absolute_start..].find('}') {
                let absolute_end = absolute_start + end;
                let var_name = &input[absolute_start + 2..absolute_end];
                let val = std::env::var(var_name)
                    .map_err(|_| format!("Env var {} not found", var_name))?;
                result.push_str(&val);
                last_end = absolute_end + 1;
            } else {
                result.push_str(&input[absolute_start..]);
                last_end = input.len();
                break;
            }
        }
        result.push_str(&input[last_end..]);
        Ok(result)
    }

    /// Build the description supplement from table and function metadata.
    fn build_description_supplement(
        tables: &[TableInfo],
        functions: &[FunctionInfo],
        permissions: &SqlPermissions,
        max_rows: u64,
    ) -> String {
        let mut lines = Vec::new();

        // Tables
        if !tables.is_empty() {
            let mut current_schema = String::new();
            for table in tables {
                if table.schema_name != current_schema {
                    current_schema = table.schema_name.clone();
                    lines.push(format!("\nAvailable tables (schema: {}):", current_schema));
                }
                match &table.description {
                    Some(desc) => lines.push(format!("  - {} -- {}", table.table_name, desc)),
                    None => lines.push(format!("  - {}", table.table_name)),
                }
            }
        }

        // Functions
        if !functions.is_empty() {
            lines.push(String::new());
            lines.push("Available functions (sandbox):".to_string());
            for func in functions {
                let params = func.parameters.as_deref().unwrap_or("");
                lines.push(format!(
                    "  - {}({}) -- {}",
                    func.function_name, params, func.description
                ));
            }
        }

        // Permissions summary
        lines.push(String::new());
        lines.push(format!("{} | Max rows: {}", permissions.describe_for_llm(), max_rows));
        lines.push("Use introspection queries to discover column details when needed.".to_string());

        lines.join("\n")
    }
}

#[async_trait::async_trait]
impl InitializableNode for SqlNode {
    async fn initialize(
        &self,
        config: &Value,
    ) -> Result<InitContext, Box<dyn StdError + Send + Sync>> {
        // Parse connection URL
        let connection_url_raw = config
            .get("connection_url")
            .and_then(|v| v.as_str())
            .ok_or("sql_query node requires 'connection_url' in config")?;
        let connection_url = Self::resolve_env_vars(connection_url_raw)
            .map_err(|e| format!("Failed to resolve connection_url: {}", e))?;

        // Parse runtime limits
        let runtime_limits = config.get("runtime_limits");
        let statement_timeout_ms = runtime_limits
            .and_then(|r| r.get("statement_timeout_ms"))
            .and_then(|v| v.as_u64())
            .unwrap_or(30_000);
        let work_mem_mb = runtime_limits
            .and_then(|r| r.get("work_mem_mb"))
            .and_then(|v| v.as_u64())
            .unwrap_or(64);
        let max_rows = runtime_limits
            .and_then(|r| r.get("max_rows"))
            .and_then(|v| v.as_u64())
            .unwrap_or(100);

        // Parse permissions
        let permissions = SqlPermissions::from_config(config.get("permissions"))
            .map_err(|e| format!("Invalid permissions config: {}", e))?;

        // Connect
        self.pool_adapter
            .connect(&connection_url, statement_timeout_ms, work_mem_mb)
            .await
            .map_err(|e| format!("Failed to initialize SQL pool: {}", e))?;

        // Create registry adapter and ensure schema
        let sandbox_schema = permissions.sandbox_schema().to_string();
        let registry = PgRegistryAdapter::new(self.pool_lock.clone(), sandbox_schema);
        let _ = registry.ensure_schema().await; // Best-effort; don't fail init if sandbox setup fails

        // Load table metadata
        let allowed_schemas: Vec<String> = config
            .get("permissions")
            .and_then(|p| p.get("allowed_schemas"))
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        let tables = self
            .pool_adapter
            .load_table_metadata(&allowed_schemas)
            .await
            .unwrap_or_default();

        let functions = registry.list_functions().await.unwrap_or_default();

        // Build description supplement
        let supplement = Self::build_description_supplement(&tables, &functions, &permissions, max_rows);

        *self.cached_description.write().await = Some(supplement.clone());
        *self.initialized.write().await = true;

        Ok(InitContext {
            description_supplement: Some(supplement),
        })
    }
}

#[async_trait::async_trait]
impl ExecutableNode for SqlNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // Extract query from inputs (provided by LLM via tool call)
        let query = inputs
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("sql_query node requires 'query' input")?;

        // Parse permissions from config
        let permissions = SqlPermissions::from_config(config.get("permissions"))
            .map_err(|e| format!("Invalid permissions: {}", e))?;

        // Parse runtime limits
        let runtime_limits = config.get("runtime_limits");
        let max_rows = runtime_limits
            .and_then(|r| r.get("max_rows"))
            .and_then(|v| v.as_u64())
            .unwrap_or(100);

        // Build the execution service
        let validator = Arc::new(StaticRuleValidator) as Arc<dyn crate::dag_engine::domain::sql_ports::SqlValidatorPort>;

        // Check if LLM critic is enabled
        let critic_enabled = config
            .get("guardrail_llm")
            .and_then(|g| g.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Build LLM critic if enabled and configured
        let critic: Option<Arc<dyn crate::dag_engine::domain::sql_ports::SqlCriticPort>> = if critic_enabled {
            let guardrail_cfg = config.get("guardrail_llm").unwrap();
            let provider = guardrail_cfg.get("provider").and_then(|v| v.as_str()).unwrap_or("openai").to_string();
            let model = guardrail_cfg.get("model").and_then(|v| v.as_str()).unwrap_or("gpt-4o-mini").to_string();
            let api_key_raw = guardrail_cfg.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
            let api_key = Self::resolve_env_vars(api_key_raw).unwrap_or_default();

            Some(Arc::new(LlmCriticAdapter::new(
                provider,
                model,
                api_key,
            )) as Arc<dyn crate::dag_engine::domain::sql_ports::SqlCriticPort>)
        } else {
            None
        };

        let sandbox_schema = permissions.sandbox_schema().to_string();
        let registry = Arc::new(PgRegistryAdapter::new(self.pool_lock.clone(), sandbox_schema))
            as Arc<dyn crate::dag_engine::domain::sql_ports::FunctionRegistryPort>;

        let service = SqlExecutionService::new(
            self.pool_adapter.clone() as Arc<dyn crate::dag_engine::domain::sql_ports::SqlConnectionPort>,
            validator,
            critic,
            registry,
        );

        // Generate a session ID for feedback tracking
        let session_id = uuid::Uuid::new_v4().to_string();
        let schema_context = self
            .cached_description
            .read()
            .await
            .clone()
            .unwrap_or_default();

        println!("[SqlNode] → Executing: {}", &query[..query.len().min(100)]);

        match service.execute(query, &permissions, max_rows, &session_id, &schema_context).await {
            Ok(result) => {
                println!("[SqlNode] ✓ {} rows, truncated: {}", result.row_count, result.truncated);

                // Emit warnings and hints via observer
                if let Some(obs) = &observer {
                    for warning in &result.warnings {
                        obs.on_event(crate::dag_engine::domain::observer::NodeEvent::LlmToken {
                            token: format!("\n⚠️ SQL Warning: {}", warning),
                        });
                    }
                    for hint in &result.optimization_hints {
                        obs.on_event(crate::dag_engine::domain::observer::NodeEvent::LlmToken {
                            token: format!("\n💡 SQL Optimization: {}", hint),
                        });
                    }
                }

                Ok(result.to_json())
            }
            Err(e) => {
                println!("[SqlNode] ✗ {}", e);
                // Return error as a structured response so the LLM can read it
                Ok(json!({
                    "error": e.to_string(),
                    "source": match &e {
                        crate::dag_engine::domain::sql_errors::SqlNodeError::Blocked { .. } => "static_validator",
                        crate::dag_engine::domain::sql_errors::SqlNodeError::CriticRejected { .. } => "llm_critic",
                        _ => "execution",
                    }
                }))
            }
        }
    }

    fn schema(&self) -> Value {
        json!({
            "name": "sql_query",
            "description": "Execute PostgreSQL queries with granular permission control and validation.",
            "config": {
                "connection_url": "string (required, supports ${ENV_VAR})",
                "permissions": "object (optional, default: read_only preset)",
                "runtime_limits": "object (optional, max_rows, statement_timeout_ms, work_mem_mb)",
                "guardrail_enabled": "boolean (optional, enables static validation rules)",
                "guardrail_llm": "object (optional, LLM critic config: enabled, provider, model, api_key)"
            },
            "inputs": {
                "query": "string (required, the SQL query to execute)"
            },
            "outputs": {
                "output": "array or object (query results)",
                "row_count": "integer",
                "truncated": "boolean"
            }
        })
    }

    fn description(&self) -> Option<&str> {
        Some("Execute PostgreSQL queries with permission control, static validation, and optional LLM security review.")
    }

    fn default_input(&self) -> Option<&str> {
        Some("query")
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }
}
```

- [ ] **Step 2: Register the module**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`, add:
```rust
pub mod sql;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check 2>&1 | tail -10`
Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs
git commit -m "feat(sql_node): implement SqlNode with ExecutableNode and InitializableNode"
```

---

## Task 11: Register SqlNode in Node Registry

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`

- [ ] **Step 1: Add SqlNode import and registration**

Add to the imports at the top of `registry.rs`:
```rust
use crate::dag_engine::infrastructure::nodes::sql::SqlNode;
```

Add after the `socketio_request` registration (around line 64):
```rust
// --- Register SQL Node ---
nodes.insert("sql_query".to_string(), Arc::new(SqlNode::new()));
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check 2>&1 | tail -10`
Expected: No errors.

- [ ] **Step 3: Run existing tests to verify no regressions**

Run: `cargo test --lib 2>&1 | tail -20`
Expected: All existing tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/registry.rs
git commit -m "feat(sql_node): register SqlNode as 'sql_query' in node registry"
```

---

## Task 12: Register Infrastructure Modules

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/mod.rs` (if it exists, otherwise check where `dag_tool_executor` is exported)

- [ ] **Step 1: Find and update the infrastructure module file**

Run: `cat src/libs/colmena/src/dag_engine/infrastructure/mod.rs` to see existing module declarations.

Add the new modules:
```rust
pub mod sql_pool_adapter;
pub mod sql_static_validator;
pub mod sql_llm_critic;
pub mod sql_function_registry;
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check 2>&1 | tail -10`
Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/mod.rs
git commit -m "feat(sql_node): register SQL infrastructure modules"
```

---

## Task 13: Integration Test — Test Graph JSON

**Files:**
- Create: `tests/graphs/agents/sql_query_readonly_test.json`

- [ ] **Step 1: Create a test graph**

This graph demonstrates the SQL node as a read-only tool for an LLM agent.
Note: This requires a running PostgreSQL instance with the `production` schema. Set `DATABASE_URL` env var.

```json
{
  "nodes": {
    "sql_agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "${OPENAI_API_KEY}",
        "system_message": "You are a helpful data analyst. Use the query_database tool to answer questions about the database. Always start by listing available tables.",
        "enabled_tools": ["query_database"],
        "tool_configurations": {
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
              "guardrail_enabled": { "type": "boolean", "fixed": true },
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
        "prompt": "What tables are available in the database? List them."
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

- [ ] **Step 2: Verify the graph runs** (requires PostgreSQL + API key)

Run: `cargo run --bin dag_engine -- run tests/graphs/agents/sql_query_readonly_test.json`
Expected: The agent queries the database and lists tables.

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/agents/sql_query_readonly_test.json
git commit -m "test(sql_node): add read-only SQL agent test graph"
```

---

## Task 14: Update Documentation

**Files:**
- Modify: `docs/node_configurations.json`
- Modify: `docs/DEVELOPER_GUIDE.md`

- [ ] **Step 1: Add sql_query to node_configurations.json**

Add `"sql_query"` to the `valid_values` array in `common_node_properties.type` (around line 12), and add a new top-level entry for the `sql_query` node type with its config fields.

- [ ] **Step 2: Add sql_query to DEVELOPER_GUIDE.md**

Add a section reference pointing to the design spec for SQL node documentation.

- [ ] **Step 3: Commit**

```bash
git add docs/node_configurations.json docs/DEVELOPER_GUIDE.md
git commit -m "docs: add sql_query node to configuration reference and developer guide"
```

---

## Summary

| Task | Component | Files | Key Deliverable |
|------|-----------|-------|-----------------|
| 1 | Permissions Model | domain/sql_permissions.rs | Presets + deny + schema access |
| 2 | Error Types | domain/sql_errors.rs | SqlNodeError enum |
| 3 | Port Traits | domain/sql_ports.rs | 4 domain traits |
| 4 | InitializableNode | domain/initializable_node.rs | New trait for pre-execution setup |
| 5 | Static Validator | infrastructure/sql_static_validator.rs | 12 blocking + warning rules |
| 6 | Pool Adapter | infrastructure/sql_pool_adapter.rs | PgPool + query execution |
| 7 | Function Registry | infrastructure/sql_function_registry.rs | Sandbox table CRUD |
| 8 | LLM Critic | infrastructure/sql_llm_critic.rs | Security + optimization review |
| 9 | Execution Service | application/sql_execution_service.rs | Pipeline orchestration |
| 10 | SqlNode | infrastructure/nodes/sql.rs | ExecutableNode + InitializableNode |
| 11 | Registry | infrastructure/registry.rs | Register as "sql_query" |
| 12 | Module Registration | infrastructure/mod.rs | Export new modules |
| 13 | Test Graph | tests/graphs/agents/ | Integration test JSON |
| 14 | Documentation | docs/ | Config reference + guide |
