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

        // Execute each DDL statement separately — PostgreSQL doesn't allow
        // mixing DDL with COMMENT in a single multi-statement string via sqlx.
        sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", schema))
            .execute(&pool)
            .await
            .map_err(|e| {
                SqlNodeError::ExecutionError(format!("Failed to create schema: {}", e))
            })?;

        sqlx::query(&format!(
            "CREATE TABLE IF NOT EXISTS {schema}.function_registry (
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
            )",
            schema = schema
        ))
        .execute(&pool)
        .await
        .map_err(|e| {
            SqlNodeError::ExecutionError(format!("Failed to create function_registry: {}", e))
        })?;

        sqlx::query(&format!(
            "COMMENT ON TABLE {}.function_registry IS \
             'Registry of SQL functions created by AI agents in the sandbox schema'",
            schema
        ))
        .execute(&pool)
        .await
        .map_err(|e| {
            SqlNodeError::ExecutionError(format!(
                "Failed to comment on function_registry: {}",
                e
            ))
        })?;

        sqlx::query(&format!(
            "CREATE TABLE IF NOT EXISTS {schema}.query_feedback (
                id SERIAL PRIMARY KEY,
                session_id TEXT NOT NULL,
                query_text TEXT NOT NULL,
                feedback_type TEXT NOT NULL,
                source TEXT NOT NULL,
                message TEXT NOT NULL,
                created_at TIMESTAMPTZ DEFAULT NOW()
            )",
            schema = schema
        ))
        .execute(&pool)
        .await
        .map_err(|e| {
            SqlNodeError::ExecutionError(format!("Failed to create query_feedback: {}", e))
        })?;

        sqlx::query(&format!(
            "COMMENT ON TABLE {}.query_feedback IS \
             'Feedback history from static validator and LLM critic on agent queries'",
            schema
        ))
        .execute(&pool)
        .await
        .map_err(|e| {
            SqlNodeError::ExecutionError(format!("Failed to comment on query_feedback: {}", e))
        })?;

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
        .map_err(|e| {
            SqlNodeError::ExecutionError(format!("Failed to register function: {}", e))
        })?;

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
        .map_err(|e| {
            SqlNodeError::ExecutionError(format!("Failed to list functions: {}", e))
        })?;

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
            "INSERT INTO {}.query_feedback \
             (session_id, query_text, feedback_type, source, message) \
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
        .map_err(|e| {
            SqlNodeError::ExecutionError(format!("Failed to record feedback: {}", e))
        })?;

        Ok(())
    }
}
