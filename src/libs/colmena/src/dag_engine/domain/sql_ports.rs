//! Domain ports (traits) for the SQL node's hexagonal architecture.
//!
//! Each trait defines a capability boundary. Infrastructure adapters implement
//! these traits; the application service and node depend only on the traits.

use crate::dag_engine::domain::sql_errors::SqlNodeError;
use crate::dag_engine::domain::sql_permissions::SqlPermissions;
use serde_json::Value;

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
    /// Execute a SQL query and return results as JSON.
    /// If `tenant_user_id` is Some, runs `SET LOCAL app.current_user_id` in the same transaction.
    async fn execute_query(
        &self,
        query: &str,
        max_rows: u64,
        tenant_user_id: Option<&str>,
    ) -> Result<QueryResult, SqlNodeError>;

    /// Load table metadata (names + comments) for the given schemas.
    async fn load_table_metadata(&self, schemas: &[String])
        -> Result<Vec<TableInfo>, SqlNodeError>;

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
    fn validate(&self, query: &str, permissions: &SqlPermissions) -> ValidationResult;
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
