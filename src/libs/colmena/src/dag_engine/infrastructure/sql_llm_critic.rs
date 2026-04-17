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

        // Resolve provider kind from string
        let provider_kind = ProviderKind::from_str(&self.provider)
            .map_err(|e| SqlNodeError::ConfigError(format!("Invalid critic provider: {}", e)))?;

        // Build LlmProvider (holds api_key + model)
        let llm_provider =
            LlmProvider::new(provider_kind.clone(), self.api_key.clone(), Some(self.model.clone()))
                .map_err(|e| {
                    SqlNodeError::ConfigError(format!("Invalid critic LLM config: {}", e))
                })?;

        // Build LlmConfig with low temperature for deterministic responses
        let config = LlmConfig::new(llm_provider)
            .with_temperature(0.0)
            .map_err(|e| SqlNodeError::ConfigError(format!("{}", e)))?
            .with_max_tokens(500)
            .map_err(|e| SqlNodeError::ConfigError(format!("{}", e)))?;

        // Build messages
        let messages = vec![
            LlmMessage::system(CRITIC_SYSTEM_PROMPT.to_string()).map_err(|e| {
                SqlNodeError::ExecutionError(format!("Failed to create system message: {}", e))
            })?,
            LlmMessage::user(user_message).map_err(|e| {
                SqlNodeError::ExecutionError(format!("Failed to create user message: {}", e))
            })?,
        ];

        // Build request (non-streaming)
        let request = LlmRequest::new(messages, config, false).map_err(|e| {
            SqlNodeError::ExecutionError(format!("Failed to create LLM request: {}", e))
        })?;

        // Create provider adapter via factory and call
        let llm_repo = LlmProviderFactory::create(provider_kind);
        let response = llm_repo
            .call(request)
            .await
            .map_err(|e| SqlNodeError::ExecutionError(format!("LLM critic call failed: {}", e)))?;

        // Parse the JSON response — fail-open: if parsing fails, assume OK
        let content = response.content().trim();
        let parsed: serde_json::Value = serde_json::from_str(content).unwrap_or_else(|_| {
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
