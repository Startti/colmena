use crate::colmena_log;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

use crate::llm::application::AgentService;
use crate::llm::domain::{
    LlmConfig, LlmMessage, LlmProvider, LlmStreamPart, ProviderKind, SessionId,
};
use crate::llm::infrastructure::persistence::in_memory_conversation_repository::InMemoryConversationRepository;
use crate::llm::infrastructure::LlmProviderFactory;

// ---------------------------------------------------------------------------
// Fixed schema — always the same for every CriticNode.
// After reviewing the current task's result, the Critic decides:
//   - task_ok   → result is acceptable, move on
//   - feedback  → when task_ok=false, explains what was wrong and what the agent
//                 must do differently on the next attempt
//   - suspend   → pause and ask the user a clarifying question
// ---------------------------------------------------------------------------
fn critic_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "task_ok": {
                "type": "boolean",
                "description": "Set to true if the current task result is satisfactory and no further action is needed for it."
            },
            "feedback": {
                "type": "string",
                "description": "Only populate when task_ok is false. Write a concise, actionable explanation of what was wrong or missing in the result and exactly what the agent must do differently in the next attempt. Leave empty string when task_ok is true."
            },
            "suspend": {
                "type": "boolean",
                "description": "Set to true if you need to pause execution and ask the user a clarifying question before continuing."
            },
            "question": {
                "type": "string",
                "description": "The question to ask the user. Required when suspend is true."
            }
        },
        "required": ["task_ok", "feedback", "suspend"]
    })
}

/// Default system prompt baked into every CriticNode.
const DEFAULT_CRITIC_SYSTEM_MSG: &str = include_str!("prompts/critic_system.md");

pub struct CriticNode;

impl Default for CriticNode {
    fn default() -> Self {
        Self::new()
    }
}

impl CriticNode {
    pub fn new() -> Self {
        Self
    }

    fn resolve_env_var(value: &str) -> Result<String, String> {
        if value.starts_with("${") && value.ends_with('}') {
            let var_name = &value[2..value.len() - 1];
            std::env::var(var_name)
                .map_err(|_| format!("Environment variable '{}' not found", var_name))
        } else {
            Ok(value.to_string())
        }
    }
}

#[async_trait]
impl ExecutableNode for CriticNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        // --- 1. Resolve Provider Configuration ---
        let provider_str = config
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or("CriticNode: Missing 'provider' in config")?;

        let provider_kind = match provider_str.to_lowercase().as_str() {
            "openai" => ProviderKind::OpenAi,
            "gemini" => ProviderKind::Gemini,
            "anthropic" => ProviderKind::Anthropic,
            _ => return Err(format!("CriticNode: Invalid provider '{}'.", provider_str).into()),
        };

        let api_key_raw = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or("CriticNode: Missing 'api_key' in config")?;
        let api_key = Self::resolve_env_var(api_key_raw)?;

        let model = config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Verbose flag for debugging.
        let verbose = config
            .get("verbose")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // --- 2. Collect input texts ---
        // All keys that start with "texts." are treated as context for the Critic.
        let mut formatted_texts = String::new();
        for (key, val) in inputs.iter() {
            if key == "system_message" {
                continue;
            }
            let label = key.strip_prefix("texts.").unwrap_or(key.as_str());
            let text_str = match val {
                Value::String(s) => s.clone(),
                Value::Null => continue,
                _ => val.to_string(),
            };
            let clean = if text_str.starts_with('"') && text_str.ends_with('"') {
                text_str[1..text_str.len() - 1].to_string()
            } else {
                text_str
            };
            formatted_texts.push_str(&format!("# {}\n\n{}\n\n", label, clean));
        }

        // Also accept texts defined in config.texts
        if let Some(texts_obj) = config.get("texts").and_then(|v| v.as_object()) {
            for (key, val) in texts_obj {
                if let Some(s) = val.as_str() {
                    formatted_texts.push_str(&format!("# {}\n\n{}\n\n", key, s));
                }
            }
        }

        if formatted_texts.is_empty() {
            colmena_log!("⚠️ [CriticNode] Skipped — no input texts provided.");
            return Ok(Value::Null);
        }

        // --- 4. Compose system message ---
        let extra_system_msg = inputs
            .get("system_message")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("system_message").and_then(|v| v.as_str()))
            .unwrap_or("");

        let extra_section = if !extra_system_msg.is_empty() {
            format!("\n\nAdditional critic guidelines:\n{}", extra_system_msg)
        } else {
            String::new()
        };

        let schema = critic_schema();

        let system_message = format!(
            "{}{}\n\nYou MUST output JSON matching this schema:\n{}",
            DEFAULT_CRITIC_SYSTEM_MSG,
            extra_section,
            serde_json::to_string_pretty(&schema)?
        );

        if verbose {
            colmena_log!("\n═══════════════════════════════════════");
            colmena_log!("🔎 [CriticNode] VERBOSE — System Prompt:");
            colmena_log!("───────────────────────────────────────");
            colmena_log!("{}", system_message);
            colmena_log!("───────────────────────────────────────");
            colmena_log!("Context Texts:\n{}", formatted_texts);
            colmena_log!("═══════════════════════════════════════\n");
        } else {
            colmena_log!("🔎 [CriticNode] Reviewing task result...");
        }

        // --- 5. Call LLM ---
        let provider = LlmProvider::new(provider_kind.clone(), api_key, model)?;
        let mut llm_config = LlmConfig::new(provider);
        llm_config = llm_config.with_temperature(0.1)?;

        let llm_repo = LlmProviderFactory::create(provider_kind);
        let conversation_repo = Arc::new(InMemoryConversationRepository::new());
        let agent_service = AgentService::new(llm_repo, conversation_repo);
        let tid = SessionId(uuid::Uuid::new_v4().to_string());

        let messages = vec![
            LlmMessage::system(system_message)?,
            LlmMessage::user(formatted_texts)?,
        ];

        struct EmptyToolExecutor;
        #[async_trait]
        impl crate::llm::domain::ToolExecutor for EmptyToolExecutor {
            async fn execute(
                &self,
                _tc: &crate::llm::domain::ToolCall,
            ) -> Result<crate::llm::domain::ToolResult, crate::llm::domain::LlmError> {
                Err(crate::llm::domain::LlmError::ToolExecutionFailed {
                    message: "No tools".into(),
                })
            }
            async fn available_tools(&self) -> Vec<crate::llm::domain::ToolDefinition> {
                vec![]
            }
        }

        // Build streaming callback — only when streaming is explicitly enabled in config
        let streaming = config
            .get("streaming")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let observer_for_stream = _observer.clone();
        let on_token: Option<Box<dyn Fn(LlmStreamPart) + Send + Sync>> = if streaming {
            if let Some(obs) = observer_for_stream {
                Some(Box::new(move |part: LlmStreamPart| {
                    use crate::dag_engine::domain::observer::NodeEvent;
                    match part {
                        LlmStreamPart::Content(token) => {
                            obs.on_event(NodeEvent::LlmToken { token })
                        }
                        LlmStreamPart::Usage(usage) => obs.on_event(NodeEvent::LlmUsage {
                            prompt_tokens: usage.prompt_tokens,
                            completion_tokens: usage.completion_tokens,
                            thinking_tokens: usage.thinking_tokens,
                            cache_read_tokens: usage.cache_read_tokens,
                            cache_write_tokens: usage.cache_write_tokens,
                        }),
                        LlmStreamPart::LlmMessageStart => obs.on_event(NodeEvent::LlmMessageStart),
                        LlmStreamPart::LlmMessageFinish(usage) => {
                            obs.on_event(NodeEvent::LlmMessageFinish(usage))
                        }
                        _ => {}
                    }
                }))
            } else {
                None
            }
        } else {
            None
        };

        let params = crate::llm::application::AgentRunParams {
            session_id: &tid,
            prompt: String::new(),
            messages: Some(messages),
            config: llm_config,
            tools: vec![],
            tool_executor: &EmptyToolExecutor,
            max_iterations: Some(1),
            on_token,
        };

        let response = agent_service.run(params).await?;

        let raw = response.content();

        if verbose {
            colmena_log!("\n═══════════════════════════════════════");
            colmena_log!("🔎 [CriticNode] VERBOSE — Raw LLM Response:");
            colmena_log!("───────────────────────────────────────");
            colmena_log!("{}", raw);
            colmena_log!("═══════════════════════════════════════\n");
        }

        // --- 6. Parse JSON response ---
        let mut clean = raw.trim();
        if clean.starts_with("```json") {
            clean = clean.trim_start_matches("```json");
        } else if clean.starts_with("```") {
            clean = clean.trim_start_matches("```");
        }
        if clean.ends_with("```") {
            clean = clean.trim_end_matches("```");
        }
        let clean = clean.trim();

        let parsed: Value = serde_json::from_str(clean).map_err(|e| {
            format!(
                "CriticNode: Failed to parse LLM response as JSON: {}. Raw: {}",
                e, raw
            )
        })?;

        let task_ok = parsed
            .get("task_ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let feedback = parsed
            .get("feedback")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let suspend = parsed
            .get("suspend")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let question = parsed.get("question").cloned().unwrap_or(Value::Null);

        colmena_log!(
            "🔎 [CriticNode] Decision → task_ok={}, has_feedback={}, suspend={}",
            task_ok,
            !feedback.is_empty(),
            suspend
        );

        Ok(json!({
            "result": task_ok,
            "extra_info": {
                "task_ok":  task_ok,
                "feedback": feedback,
                "suspend":  suspend,
                "question": question,
                "__colmena_status": if suspend { "SUSPENDED" } else { "OK" }
            }
        }))
    }

    fn description(&self) -> Option<&str> {
        Some("Reviews the result of a specialist agent and decides if the task is complete, needs more work, or requires user input.")
    }

    fn default_output(&self) -> Option<&str> {
        Some("result")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "critic",
            "config": {
                "provider": "openai | gemini | anthropic",
                "api_key": "string or ${ENV_VAR}",
                "model": "optional model name",
                "verbose": "bool (default false)",
                "system_message": "optional extra instructions concatenated with the default prompt"
            },
            "inputs": {
                "texts.*": "Named text inputs for the Critic to review (e.g. texts.agent_result, texts.current_task)"
            },
            "outputs": {
                "task_ok":  "bool — true if the result is satisfactory",
                "feedback": "string — when task_ok=false, explains what was wrong and what to do differently",
                "suspend":  "bool — true if user input is needed",
                "question": "string — the question to ask the user when suspend=true",
                "__colmena_status": "SUSPENDED | OK"
            }
        })
    }
}
