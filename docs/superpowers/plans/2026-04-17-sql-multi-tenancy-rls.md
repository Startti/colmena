# SQL Node Multi-Tenancy via RLS — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-user data isolation to the `sql_query` node using PostgreSQL Row-Level Security, with optional automatic policy creation.

**Architecture:** Three new fields in `SqlPermissions` (`tenant_user_id`, `tenant_column`, `auto_rls`) control multi-tenancy. At query time, `SET LOCAL app.current_user_id` is injected into the transaction. When `auto_rls: true`, the node auto-creates RLS policies during initialization and after `CREATE TABLE` detection. All RLS setup runs directly on the pool adapter, bypassing the validation pipeline.

**Tech Stack:** Rust, sqlx (PgPool), PostgreSQL RLS, existing Colmena DAG engine

**Design spec:** `docs/superpowers/specs/2026-04-17-sql-multi-tenancy-rls-design.md`

---

## File Structure

| File | Responsibility | Change |
|------|---------------|--------|
| `src/libs/colmena/src/dag_engine/domain/sql_permissions.rs` | Permission model | Add tenant fields, accessors, tests |
| `src/libs/colmena/src/dag_engine/domain/sql_ports.rs` | Domain trait ports | Add `tenant_user_id` param to `execute_query` |
| `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs` | PgPool adapter | Implement tenant SET LOCAL, add RLS setup methods |
| `src/libs/colmena/src/dag_engine/application/sql_execution_service.rs` | Execution pipeline | Pass tenant through, add post-CREATE TABLE hook |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs` | SqlNode orchestrator | Wire tenant from config, RLS setup in initialize |
| `src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs` | Static validator | Add CreateTable operation detection |
| `tests/graphs/agents/sql_rls_todo_test.json` | Test graph | Multi-tenant todo manager test |
| `docs/node_configurations.json` | Node config docs | Add tenant fields |

---

### Task 1: Add Tenant Fields to SqlPermissions

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/sql_permissions.rs`

- [ ] **Step 1: Write failing tests for tenant fields**

Add these tests at the end of the existing `mod tests` block in `sql_permissions.rs`:

```rust
#[test]
fn test_tenant_fields_parsed() {
    let config = serde_json::json!({
        "preset": "read_write",
        "allowed_schemas": ["public"],
        "tenant_user_id": "user-abc-123",
        "tenant_column": "owner_id",
        "auto_rls": true
    });
    let perms = SqlPermissions::from_config(Some(&config)).unwrap();
    assert_eq!(perms.tenant_user_id(), Some("user-abc-123"));
    assert_eq!(perms.tenant_column(), "owner_id");
    assert!(perms.auto_rls());
}

#[test]
fn test_tenant_defaults() {
    let config = serde_json::json!({
        "preset": "read_only",
        "tenant_user_id": "user-123"
    });
    let perms = SqlPermissions::from_config(Some(&config)).unwrap();
    assert_eq!(perms.tenant_user_id(), Some("user-123"));
    assert_eq!(perms.tenant_column(), "user_id");
    assert!(!perms.auto_rls());
}

#[test]
fn test_no_tenant_backwards_compatible() {
    let config = serde_json::json!({
        "preset": "read_only",
        "allowed_schemas": ["public"]
    });
    let perms = SqlPermissions::from_config(Some(&config)).unwrap();
    assert_eq!(perms.tenant_user_id(), None);
    assert!(!perms.auto_rls());
}

#[test]
fn test_none_config_no_tenant() {
    let perms = SqlPermissions::from_config(None).unwrap();
    assert_eq!(perms.tenant_user_id(), None);
    assert_eq!(perms.tenant_column(), "user_id");
    assert!(!perms.auto_rls());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p colmena_dag_engine --lib sql_permissions`
Expected: FAIL — methods `tenant_user_id()`, `tenant_column()`, `auto_rls()` don't exist.

- [ ] **Step 3: Add tenant fields to SqlPermissions struct and from_config**

In `sql_permissions.rs`, add three fields to the `SqlPermissions` struct:

```rust
#[derive(Debug, Clone)]
pub struct SqlPermissions {
    allowed_ops: HashSet<SqlOperation>,
    allowed_schemas: HashSet<String>,
    sandbox_schema: String,
    tenant_user_id: Option<String>,
    tenant_column: String,
    auto_rls: bool,
}
```

Update the `None` config early return in `from_config` to include the new fields:

```rust
None => {
    return Ok(Self {
        allowed_ops: PermissionPreset::ReadOnly.allowed_operations(),
        allowed_schemas: HashSet::new(),
        sandbox_schema: "sandbox".to_string(),
        tenant_user_id: None,
        tenant_column: "user_id".to_string(),
        auto_rls: false,
    });
}
```

Add parsing for the new fields after `sandbox_schema` parsing in the `Some(c)` branch:

```rust
let tenant_user_id = config
    .get("tenant_user_id")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());

let tenant_column = config
    .get("tenant_column")
    .and_then(|v| v.as_str())
    .unwrap_or("user_id")
    .to_string();

let auto_rls = config
    .get("auto_rls")
    .and_then(|v| v.as_bool())
    .unwrap_or(false);
```

Update the `Ok(Self { ... })` return to include them:

```rust
Ok(Self {
    allowed_ops,
    allowed_schemas,
    sandbox_schema,
    tenant_user_id,
    tenant_column,
    auto_rls,
})
```

Add accessor methods after `sandbox_schema()`:

```rust
/// The tenant user ID for RLS isolation. None means no multi-tenancy.
pub fn tenant_user_id(&self) -> Option<&str> {
    self.tenant_user_id.as_deref()
}

/// The column name used for tenant isolation (default: "user_id").
pub fn tenant_column(&self) -> &str {
    &self.tenant_column
}

/// Whether to auto-create RLS policies during initialization.
pub fn auto_rls(&self) -> bool {
    self.auto_rls
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p colmena_dag_engine --lib sql_permissions`
Expected: ALL tests pass (existing + new).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/sql_permissions.rs
git commit -m "feat(sql): add tenant_user_id, tenant_column, auto_rls to SqlPermissions"
```

---

### Task 2: Add CreateTable Operation

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/sql_permissions.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs`

- [ ] **Step 1: Write failing tests**

In `sql_permissions.rs` `mod tests`, add:

```rust
#[test]
fn test_full_preset_includes_create_table() {
    let config = serde_json::json!({ "preset": "full" });
    let perms = SqlPermissions::from_config(Some(&config)).unwrap();
    assert!(perms.is_allowed(&SqlOperation::CreateTable));
}

#[test]
fn test_read_write_no_create_table() {
    let config = serde_json::json!({ "preset": "read_write" });
    let perms = SqlPermissions::from_config(Some(&config)).unwrap();
    assert!(!perms.is_allowed(&SqlOperation::CreateTable));
}
```

In `sql_static_validator.rs` `mod tests`, add:

```rust
#[test]
fn test_create_table_allowed_full() {
    let v = StaticRuleValidator;
    let r = v.validate("CREATE TABLE public.todos (id SERIAL, title TEXT)", &full_perms());
    assert!(r.allowed);
}

#[test]
fn test_create_table_blocked_read_only() {
    let v = StaticRuleValidator;
    let r = v.validate("CREATE TABLE public.todos (id SERIAL)", &read_only_perms());
    assert!(!r.allowed);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p colmena_dag_engine --lib sql_permissions -- test_full_preset_includes_create_table test_read_write_no_create_table`
Run: `cargo test -p colmena_dag_engine --lib sql_static_validator -- test_create_table`
Expected: FAIL — `SqlOperation::CreateTable` doesn't exist.

- [ ] **Step 3: Add CreateTable variant**

In `sql_permissions.rs`, add to the `SqlOperation` enum:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SqlOperation {
    Select,
    Insert,
    Update,
    Delete,
    CreateFunction,
    CreateTable,
    /// Always blocked — no preset enables this.
    Truncate,
    /// Always blocked on protected schemas.
    Drop,
    /// Always blocked on protected schemas.
    Alter,
}
```

Add to `from_str_loose`:

```rust
"create_table" => Some(Self::CreateTable),
```

Add `CreateTable` to the `Full` preset's `allowed_operations()`:

```rust
Self::Full => {
    let mut set = HashSet::new();
    set.insert(SqlOperation::Select);
    set.insert(SqlOperation::Insert);
    set.insert(SqlOperation::Update);
    set.insert(SqlOperation::Delete);
    set.insert(SqlOperation::CreateFunction);
    set.insert(SqlOperation::CreateTable);
    set
}
```

Add `CreateTable` to `describe_for_llm()` array:

```rust
let ops: Vec<&str> = [
    (SqlOperation::Select, "SELECT"),
    (SqlOperation::Insert, "INSERT"),
    (SqlOperation::Update, "UPDATE"),
    (SqlOperation::Delete, "DELETE"),
    (SqlOperation::CreateFunction, "CREATE FUNCTION"),
    (SqlOperation::CreateTable, "CREATE TABLE"),
]
```

- [ ] **Step 4: Update static validator to detect CREATE TABLE**

In `sql_static_validator.rs`, update `detect_operation()` — add this **before** the existing `CREATE FUNCTION` check:

```rust
} else if upper.starts_with("CREATE TABLE") || upper.starts_with("CREATE TABLE IF NOT EXISTS") {
    Some(SqlOperation::CreateTable)
} else if upper.starts_with("CREATE FUNCTION") || upper.starts_with("CREATE OR REPLACE FUNCTION") {
```

- [ ] **Step 5: Run all tests**

Run: `cargo test -p colmena_dag_engine --lib sql_permissions`
Run: `cargo test -p colmena_dag_engine --lib sql_static_validator`
Expected: ALL pass.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/sql_permissions.rs
git add src/libs/colmena/src/dag_engine/infrastructure/sql_static_validator.rs
git commit -m "feat(sql): add CreateTable operation to permissions and validator"
```

---

### Task 3: Add tenant_user_id Parameter to SqlConnectionPort

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/sql_ports.rs`

- [ ] **Step 1: Update the execute_query signature**

In `sql_ports.rs`, change the `execute_query` method in `SqlConnectionPort`:

```rust
/// Execute a SQL query and return results as JSON.
/// If `tenant_user_id` is Some, runs `SET LOCAL app.current_user_id` in the same transaction.
async fn execute_query(
    &self,
    query: &str,
    max_rows: u64,
    tenant_user_id: Option<&str>,
) -> Result<QueryResult, SqlNodeError>;
```

- [ ] **Step 2: Verify it compiles (will fail — callers need updating)**

Run: `cargo check -p colmena_dag_engine`
Expected: Compile errors in `sql_pool_adapter.rs` and `sql_execution_service.rs` — they call `execute_query` with the old signature. This is expected and fixed in Tasks 4 and 5.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/sql_ports.rs
git commit -m "feat(sql): add tenant_user_id parameter to SqlConnectionPort::execute_query"
```

---

### Task 4: Implement Tenant SET LOCAL and RLS Setup in PgPoolAdapter

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs`

- [ ] **Step 1: Update execute_query to accept tenant_user_id and wrap mutations in transactions**

Replace the entire `execute_query` method in `impl SqlConnectionPort for PgPoolAdapter`:

```rust
async fn execute_query(
    &self,
    query: &str,
    max_rows: u64,
    tenant_user_id: Option<&str>,
) -> Result<QueryResult, SqlNodeError> {
    let pool = self.get_pool().await?;
    let timeout_ms = *self.statement_timeout_ms.read().await;
    let work_mem = *self.work_mem_mb.read().await;

    let trimmed = query.trim_start().to_uppercase();
    let is_select = trimmed.starts_with("SELECT") || trimmed.starts_with("WITH");

    // All queries now use transactions so we can SET LOCAL tenant context
    let mut tx = pool.begin().await.map_err(|e| {
        SqlNodeError::ExecutionError(format!("Failed to begin transaction: {}", e))
    })?;

    // Apply runtime limits
    sqlx::query(&format!("SET LOCAL statement_timeout = {}", timeout_ms))
        .execute(&mut *tx)
        .await
        .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;

    sqlx::query(&format!("SET LOCAL work_mem = '{}MB'", work_mem))
        .execute(&mut *tx)
        .await
        .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;

    // Set tenant context if multi-tenancy is active
    if let Some(uid) = tenant_user_id {
        sqlx::query("SELECT set_config('app.current_user_id', $1, true)")
            .bind(uid)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                SqlNodeError::ExecutionError(format!("Failed to set tenant context: {}", e))
            })?;
    }

    if is_select {
        let limited_query = if max_rows > 0 && !trimmed.contains("LIMIT") {
            format!("{} LIMIT {}", query.trim_end_matches(';'), max_rows + 1)
        } else {
            query.to_string()
        };

        let rows = sqlx::query(&limited_query)
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;

        tx.commit().await.map_err(|e| {
            SqlNodeError::ExecutionError(format!("Failed to commit: {}", e))
        })?;

        let mut json_rows: Vec<Value> = Vec::new();
        for row in &rows {
            let mut obj = serde_json::Map::new();
            for col in row.columns() {
                let name = col.name();
                let type_name = col.type_info().name();
                let val: Value = match type_name {
                    "INT4" | "INT8" | "INT2" | "OID" => row
                        .try_get::<i64, _>(name)
                        .map(|v| json!(v))
                        .unwrap_or(Value::Null),
                    "FLOAT4" | "FLOAT8" | "NUMERIC" => row
                        .try_get::<f64, _>(name)
                        .map(|v| json!(v))
                        .unwrap_or(Value::Null),
                    "BOOL" => row
                        .try_get::<bool, _>(name)
                        .map(|v| json!(v))
                        .unwrap_or(Value::Null),
                    _ => row
                        .try_get::<String, _>(name)
                        .map(|v| json!(v))
                        .unwrap_or(Value::Null),
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
        let result = sqlx::query(query)
            .execute(&mut *tx)
            .await
            .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;

        tx.commit().await.map_err(|e| {
            SqlNodeError::ExecutionError(format!("Failed to commit: {}", e))
        })?;

        let rows_affected = result.rows_affected();

        if trimmed.starts_with("CREATE FUNCTION")
            || trimmed.starts_with("CREATE OR REPLACE FUNCTION")
        {
            Ok(QueryResult {
                output: json!({ "created": true }),
                row_count: 0,
                truncated: false,
            })
        } else if trimmed.starts_with("CREATE TABLE") {
            Ok(QueryResult {
                output: json!({ "created": true, "type": "table" }),
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
```

Key changes vs. the old implementation:
- All queries (including mutations) now use transactions
- `SET LOCAL app.current_user_id` via `set_config()` with parameterized binding (prevents SQL injection)
- `CREATE TABLE` detection in the output section

- [ ] **Step 2: Add RLS setup methods to PgPoolAdapter**

Add these methods to `impl PgPoolAdapter` (NOT the trait — these are infrastructure-only):

```rust
/// Check if RLS is enabled on a table.
pub async fn is_rls_enabled(&self, schema: &str, table: &str) -> Result<bool, SqlNodeError> {
    let pool = self.get_pool().await?;
    let row = sqlx::query(
        "SELECT c.relrowsecurity \
         FROM pg_catalog.pg_class c \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = $2"
    )
    .bind(schema)
    .bind(table)
    .fetch_optional(&pool)
    .await
    .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to check RLS status: {}", e)))?;

    Ok(row.map(|r| r.try_get::<bool, _>("relrowsecurity").unwrap_or(false)).unwrap_or(false))
}

/// Check if a column exists in a table.
pub async fn has_column(&self, schema: &str, table: &str, column: &str) -> Result<bool, SqlNodeError> {
    let pool = self.get_pool().await?;
    let row = sqlx::query(
        "SELECT 1 FROM information_schema.columns \
         WHERE table_schema = $1 AND table_name = $2 AND column_name = $3"
    )
    .bind(schema)
    .bind(table)
    .bind(column)
    .fetch_optional(&pool)
    .await
    .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to check column: {}", e)))?;

    Ok(row.is_some())
}

/// Check if a specific RLS policy exists on a table.
async fn has_policy(&self, schema: &str, table: &str, policy_name: &str) -> Result<bool, SqlNodeError> {
    let pool = self.get_pool().await?;
    let row = sqlx::query(
        "SELECT 1 FROM pg_policies \
         WHERE schemaname = $1 AND tablename = $2 AND policyname = $3"
    )
    .bind(schema)
    .bind(table)
    .bind(policy_name)
    .fetch_optional(&pool)
    .await
    .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to check policy: {}", e)))?;

    Ok(row.is_some())
}

/// Add the tenant column to a table if it doesn't exist.
pub async fn add_tenant_column(&self, schema: &str, table: &str, tenant_column: &str) -> Result<(), SqlNodeError> {
    let pool = self.get_pool().await?;
    let sql = format!(
        "ALTER TABLE {}.{} ADD COLUMN IF NOT EXISTS {} TEXT DEFAULT current_setting('app.current_user_id')",
        schema, table, tenant_column
    );
    sqlx::query(&sql)
        .execute(&pool)
        .await
        .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to add tenant column: {}", e)))?;
    Ok(())
}

/// Set up RLS for a single table. Called during initialize() and after CREATE TABLE.
///
/// - If table has `tenant_column`: enables RLS + tenant isolation policy + DEFAULT on column
/// - If table lacks `tenant_column`: enables RLS + read-only policy (SELECT only)
pub async fn setup_rls_for_table(
    &self,
    schema: &str,
    table: &str,
    tenant_column: &str,
) -> Result<(), SqlNodeError> {
    // Check if RLS is already enabled
    if self.is_rls_enabled(schema, table).await? {
        println!("[RLS] {}.{} — already enabled, skipping", schema, table);
        return Ok(());
    }

    let pool = self.get_pool().await?;
    let has_tenant_col = self.has_column(schema, table, tenant_column).await?;

    // Enable RLS
    let enable_sql = format!("ALTER TABLE {}.{} ENABLE ROW LEVEL SECURITY", schema, table);
    sqlx::query(&enable_sql)
        .execute(&pool)
        .await
        .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to enable RLS on {}.{}: {}", schema, table, e)))?;

    if has_tenant_col {
        // Tenant isolation policy
        let policy_name = "colmena_tenant_isolation";
        if !self.has_policy(schema, table, policy_name).await? {
            let policy_sql = format!(
                "CREATE POLICY {} ON {}.{} \
                 USING ({} = current_setting('app.current_user_id')) \
                 WITH CHECK ({} = current_setting('app.current_user_id'))",
                policy_name, schema, table, tenant_column, tenant_column
            );
            sqlx::query(&policy_sql)
                .execute(&pool)
                .await
                .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to create tenant policy on {}.{}: {}", schema, table, e)))?;
        }

        // Auto-DEFAULT on tenant column
        let default_sql = format!(
            "ALTER TABLE {}.{} ALTER COLUMN {} SET DEFAULT current_setting('app.current_user_id')",
            schema, table, tenant_column
        );
        sqlx::query(&default_sql)
            .execute(&pool)
            .await
            .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to set default on {}.{}.{}: {}", schema, table, tenant_column, e)))?;

        println!("[RLS] {}.{} — tenant isolation enabled (column: {})", schema, table, tenant_column);
    } else {
        // Read-only policy for shared tables
        let policy_name = "colmena_shared_read";
        if !self.has_policy(schema, table, policy_name).await? {
            let policy_sql = format!(
                "CREATE POLICY {} ON {}.{} FOR SELECT USING (true)",
                policy_name, schema, table
            );
            sqlx::query(&policy_sql)
                .execute(&pool)
                .await
                .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to create read-only policy on {}.{}: {}", schema, table, e)))?;
        }

        println!("[RLS] {}.{} — read-only (no {} column)", schema, table, tenant_column);
    }

    Ok(())
}

/// Set up RLS for a newly created table. Auto-adds tenant column if missing.
pub async fn setup_rls_for_new_table(
    &self,
    schema: &str,
    table: &str,
    tenant_column: &str,
) -> Result<(), SqlNodeError> {
    let has_tenant_col = self.has_column(schema, table, tenant_column).await?;

    if !has_tenant_col {
        // Auto-add tenant column with DEFAULT
        self.add_tenant_column(schema, table, tenant_column).await?;
        println!("[RLS] {}.{} — auto-added column '{}'", schema, table, tenant_column);
    }

    // Now set up RLS (the table now has the tenant column either way)
    self.setup_rls_for_table(schema, table, tenant_column).await
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p colmena_dag_engine`
Expected: Still fails due to `sql_execution_service.rs` calling old `execute_query` signature. That's fixed in Task 5.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs
git commit -m "feat(sql): implement tenant SET LOCAL and RLS setup in PgPoolAdapter"
```

---

### Task 5: Pass Tenant Through SqlExecutionService and Add Post-CREATE TABLE Hook

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/sql_execution_service.rs`

- [ ] **Step 1: Update execute() to accept tenant_user_id and pass it to execute_query**

Change the `execute()` method signature to add `tenant_user_id`:

```rust
pub async fn execute(
    &self,
    query: &str,
    permissions: &SqlPermissions,
    max_rows: u64,
    session_id: &str,
    schema_context: &str,
    tenant_user_id: Option<&str>,
) -> Result<SqlExecutionResult, SqlNodeError> {
```

Update the Stage 3 line (the `execute_query` call) to pass the tenant:

```rust
// Stage 3: Execute
let result = self.connection.execute_query(query, max_rows, tenant_user_id).await?;
```

No other changes needed in this method — the rest of the pipeline (validation, critic, feedback) is unchanged.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p colmena_dag_engine`
Expected: Fails in `sql.rs` — `SqlNode` calls `service.execute()` with the old signature. Fixed in Task 6.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/dag_engine/application/sql_execution_service.rs
git commit -m "feat(sql): pass tenant_user_id through SqlExecutionService::execute"
```

---

### Task 6: Wire Tenant Through SqlNode

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`

This is the integration task. SqlNode must:
1. Resolve `tenant_user_id` from `${ENV_VAR}` in permissions config
2. Run auto-RLS setup during `initialize()` when `auto_rls: true`
3. Pass `tenant_user_id` to `service.execute()`
4. Detect `CREATE TABLE` after execution and apply RLS to the new table

- [ ] **Step 1: Add RLS setup to initialize()**

In the `initialize()` method of `impl InitializableNode for SqlNode`, add the RLS setup **after** the table metadata is loaded and the supplement is built (after line `let supplement = Self::build_description_supplement(...)`) but **before** setting `cached_description`:

```rust
// Auto-RLS setup if enabled
let permissions_for_rls = SqlPermissions::from_config(config.get("permissions"))
    .map_err(|e| format!("Invalid permissions config: {}", e))?;

if permissions_for_rls.auto_rls() {
    println!("[SqlNode] auto_rls enabled — setting up RLS policies...");
    let tenant_col = permissions_for_rls.tenant_column();
    for table in &tables {
        if let Err(e) = self.pool_adapter.setup_rls_for_table(
            &table.schema_name,
            &table.table_name,
            tenant_col,
        ).await {
            println!("[SqlNode] RLS setup warning for {}.{}: {}", table.schema_name, table.table_name, e);
        }
    }
}
```

- [ ] **Step 2: Add helper to extract table name from CREATE TABLE statement**

Add this method to `impl SqlNode`:

```rust
/// Extract schema and table name from a CREATE TABLE statement.
/// Returns (schema, table). If no schema is specified, defaults to "public".
fn extract_create_table_name(query: &str) -> Option<(String, String)> {
    let re = regex::Regex::new(
        r"(?i)CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(?:(\w+)\.)?(\w+)\s*\("
    ).ok()?;
    let caps = re.captures(query)?;
    let schema = caps.get(1).map(|m| m.as_str().to_string()).unwrap_or_else(|| "public".to_string());
    let table = caps.get(2)?.as_str().to_string();
    Some((schema, table))
}
```

- [ ] **Step 3: Update execute() to resolve tenant and handle post-CREATE TABLE**

In the `execute()` method of `impl ExecutableNode for SqlNode`, make these changes:

After building `effective_config` and getting `permissions`, resolve the tenant:

```rust
// Resolve tenant_user_id (may contain ${ENV_VAR})
let tenant_user_id: Option<String> = permissions.tenant_user_id().map(|raw| {
    Self::resolve_env_vars(raw).unwrap_or_else(|e| {
        println!("[SqlNode] Warning: failed to resolve tenant_user_id: {}", e);
        raw.to_string()
    })
});
```

Update the `service.execute()` call to pass the tenant:

```rust
match service.execute(query, &permissions, max_rows, &session_id, &schema_context, tenant_user_id.as_deref()).await {
```

After the `Ok(result)` match arm (the successful execution path), add `CREATE TABLE` detection **before** `Ok(result.to_json())`:

```rust
Ok(result) => {
    println!("[SqlNode] {} rows, truncated: {}", result.row_count, result.truncated);

    // Post-CREATE TABLE: apply RLS to the new table
    let trimmed_upper = query.trim_start().to_uppercase();
    if trimmed_upper.starts_with("CREATE TABLE") && permissions.auto_rls() {
        if let Some((schema, table)) = Self::extract_create_table_name(query) {
            println!("[SqlNode] CREATE TABLE detected — applying RLS to {}.{}", schema, table);
            if let Err(e) = self.pool_adapter.setup_rls_for_new_table(
                &schema,
                &table,
                permissions.tenant_column(),
            ).await {
                println!("[SqlNode] RLS setup warning for new table {}.{}: {}", schema, table, e);
            }
        }
    }

    if let Some(obs) = &observer {
        // ... existing observer code ...
    }

    Ok(result.to_json())
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p colmena_dag_engine`
Expected: PASS — all pieces are now wired together.

- [ ] **Step 5: Run all existing tests**

Run: `cargo test -p colmena_dag_engine --lib`
Expected: ALL existing tests pass (backwards compatible — no tenant_user_id = no behavior change).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs
git commit -m "feat(sql): wire tenant isolation through SqlNode with auto-RLS and CREATE TABLE hook"
```

---

### Task 7: Test Graph and Documentation

**Files:**
- Create: `tests/graphs/agents/sql_rls_todo_test.json`
- Modify: `docs/node_configurations.json`

- [ ] **Step 1: Create the multi-tenant test graph**

```json
{
  "nodes": {
    "todo_agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "${OPENAI_API_KEY}",
        "system_message": "You are a personal todo manager. You can create tables, insert tasks, list tasks, and update them. Start by checking if a 'todos' table exists. If not, create it with columns: id SERIAL PRIMARY KEY, title TEXT NOT NULL, done BOOLEAN DEFAULT false. Then list all tasks.",
        "enabled_tools": [
          "manage_todos"
        ],
        "stream": false,
        "tool_configurations": {
          "manage_todos": {
            "name": "manage_todos",
            "node_type": "sql_query",
            "description": "Manage the user's todo list in the database.",
            "node_schema": {
              "connection_url": {
                "type": "string",
                "fixed": "${DATABASE_URL_GRAPHS}"
              },
              "permissions": {
                "type": "object",
                "fixed": {
                  "preset": "full",
                  "allowed_schemas": [
                    "public"
                  ],
                  "tenant_user_id": "${USER_ID}",
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
              "guardrail_enabled": {
                "type": "boolean",
                "fixed": true
              },
              "guardrail_llm": {
                "type": "object",
                "fixed": {
                  "enabled": false
                }
              },
              "query": {
                "type": "string",
                "required": true,
                "description": "SQL query to manage the user's todo list. You can CREATE TABLE, INSERT, SELECT, UPDATE, and DELETE."
              }
            }
          }
        },
        "prompt": "Create a todos table if it doesn't exist, then add a task 'Buy groceries' and list all tasks."
      }
    },
    "result": {
      "type": "output",
      "config": {
        "label": "Todo Agent Result"
      }
    }
  },
  "edges": [
    {
      "from": "todo_agent",
      "to": "result"
    }
  ]
}
```

- [ ] **Step 2: Update docs/node_configurations.json**

In the existing `sql_query` node configuration entry, add the tenant fields inside the `permissions` config object description. Find the `sql_query` entry's `permissions` field and add:

```json
"tenant_user_id": {
  "type": "string",
  "required": false,
  "description": "User ID for RLS multi-tenancy. Supports ${ENV_VAR} resolution. When set, runs SET app.current_user_id before every query.",
  "example": "${USER_ID}"
},
"tenant_column": {
  "type": "string",
  "required": false,
  "default": "user_id",
  "description": "Column name used for tenant isolation in RLS policies."
},
"auto_rls": {
  "type": "boolean",
  "required": false,
  "default": false,
  "description": "When true, auto-creates RLS policies during initialization and after CREATE TABLE. Tables with tenant_column get tenant isolation; tables without get read-only."
}
```

- [ ] **Step 3: Test the graph manually**

Run:
```bash
USER_ID=test-user-1 cargo run --bin dag_engine -- run tests/graphs/agents/sql_rls_todo_test.json
```

Expected: Agent creates the `todos` table, RLS is auto-applied, task is inserted and listed. Look for `[RLS]` log lines.

Then test with a different user:
```bash
USER_ID=test-user-2 cargo run --bin dag_engine -- run tests/graphs/agents/sql_rls_todo_test.json
```

Expected: User 2 should NOT see User 1's tasks. The table already exists so RLS setup is skipped ("already enabled").

- [ ] **Step 4: Commit**

```bash
git add tests/graphs/agents/sql_rls_todo_test.json docs/node_configurations.json
git commit -m "feat(sql): add multi-tenant RLS test graph and update documentation"
```

---

## Verification Checklist

After all tasks are complete, verify:

1. **Backwards compatibility**: Run the existing `sql_query_readonly_test.json` — should work exactly as before (no `tenant_user_id` = no RLS behavior).
   ```bash
   cargo run --bin dag_engine -- run tests/graphs/agents/sql_query_readonly_test.json
   ```

2. **Multi-tenancy isolation**: Run `sql_rls_todo_test.json` with `USER_ID=alice`, then `USER_ID=bob`. Each should only see their own tasks.

3. **Auto-RLS on existing tables**: Create a table manually in the DB, then run the test graph — the node should detect and apply RLS during initialization.

4. **All unit tests pass**:
   ```bash
   cargo test -p colmena_dag_engine --lib
   ```
