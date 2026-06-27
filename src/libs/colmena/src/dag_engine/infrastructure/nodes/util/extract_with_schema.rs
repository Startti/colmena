use crate::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
use crate::llm::application::{AgentRunParams, AgentService};
use crate::llm::domain::{
    ConversationKey, LlmConfig, LlmError, LlmMessage, LlmProvider, NodeIdPath, ProviderKind,
    SessionId, ToolCall, ToolDefinition, ToolExecutor, ToolResult,
};
use crate::llm::infrastructure::persistence::in_memory_conversation_repository::InMemoryConversationRepository;
use crate::llm::infrastructure::LlmProviderFactory;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use super::inline_schema::validate_against_inline_schema;

/// Inputs the helper needs to make one structured-output LLM call.
pub struct ExtractInput<'a> {
    pub provider_kind: ProviderKind,
    pub api_key: String,
    pub model: Option<String>,
    pub system_message: String,
    pub user_text: String,
    pub inline_schema: &'a Value,
    pub temperature: Option<f32>,
    pub observer: Option<Arc<dyn ExecutionObserver>>,
}

/// Calls the LLM once with the given system+user messages, strips markdown
/// code fences from the response, parses JSON, and validates against the
/// inline schema. Returns the parsed JSON on success.
pub async fn extract_with_schema<'a>(
    input: ExtractInput<'a>,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let provider = LlmProvider::new(input.provider_kind.clone(), input.api_key, input.model)?;
    let mut llm_config = LlmConfig::new(provider);
    llm_config = llm_config.with_temperature(input.temperature.unwrap_or(0.1))?;

    let llm_repo = LlmProviderFactory::create(input.provider_kind);
    let conversation_repo = Arc::new(InMemoryConversationRepository::new());
    let agent_service = AgentService::new(llm_repo, conversation_repo);

    let tid_val = uuid::Uuid::new_v4().to_string();
    let tid = ConversationKey {
        session_id: SessionId(tid_val.clone()),
        agent_session_id: None,
        node_id: NodeIdPath(tid_val),
    };
    let messages = vec![
        LlmMessage::system(input.system_message)?,
        LlmMessage::user(input.user_text)?,
    ];

    struct EmptyToolExecutor;
    #[async_trait]
    impl ToolExecutor for EmptyToolExecutor {
        async fn execute(&self, _: &ToolCall) -> Result<ToolResult, LlmError> {
            Err(LlmError::ToolExecutionFailed {
                message: "No tools available".into(),
            })
        }
        async fn available_tools(&self) -> Vec<ToolDefinition> {
            vec![]
        }
    }

    let params = AgentRunParams {
        session_id: &tid,
        prompt: None,
        messages: Some(messages),
        config: llm_config,
        tools: vec![],
        tool_executor: &EmptyToolExecutor,
        max_turns: Some(1),
        max_tool_repeats: None,
        on_token: None,
        tools_provider: None,
        attachment_resolver: None,
        agent_session_id: None,
        lazy_catalog_names: None,
    };

    let response = agent_service.run(params).await?;

    if let Some(obs) = input.observer.clone() {
        if let Some(usage) = response.usage() {
            obs.on_event(NodeEvent::LlmUsage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                thinking_tokens: usage.thinking_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                cache_write_tokens: usage.cache_write_tokens,
            });
        }
    }

    let raw = response.content();
    let parsed = parse_and_validate(raw, input.inline_schema)?;
    Ok(parsed)
}

/// Strips markdown code fences from a string and parses it as JSON,
/// then validates against the inline schema. Public so callers (and
/// tests) can drive it without an LLM.
///
/// If `inline_schema` is an empty JSON object (`{}`), the validation step
/// is skipped — this keeps `extraction.rs`'s legacy behavior of "no
/// schema validation on output" intact when it routes through this helper.
pub fn parse_and_validate(
    raw: &str,
    inline_schema: &Value,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let mut s = raw.trim();
    if let Some(stripped) = s.strip_prefix("```json") {
        s = stripped;
    } else if let Some(stripped) = s.strip_prefix("```") {
        s = stripped;
    }
    if let Some(stripped) = s.strip_suffix("```") {
        s = stripped;
    }
    let s = s.trim();
    let parsed: Value = serde_json::from_str(s)
        .map_err(|e| format!("failed to parse LLM response as JSON: {}. raw: {}", e, raw))?;
    if !inline_schema.as_object().is_some_and(|o| o.is_empty()) {
        validate_against_inline_schema(&parsed, inline_schema)
            .map_err(|e| format!("schema validation failed: {}", e))?;
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_and_validate_strips_json_fence() {
        let raw = "```json\n{\"intent\":\"sales\"}\n```";
        let schema = json!({ "intent": { "type": "string", "required": true } });
        let out = parse_and_validate(raw, &schema).unwrap();
        assert_eq!(out["intent"], json!("sales"));
    }

    #[test]
    fn parse_and_validate_strips_plain_fence() {
        let raw = "```\n{\"intent\":\"x\"}\n```";
        let schema = json!({ "intent": { "type": "string", "required": true } });
        assert!(parse_and_validate(raw, &schema).is_ok());
    }

    #[test]
    fn parse_and_validate_accepts_unwrapped_json() {
        let raw = "  {\"intent\":\"sales\"}  ";
        let schema = json!({ "intent": { "type": "string", "required": true } });
        assert!(parse_and_validate(raw, &schema).is_ok());
    }

    #[test]
    fn parse_and_validate_fails_on_invalid_json() {
        let raw = "this is not json";
        let schema = json!({ "intent": { "type": "string", "required": true } });
        let err = parse_and_validate(raw, &schema).unwrap_err().to_string();
        assert!(err.contains("failed to parse LLM response as JSON"));
    }

    #[test]
    fn parse_and_validate_fails_on_schema_mismatch() {
        let raw = r#"{"intent": 42}"#;
        let schema = json!({ "intent": { "type": "string", "required": true } });
        let err = parse_and_validate(raw, &schema).unwrap_err().to_string();
        assert!(err.contains("schema validation failed"));
        assert!(err.contains("expected type 'string'"));
    }

    #[test]
    fn parse_and_validate_fails_on_missing_required_field() {
        let raw = "{}";
        let schema = json!({ "intent": { "type": "string", "required": true } });
        let err = parse_and_validate(raw, &schema).unwrap_err().to_string();
        assert!(err.contains("required field 'intent'"));
    }

    #[test]
    fn parse_and_validate_skips_validation_for_empty_schema() {
        let raw = r#"{"anything": "goes", "extra": 42}"#;
        let schema = json!({});
        let out = parse_and_validate(raw, &schema).unwrap();
        assert_eq!(out["extra"], json!(42));
    }
}
