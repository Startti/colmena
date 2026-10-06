//! LLM-based SQL critic adapter.
//!
//! Sends SQL queries to a secondary LLM for security and optimization analysis.
//! Activated only when `guardrail_llm.enabled: true` in the node config.
//!
//! Uses `LlmProviderFactory` to create a provider adapter and `LlmRepository::call()`
//! to make a single non-streaming request. No conversation persistence needed.

use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::domain::sql_errors::SqlNodeError;
use crate::dag_engine::domain::sql_ports::{CriticResult, SqlCriticPort};
use crate::dag_engine::infrastructure::nodes::util::billed_llm::billed;
use crate::llm::domain::{LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderKind};
use crate::llm::infrastructure::LlmProviderFactory;
use std::str::FromStr;
use std::sync::Arc;

/// Adapter that uses an LLM to analyze SQL queries for security and optimization.
pub struct LlmCriticAdapter {
    provider: String,
    model: String,
    api_key: String,
    /// The `sql` node's observer, billed for each critic call.
    observer: Option<Arc<dyn ExecutionObserver>>,
}

impl LlmCriticAdapter {
    pub fn new(provider: String, model: String, api_key: String) -> Self {
        Self {
            provider,
            model,
            api_key,
            observer: None,
        }
    }

    /// Bills each critic call to `observer` (the node running the query).
    pub fn with_observer(mut self, observer: Option<Arc<dyn ExecutionObserver>>) -> Self {
        self.observer = observer;
        self
    }
}

const CRITIC_SYSTEM_PROMPT: &str = include_str!("../../../text/prompts/sql_llm_critic.md");

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
        let llm_provider = LlmProvider::new(
            provider_kind.clone(),
            self.api_key.clone(),
            Some(self.model.clone()),
        )
        .map_err(|e| SqlNodeError::ConfigError(format!("Invalid critic LLM config: {}", e)))?;

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
        let llm_repo = billed(
            LlmProviderFactory::create(provider_kind),
            self.observer.clone(),
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::observer::NodeEvent;
    use crate::llm::domain::{LlmRequestId, LlmResponse, LlmUsage, MockLlmRepository};
    use crate::llm::infrastructure::OverrideGuard;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<u32>>);

    impl ExecutionObserver for Recorder {
        fn on_event(&self, event: NodeEvent) {
            if let NodeEvent::LlmUsage { prompt_tokens, .. } = event {
                self.0.lock().unwrap().push(prompt_tokens);
            }
        }
    }

    /// The critic's call is billed to the `sql` node, once.
    #[tokio::test]
    async fn the_critic_call_is_billed_to_the_node() {
        let mut model = MockLlmRepository::new();
        model.expect_call().times(1).returning(|request| {
            let provider = request.config().provider().clone();
            let id = LlmRequestId::from_string("r".into()).unwrap();
            let response = LlmResponse::new(id, r#"{"security":"ok"}"#.into(), provider)?;
            Ok(response.with_usage(LlmUsage::new(321, 4)))
        });
        let _guard = OverrideGuard::install(Arc::new(model));
        let recorder = Arc::new(Recorder::default());
        let critic = LlmCriticAdapter::new("openai".into(), "m".into(), "k".into())
            .with_observer(Some(recorder.clone()));
        let result = critic.analyze("SELECT 1", "").await.unwrap();
        assert!(result.security_ok);
        assert_eq!(*recorder.0.lock().unwrap(), vec![321]);
    }
}
