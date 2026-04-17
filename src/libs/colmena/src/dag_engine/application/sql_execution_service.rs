//! Orchestrates the SQL execution pipeline: validate → critic → execute → feedback.
//!
//! This is the application-layer use case. It depends only on domain ports (traits),
//! not on infrastructure adapters.

use crate::dag_engine::domain::sql_errors::SqlNodeError;
use crate::dag_engine::domain::sql_permissions::SqlPermissions;
use crate::dag_engine::domain::sql_ports::{
    FunctionRegistryPort, SqlConnectionPort, SqlCriticPort, SqlValidatorPort,
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

        let all_warnings = validation.warnings;

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
