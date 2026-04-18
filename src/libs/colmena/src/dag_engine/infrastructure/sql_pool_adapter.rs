//! PostgreSQL connection pool adapter.
//!
//! Wraps `sqlx::PgPool` and implements `SqlConnectionPort`. Manages connection pooling,
//! runtime limits (`statement_timeout`, `work_mem`), and query execution with row capping.

use crate::dag_engine::domain::sql_errors::SqlNodeError;
use crate::dag_engine::domain::sql_ports::{QueryResult, SqlConnectionPort, TableInfo};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Column, PgPool, Row, TypeInfo};
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

    /// Get a reference to the underlying pool lock for sharing with other adapters.
    pub fn pool_ref(&self) -> Arc<RwLock<Option<PgPool>>> {
        self.pool.clone()
    }

    /// Get a reference to the pool, returning an error if not initialized.
    async fn get_pool(&self) -> Result<PgPool, SqlNodeError> {
        let guard = self.pool.read().await;
        guard.clone().ok_or_else(|| {
            SqlNodeError::ConnectionError("Pool not initialized. Call connect() first.".to_string())
        })
    }
}

impl Default for PgPoolAdapter {
    fn default() -> Self {
        Self::new()
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

        let trimmed = query.trim_start().to_uppercase();
        let is_select = trimmed.starts_with("SELECT") || trimmed.starts_with("WITH");

        if is_select {
            // For SELECT: add LIMIT to enforce max_rows
            let limited_query = if max_rows > 0 && !trimmed.contains("LIMIT") {
                format!("{} LIMIT {}", query.trim_end_matches(';'), max_rows + 1)
            } else {
                query.to_string()
            };

            // Apply runtime limits per-transaction and execute
            let mut tx = pool.begin().await.map_err(|e| {
                SqlNodeError::ExecutionError(format!("Failed to begin transaction: {}", e))
            })?;

            sqlx::query(&format!(
                "SET LOCAL statement_timeout = {}",
                timeout_ms
            ))
            .execute(&mut *tx)
            .await
            .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;

            sqlx::query(&format!("SET LOCAL work_mem = '{}MB'", work_mem))
                .execute(&mut *tx)
                .await
                .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;

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
            // For mutations: execute and return rows_affected
            let result = sqlx::query(query)
                .execute(&pool)
                .await
                .map_err(|e| SqlNodeError::ExecutionError(format!("{}", e)))?;

            let rows_affected = result.rows_affected();

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
        self.pool
            .try_read()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }
}
