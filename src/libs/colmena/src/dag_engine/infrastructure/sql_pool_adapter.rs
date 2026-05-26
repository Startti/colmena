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

#[async_trait::async_trait]
impl SqlConnectionPort for PgPoolAdapter {
    async fn execute_query(
        &self,
        query: &str,
        max_rows: u64,
        tenant_user_id: Option<&str>,
    ) -> Result<QueryResult, SqlNodeError> {
        let pool = &*self.pool;
        let timeout_ms = self.statement_timeout_ms;
        let work_mem = self.work_mem_mb;

        let parsed = crate::dag_engine::infrastructure::sql_ast::parse(query).ok();
        // First statement classifier — execution-path decisions only consider the
        // outer shape, but the validator (which ran before us) checked every statement.
        let first_stmt = parsed.as_ref().and_then(|s| s.first());
        let is_select = first_stmt
            .map(crate::dag_engine::infrastructure::sql_ast::is_query)
            .unwrap_or(false);
        let already_has_limit = first_stmt
            .map(crate::dag_engine::infrastructure::sql_ast::query_has_limit)
            .unwrap_or(false);

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
            let limited_query = if max_rows > 0 && !already_has_limit {
                format!("{} LIMIT {}", query.trim_end_matches(';'), max_rows + 1)
            } else {
                query.to_string()
            };

            let rows = sqlx::query(&limited_query)
                .fetch_all(&mut *tx)
                .await
                .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;

            tx.commit()
                .await
                .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to commit: {}", e)))?;

            let mut json_rows: Vec<Value> = Vec::new();
            for row in &rows {
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

            tx.commit()
                .await
                .map_err(|e| SqlNodeError::ExecutionError(format!("Failed to commit: {}", e)))?;

            let rows_affected = result.rows_affected();

            use sqlparser::ast::Statement;
            let (output, row_count) = match first_stmt {
                Some(Statement::CreateFunction(_)) => (json!({ "created": true }), 0u64),
                Some(Statement::CreateTable(_)) => {
                    (json!({ "created": true, "type": "table" }), 0u64)
                }
                _ => (json!({ "rows_affected": rows_affected }), rows_affected),
            };
            Ok(QueryResult {
                output,
                row_count,
                truncated: false,
            })
        }
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

    fn is_connected(&self) -> bool {
        true
    }
}
