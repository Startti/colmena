//! PostgreSQL connection pool adapter.
//!
//! Adapter that wraps a PostgreSQL connection pool with per-query runtime limits.
//!
//! Does NOT own pool creation. The caller (normally `SqlPortFactory`) must pass
//! an `Arc<PgPool>` obtained from the shared `PgPoolRegistry`. Per-query
//! `statement_timeout` and `work_mem` are applied via `SET LOCAL` inside every
//! transaction, so multiple adapters can safely share a pool.

use crate::dag_engine::domain::sql_errors::SqlNodeError;
use crate::dag_engine::domain::sql_ports::{QueryResult, SqlConnectionPort, TableInfo};
use serde_json::{json, Value};
use sqlx::{Column, PgPool, Row, TypeInfo};
use std::sync::Arc;

/// Adapter that wraps a PostgreSQL connection pool with per-query runtime limits.
///
/// Does NOT own pool creation. The caller (normally `SqlPortFactory`) must pass
/// an `Arc<PgPool>` obtained from the shared `PgPoolRegistry`. Per-query
/// `statement_timeout` and `work_mem` are applied via `SET LOCAL` inside every
/// transaction, so multiple adapters can safely share a pool.
pub struct PgPoolAdapter {
    pool: Arc<PgPool>,
    statement_timeout_ms: u64,
    work_mem_mb: u64,
}

impl PgPoolAdapter {
    pub fn new(pool: Arc<PgPool>, statement_timeout_ms: u64, work_mem_mb: u64) -> Self {
        Self {
            pool,
            statement_timeout_ms,
            work_mem_mb,
        }
    }

    /// Shared reference to the underlying pool — used by `PgRegistryAdapter`
    /// (sandbox function registry) to reuse the same connections.
    pub fn pool(&self) -> Arc<PgPool> {
        self.pool.clone()
    }

    /// Quote a SQL identifier to prevent injection (equivalent to PostgreSQL's quote_ident).
    fn quote_ident(s: &str) -> String {
        format!("\"{}\"", s.replace('"', "\"\""))
    }

    /// Check if RLS is enabled on a table.
    pub async fn is_rls_enabled(&self, schema: &str, table: &str) -> Result<bool, SqlNodeError> {
        let row = sqlx::query(
            "SELECT c.relrowsecurity \
             FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2",
        )
        .bind(schema)
        .bind(table)
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to check RLS status: {}", e)))?;

        Ok(row
            .map(|r| r.try_get::<bool, _>("relrowsecurity").unwrap_or(false))
            .unwrap_or(false))
    }

    /// Check if a column exists in a table.
    pub async fn has_column(
        &self,
        schema: &str,
        table: &str,
        column: &str,
    ) -> Result<bool, SqlNodeError> {
        let row = sqlx::query(
            "SELECT 1 FROM information_schema.columns \
             WHERE table_schema = $1 AND table_name = $2 AND column_name = $3",
        )
        .bind(schema)
        .bind(table)
        .bind(column)
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to check column: {}", e)))?;

        Ok(row.is_some())
    }

    /// Check if a specific RLS policy exists on a table.
    async fn has_policy(
        &self,
        schema: &str,
        table: &str,
        policy_name: &str,
    ) -> Result<bool, SqlNodeError> {
        let row = sqlx::query(
            "SELECT 1 FROM pg_policies \
             WHERE schemaname = $1 AND tablename = $2 AND policyname = $3",
        )
        .bind(schema)
        .bind(table)
        .bind(policy_name)
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to check policy: {}", e)))?;

        Ok(row.is_some())
    }

    /// Add the tenant column to a table if it doesn't exist.
    pub async fn add_tenant_column(
        &self,
        schema: &str,
        table: &str,
        tenant_column: &str,
    ) -> Result<(), SqlNodeError> {
        let sql = format!(
            "ALTER TABLE {}.{} ADD COLUMN IF NOT EXISTS {} TEXT DEFAULT current_setting('app.current_user_id')",
            Self::quote_ident(schema), Self::quote_ident(table), Self::quote_ident(tenant_column)
        );
        sqlx::query(&sql).execute(&*self.pool).await.map_err(|e| {
            SqlNodeError::ExecutionError(format!("Failed to add tenant column: {}", e))
        })?;
        Ok(())
    }

    /// Set up RLS for a single table. Called during initialize() and after CREATE TABLE.
    ///
    /// - If table has `tenant_column`: enables RLS + tenant isolation policy + DEFAULT on column.
    /// - If table lacks `tenant_column`: enables RLS + read-only policy (SELECT only).
    pub async fn setup_rls_for_table(
        &self,
        schema: &str,
        table: &str,
        tenant_column: &str,
    ) -> Result<(), SqlNodeError> {
        let has_tenant_col = self.has_column(schema, table, tenant_column).await?;

        // Enable RLS (idempotent — Postgres ignores if already enabled)
        let enable_sql = format!(
            "ALTER TABLE {}.{} ENABLE ROW LEVEL SECURITY",
            Self::quote_ident(schema),
            Self::quote_ident(table)
        );
        sqlx::query(&enable_sql)
            .execute(&*self.pool)
            .await
            .map_err(|e| {
                SqlNodeError::ExecutionError(format!(
                    "Failed to enable RLS on {}.{}: {}",
                    schema, table, e
                ))
            })?;

        // Force RLS on the table owner too — without this, the user that
        // created the table (typically our connection role) bypasses RLS.
        let force_sql = format!(
            "ALTER TABLE {}.{} FORCE ROW LEVEL SECURITY",
            Self::quote_ident(schema),
            Self::quote_ident(table)
        );
        sqlx::query(&force_sql)
            .execute(&*self.pool)
            .await
            .map_err(|e| {
                SqlNodeError::ExecutionError(format!(
                    "Failed to force RLS on {}.{}: {}",
                    schema, table, e
                ))
            })?;

        if has_tenant_col {
            let policy_name = "colmena_tenant_isolation";
            if !self.has_policy(schema, table, policy_name).await? {
                let policy_sql = format!(
                    "CREATE POLICY {} ON {}.{} \
                     USING ({} = current_setting('app.current_user_id')) \
                     WITH CHECK ({} = current_setting('app.current_user_id'))",
                    policy_name,
                    Self::quote_ident(schema),
                    Self::quote_ident(table),
                    Self::quote_ident(tenant_column),
                    Self::quote_ident(tenant_column)
                );
                sqlx::query(&policy_sql)
                    .execute(&*self.pool)
                    .await
                    .map_err(|e| {
                        SqlNodeError::ExecutionError(format!(
                            "Failed to create tenant policy on {}.{}: {}",
                            schema, table, e
                        ))
                    })?;
            }

            let default_sql = format!(
                "ALTER TABLE {}.{} ALTER COLUMN {} SET DEFAULT current_setting('app.current_user_id')",
                Self::quote_ident(schema), Self::quote_ident(table), Self::quote_ident(tenant_column)
            );
            sqlx::query(&default_sql)
                .execute(&*self.pool)
                .await
                .map_err(|e| {
                    SqlNodeError::ExecutionError(format!(
                        "Failed to set default on {}.{}.{}: {}",
                        schema, table, tenant_column, e
                    ))
                })?;

            println!(
                "[RLS] {}.{} — tenant isolation enabled (column: {})",
                schema, table, tenant_column
            );
        } else {
            let policy_name = "colmena_shared_read";
            if !self.has_policy(schema, table, policy_name).await? {
                let policy_sql = format!(
                    "CREATE POLICY {} ON {}.{} FOR SELECT USING (true)",
                    policy_name,
                    Self::quote_ident(schema),
                    Self::quote_ident(table)
                );
                sqlx::query(&policy_sql)
                    .execute(&*self.pool)
                    .await
                    .map_err(|e| {
                        SqlNodeError::ExecutionError(format!(
                            "Failed to create read-only policy on {}.{}: {}",
                            schema, table, e
                        ))
                    })?;
            }

            println!(
                "[RLS] {}.{} — read-only (no {} column)",
                schema, table, tenant_column
            );
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
            self.add_tenant_column(schema, table, tenant_column).await?;
            println!(
                "[RLS] {}.{} — auto-added column '{}'",
                schema, table, tenant_column
            );
        }

        self.setup_rls_for_table(schema, table, tenant_column).await
    }
}

/// Marshall a vector of sqlx `PgRow`s into JSON-friendly `Vec<Value>`.
/// Extracted so the SELECT path and any future helper can share the logic.
fn marshall_rows(rows: &[sqlx::postgres::PgRow]) -> Vec<Value> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut obj = serde_json::Map::new();
        for col in row.columns() {
            let name = col.name();
            let type_name = col.type_info().name();
            let val: Value = match type_name {
                "INT8" => row
                    .try_get::<i64, _>(name)
                    .map(|v| json!(v))
                    .unwrap_or(Value::Null),
                "INT4" | "OID" => row
                    .try_get::<i32, _>(name)
                    .map(|v| json!(v as i64))
                    .unwrap_or(Value::Null),
                "INT2" => row
                    .try_get::<i16, _>(name)
                    .map(|v| json!(v as i64))
                    .unwrap_or(Value::Null),
                "FLOAT4" | "FLOAT8" => row
                    .try_get::<f64, _>(name)
                    .map(|v| json!(v))
                    .unwrap_or(Value::Null),
                // Postgres NUMERIC/DECIMAL is arbitrary-precision and cannot be
                // decoded directly as `f64` by sqlx — we must go through
                // `BigDecimal` (enabled via the `bigdecimal` feature on sqlx).
                // We then convert to `f64` for JSON output, accepting the
                // ~15-17 significant-digit precision limit of IEEE 754 doubles.
                // For values that require exact precision beyond that, ask the
                // LLM to cast the column to TEXT in its SELECT.
                "NUMERIC" => row
                    .try_get::<sqlx::types::BigDecimal, _>(name)
                    .ok()
                    .and_then(|bd| bd.to_string().parse::<f64>().ok())
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
        out.push(Value::Object(obj));
    }
    out
}

#[async_trait::async_trait]
impl SqlConnectionPort for PgPoolAdapter {
    /// Execute a SQL script that may contain multiple statements separated by `;`.
    ///
    /// **Policy C (shipped 2026-06-09):** parses the script with `sqlparser`,
    /// then executes each statement individually inside ONE atomic transaction.
    /// The result returned is the output of the LAST statement:
    /// - last is SELECT → rows as `Value::Array`, with `LIMIT` auto-injected
    ///   when the statement has no explicit LIMIT.
    /// - last is INSERT/UPDATE/DELETE → `{ rows_affected: SUM }` over all
    ///   mutator statements in the script (not just the last one).
    /// - last is CREATE TABLE → `{ created: true, type: "table" }`.
    /// - last is CREATE FUNCTION → `{ created: true }`.
    ///
    /// Intermediate SELECTs execute normally but their rows are discarded;
    /// `LIMIT` is NOT auto-injected on intermediate SELECTs.
    ///
    /// If ANY statement fails, the whole transaction rolls back.
    ///
    /// See dev guide `docs/developer_guide/23_sql_node.md` §"Multi-statement"
    /// and the user-facing skill `sql-query-best-practices` for examples.
    async fn execute_query(
        &self,
        query: &str,
        max_rows: u64,
        tenant_user_id: Option<&str>,
    ) -> Result<QueryResult, SqlNodeError> {
        use sqlparser::ast::Statement;
        let pool = &*self.pool;

        // Parse the script. The validator already parsed and accepted it before
        // we got here, so a parse error here is unexpected — surface clearly.
        let stmts = crate::dag_engine::infrastructure::sql_ast::parse(query)
            .map_err(|e| SqlNodeError::ExecutionError(format!("re-parse failed: {}", e)))?;
        if stmts.is_empty() {
            return Err(SqlNodeError::ExecutionError("empty SQL script".into()));
        }
        let last_idx = stmts.len() - 1;

        // Begin transaction + apply session-level guardrails.
        let mut tx = pool.begin().await.map_err(|e| {
            SqlNodeError::ExecutionError(format!("Failed to begin transaction: {}", e))
        })?;

        sqlx::query(&format!(
            "SET LOCAL statement_timeout = {}",
            self.statement_timeout_ms
        ))
        .execute(&mut *tx)
        .await
        .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;

        sqlx::query(&format!("SET LOCAL work_mem = '{}MB'", self.work_mem_mb))
            .execute(&mut *tx)
            .await
            .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;

        if let Some(uid) = tenant_user_id {
            sqlx::query("SELECT set_config('app.current_user_id', $1, true)")
                .bind(uid)
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    SqlNodeError::ExecutionError(format!("Failed to set tenant context: {}", e))
                })?;
        }

        // Per-statement loop.
        let mut rows_affected_sum: u64 = 0;

        for (idx, stmt) in stmts.iter().enumerate() {
            let is_last = idx == last_idx;
            // sqlparser re-serializes the statement — preserves PL/pgSQL bodies
            // (`AS $$ ... $$`), JSON arrows, escaped quotes, etc. Stable round-trip.
            let stmt_sql = stmt.to_string();
            let is_select_stmt = crate::dag_engine::infrastructure::sql_ast::is_query(stmt);

            if is_select_stmt && is_last {
                // Final SELECT — apply LIMIT, fetch, marshall, return.
                let already_has_limit =
                    crate::dag_engine::infrastructure::sql_ast::query_has_limit(stmt);
                let limited = if max_rows > 0 && !already_has_limit {
                    format!("{} LIMIT {}", stmt_sql.trim_end_matches(';'), max_rows + 1)
                } else {
                    stmt_sql
                };
                let rows = sqlx::query(&limited)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;
                tx.commit().await.map_err(|e| {
                    SqlNodeError::ExecutionError(format!("Failed to commit: {}", e))
                })?;

                let mut json_rows = marshall_rows(&rows);
                let truncated = max_rows > 0 && json_rows.len() as u64 > max_rows;
                if truncated {
                    json_rows.truncate(max_rows as usize);
                }
                let row_count = json_rows.len() as u64;
                return Ok(QueryResult {
                    output: Value::Array(json_rows),
                    row_count,
                    truncated,
                });
            } else if is_select_stmt {
                // Intermediate SELECT — execute (side effects, e.g. set_config),
                // discard rows. No LIMIT injection.
                sqlx::query(&stmt_sql)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;
            } else {
                // Mutation (INSERT/UPDATE/DELETE/CREATE TABLE/CREATE FUNCTION/COMMENT/...).
                let result = sqlx::query(&stmt_sql)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;

                if is_last {
                    let total = rows_affected_sum + result.rows_affected();
                    tx.commit().await.map_err(|e| {
                        SqlNodeError::ExecutionError(format!("Failed to commit: {}", e))
                    })?;
                    let (output, row_count) = match stmt {
                        Statement::CreateFunction(_) => (json!({ "created": true }), 0u64),
                        Statement::CreateTable(_) => {
                            (json!({ "created": true, "type": "table" }), 0u64)
                        }
                        _ => (json!({ "rows_affected": total }), total),
                    };
                    return Ok(QueryResult {
                        output,
                        row_count,
                        truncated: false,
                    });
                }
                rows_affected_sum += result.rows_affected();
            }
        }

        // Loop always returns when processing the last statement (guaranteed
        // because we checked `!stmts.is_empty()` above and every branch in the
        // `is_last` path returns).
        unreachable!("statements vector was non-empty but loop did not return");
    }

    async fn load_table_metadata(
        &self,
        schemas: &[String],
    ) -> Result<Vec<TableInfo>, SqlNodeError> {
        let pool = &*self.pool;

        if schemas.is_empty() {
            return Ok(vec![]);
        }

        let placeholders: Vec<String> = schemas
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 1))
            .collect();

        let query = format!(
            "SELECT t.table_schema, t.table_name, \
             pg_catalog.obj_description(c.oid) as description \
             FROM information_schema.tables t \
             LEFT JOIN pg_catalog.pg_class c ON c.relname = t.table_name \
             LEFT JOIN pg_catalog.pg_namespace n \
               ON n.oid = c.relnamespace AND n.nspname = t.table_schema \
             WHERE t.table_schema IN ({}) \
             AND t.table_type = 'BASE TABLE' \
             ORDER BY t.table_schema, t.table_name",
            placeholders.join(", ")
        );

        let mut q = sqlx::query(&query);
        for schema in schemas {
            q = q.bind(schema);
        }

        let rows = q.fetch_all(pool).await.map_err(|e| {
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

    async fn missing_schemas(&self, schemas: &[String]) -> Result<Vec<String>, SqlNodeError> {
        // Never treat introspection schemas as missing — they always exist and
        // must never be created.
        let candidates: Vec<String> = schemas
            .iter()
            .filter(|s| !matches!(s.as_str(), "information_schema" | "pg_catalog"))
            .cloned()
            .collect();

        if candidates.is_empty() {
            return Ok(vec![]);
        }

        let rows = sqlx::query(
            "SELECT schema_name FROM information_schema.schemata \
             WHERE schema_name = ANY($1)",
        )
        .bind(&candidates)
        .fetch_all(&*self.pool)
        .await
        .map_err(|e| {
            SqlNodeError::ExecutionError(format!("Failed to list existing schemas: {}", e))
        })?;

        let existing: std::collections::HashSet<String> = rows
            .iter()
            .filter_map(|r| r.try_get::<String, _>("schema_name").ok())
            .collect();

        Ok(candidates
            .into_iter()
            .filter(|s| !existing.contains(s))
            .collect())
    }

    async fn create_schema(&self, schema: &str) -> Result<(), SqlNodeError> {
        let sql = format!("CREATE SCHEMA IF NOT EXISTS {}", Self::quote_ident(schema));
        sqlx::query(&sql).execute(&*self.pool).await.map_err(|e| {
            SqlNodeError::ExecutionError(format!(
                "Failed to create schema '{}' (the database role may lack CREATE privilege): {}",
                schema, e
            ))
        })?;
        Ok(())
    }

    async fn execute_setup_sql(&self, sql: &str) -> Result<(), SqlNodeError> {
        // Execute the whole operator block via the simple query protocol
        // (`raw_sql`). Postgres parses the `;` separators itself (so string
        // literals and dollar-quoted function bodies are handled correctly),
        // and a multi-statement simple-protocol batch runs as a SINGLE implicit
        // transaction — any statement failure rolls the whole block back.
        // Executing on `&*self.pool` (a `&PgPool`) avoids the HRTB error that
        // `&mut *tx` triggers in sqlx 0.8.
        sqlx::raw_sql(sql)
            .execute(&*self.pool)
            .await
            .map_err(|e| SqlNodeError::ExecutionError(format!("setup_sql execution failed: {}", e)))?;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    /// Build an adapter against `TEST_DATABASE_URL`, or return `None` to skip.
    async fn test_adapter() -> Option<PgPoolAdapter> {
        let url = std::env::var("TEST_DATABASE_URL").ok()?;
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect to TEST_DATABASE_URL");
        Some(PgPoolAdapter::new(Arc::new(pool), 30_000, 64))
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn missing_then_create_then_present() {
        let Some(adapter) = test_adapter().await else {
            eprintln!("skip: TEST_DATABASE_URL not set");
            return;
        };

        let schema = format!(
            "colmena_test_schema_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let listed = vec![schema.clone()];

        // Fresh schema is reported missing.
        let missing = adapter.missing_schemas(&listed).await.unwrap();
        assert_eq!(missing, vec![schema.clone()]);

        // Create it, then it is no longer missing.
        adapter.create_schema(&schema).await.unwrap();
        let missing_after = adapter.missing_schemas(&listed).await.unwrap();
        assert!(missing_after.is_empty(), "schema should exist after create");

        // create_schema is idempotent.
        adapter.create_schema(&schema).await.unwrap();

        // Teardown.
        sqlx::query(&format!("DROP SCHEMA IF EXISTS \"{}\" CASCADE", schema))
            .execute(&*adapter.pool())
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn introspection_schemas_never_reported_missing() {
        let Some(adapter) = test_adapter().await else {
            eprintln!("skip: TEST_DATABASE_URL not set");
            return;
        };

        let listed = vec!["information_schema".to_string(), "pg_catalog".to_string()];
        let missing = adapter.missing_schemas(&listed).await.unwrap();
        assert!(
            missing.is_empty(),
            "introspection schemas must never be reported missing"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // Multi-statement execution tests (Política C, shipped 2026-06-09).
    //
    // All require a fresh test table; setup_test_table() recreates a known
    // schema on each test for full isolation.
    // ─────────────────────────────────────────────────────────────────────

    /// Drop and recreate a fresh test table; returns table name.
    async fn setup_test_table(adapter: &PgPoolAdapter, suffix: &str) -> String {
        let table = format!("public.test_pc_{}", suffix);
        sqlx::query(&format!("DROP TABLE IF EXISTS {}", table))
            .execute(&*adapter.pool())
            .await
            .unwrap();
        sqlx::query(&format!(
            "CREATE TABLE {} (id INT PRIMARY KEY, name TEXT)",
            table
        ))
        .execute(&*adapter.pool())
        .await
        .unwrap();
        table
    }

    async fn teardown(adapter: &PgPoolAdapter, table: &str) {
        let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {}", table))
            .execute(&*adapter.pool())
            .await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn pc_single_insert_returns_rows_affected() {
        let Some(adapter) = test_adapter().await else {
            return;
        };
        let table = setup_test_table(&adapter, "single").await;
        let q = format!("INSERT INTO {} (id, name) VALUES (1, 'a')", table);

        let r = adapter.execute_query(&q, 100, None).await.unwrap();

        assert_eq!(r.row_count, 1);
        assert_eq!(r.output, json!({"rows_affected": 1}));
        teardown(&adapter, &table).await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn pc_multistatement_inserts_aggregate_rows_affected() {
        let Some(adapter) = test_adapter().await else {
            return;
        };
        let table = setup_test_table(&adapter, "multi_ins").await;
        let q = format!(
            "INSERT INTO {t} (id, name) VALUES (10, 'a');\n\
             INSERT INTO {t} (id, name) VALUES (11, 'b');\n\
             INSERT INTO {t} (id, name) VALUES (12, 'c');",
            t = table
        );

        let r = adapter.execute_query(&q, 100, None).await.unwrap();

        // Sum across all 3 mutator statements.
        assert_eq!(r.output, json!({"rows_affected": 3}));
        assert_eq!(r.row_count, 3);
        teardown(&adapter, &table).await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn pc_multistatement_failure_rolls_back_everything() {
        let Some(adapter) = test_adapter().await else {
            return;
        };
        let table = setup_test_table(&adapter, "rollback").await;
        // First INSERT ok, second violates PK → TX must rollback both.
        let q = format!(
            "INSERT INTO {t} (id, name) VALUES (20, 'a');\n\
             INSERT INTO {t} (id, name) VALUES (20, 'b');",
            t = table
        );

        let r = adapter.execute_query(&q, 100, None).await;
        assert!(r.is_err(), "PK violation must propagate as error");

        // Verify row 20 is NOT in the table (rollback worked).
        let check = adapter
            .execute_query(
                &format!("SELECT id FROM {} WHERE id = 20", table),
                100,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            check.output,
            json!([]),
            "rollback must remove the first INSERT too"
        );
        teardown(&adapter, &table).await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn pc_multistatement_insert_then_select_returns_rows() {
        let Some(adapter) = test_adapter().await else {
            return;
        };
        let table = setup_test_table(&adapter, "ins_sel").await;
        let q = format!(
            "INSERT INTO {t} (id, name) VALUES (30, 'a'), (31, 'b');\n\
             SELECT id, name FROM {t} WHERE id IN (30, 31) ORDER BY id;",
            t = table
        );

        let r = adapter.execute_query(&q, 100, None).await.unwrap();

        assert_eq!(r.row_count, 2);
        let arr = r.output.as_array().unwrap();
        assert_eq!(arr[0]["id"], 30);
        assert_eq!(arr[1]["name"], "b");
        teardown(&adapter, &table).await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn pc_multistatement_intermediate_select_rows_discarded() {
        let Some(adapter) = test_adapter().await else {
            return;
        };
        let table = setup_test_table(&adapter, "inter_sel").await;
        let q = format!(
            "SELECT id FROM {t} WHERE id < 100;\n\
             INSERT INTO {t} (id, name) VALUES (40, 'a');\n\
             SELECT id FROM {t} WHERE id = 40;",
            t = table
        );

        let r = adapter.execute_query(&q, 100, None).await.unwrap();

        // Only the final SELECT's rows returned (the table started empty).
        let arr = r.output.as_array().unwrap();
        assert_eq!(arr.len(), 1, "only the final SELECT contributes rows");
        assert_eq!(arr[0]["id"], 40);
        teardown(&adapter, &table).await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn pc_limit_applies_only_to_final_select() {
        let Some(adapter) = test_adapter().await else {
            return;
        };
        let table = setup_test_table(&adapter, "limit").await;
        // Insert 5 rows; SELECT with max_rows=2 → expect truncation.
        let q = format!(
            "INSERT INTO {t} (id, name) VALUES \
              (50,'a'),(51,'b'),(52,'c'),(53,'d'),(54,'e');\n\
             SELECT id FROM {t} WHERE id BETWEEN 50 AND 54 ORDER BY id;",
            t = table
        );

        let r = adapter.execute_query(&q, 2, None).await.unwrap();

        assert!(r.truncated, "LIMIT injection must take effect");
        assert_eq!(r.row_count, 2);
        teardown(&adapter, &table).await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn pc_multiline_formatting_single_statement_works() {
        let Some(adapter) = test_adapter().await else {
            return;
        };
        let table = setup_test_table(&adapter, "multiline").await;
        // Single statement formatted across multiple lines — sqlparser
        // already handles whitespace; this just confirms Política C doesn't
        // regress single-statement multi-line.
        let q = format!(
            "INSERT INTO {}\n  (id, name)\nVALUES\n  (60, 'a'),\n  (61, 'b')",
            table
        );

        let r = adapter.execute_query(&q, 100, None).await.unwrap();

        assert_eq!(r.output, json!({"rows_affected": 2}));
        teardown(&adapter, &table).await;
    }

    /// Regression: Postgres NUMERIC columns must marshall as JSON numbers,
    /// not nulls. Pre-fix (before 2026-06-09 NUMERIC bigdecimal feature)
    /// returned `null` because sqlx can't decode NUMERIC into `f64` directly —
    /// only into `BigDecimal`. Detected during the LLM-in-the-loop E2E test
    /// `tests/graphs/agents/sql_multistatement_e2e_llm.json`.
    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn pc_numeric_column_marshalls_as_f64_not_null() {
        let Some(adapter) = test_adapter().await else {
            return;
        };
        // Custom-schema test: needs NUMERIC, not the helper's INT/TEXT shape.
        let table = "public.test_pc_numeric";
        let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {}", table))
            .execute(&*adapter.pool())
            .await;
        sqlx::query(&format!(
            "CREATE TABLE {} (id INT PRIMARY KEY, amount NUMERIC(10,2), big_amount NUMERIC)",
            table
        ))
        .execute(&*adapter.pool())
        .await
        .unwrap();

        let q = format!(
            "INSERT INTO {t} (id, amount, big_amount) VALUES \
                (1, 100.00, 12345.6789),\n\
                (2,   1.81, 0.5),\n\
                (3, 999.99, 1234567890.123);\n\
             SELECT id, amount, big_amount FROM {t} ORDER BY id;",
            t = table
        );
        let r = adapter.execute_query(&q, 100, None).await.unwrap();

        let arr = r.output.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        // Values must be JSON numbers, NOT JSON nulls.
        assert!(
            arr[0]["amount"].is_number(),
            "amount must be a number, got: {}",
            arr[0]["amount"]
        );
        assert!(
            arr[0]["big_amount"].is_number(),
            "big_amount must be a number"
        );
        // Exact comparisons (f64 reproduces these values exactly).
        assert_eq!(arr[0]["amount"].as_f64(), Some(100.0));
        assert_eq!(arr[1]["amount"].as_f64(), Some(1.81));
        assert_eq!(arr[2]["amount"].as_f64(), Some(999.99));
        // Precision within f64 (15-17 sig digits).
        assert!((arr[0]["big_amount"].as_f64().unwrap() - 12345.6789).abs() < 1e-9);

        let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {}", table))
            .execute(&*adapter.pool())
            .await;
    }

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

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn setup_sql_handles_semicolons_inside_string_literals() {
        let Some(adapter) = test_adapter().await else {
            eprintln!("skip: TEST_DATABASE_URL not set");
            return;
        };
        let schema = unique_schema("colmena_setup_semic");
        // The seed value contains a semicolon inside a string literal. A naive
        // split on `;` would mangle this; Postgres parses it correctly.
        let sql = format!(
            "CREATE SCHEMA IF NOT EXISTS {s};\n\
             CREATE TABLE IF NOT EXISTS {s}.notas (id SERIAL PRIMARY KEY, nota TEXT UNIQUE NOT NULL);\n\
             INSERT INTO {s}.notas (nota) VALUES ('a; b; c') ON CONFLICT (nota) DO NOTHING;",
            s = schema
        );

        adapter.execute_setup_sql(&sql).await.expect("setup_sql with semicolon-in-literal");

        let nota: String =
            sqlx::query_scalar(&format!("SELECT nota FROM {}.notas", schema))
                .fetch_one(&*adapter.pool())
                .await
                .unwrap();
        assert_eq!(nota, "a; b; c", "semicolons inside string literals must be preserved");

        sqlx::query(&format!("DROP SCHEMA {} CASCADE", schema))
            .execute(&*adapter.pool())
            .await
            .ok();
    }
}
