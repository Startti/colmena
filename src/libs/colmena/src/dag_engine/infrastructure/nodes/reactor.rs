use crate::colmena_log;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

use crate::llm::application::AgentService;
use crate::llm::domain::{LlmConfig, LlmMessage, LlmProvider, ProviderKind, SessionId};
use crate::llm::infrastructure::persistence::in_memory_conversation_repository::InMemoryConversationRepository;
use crate::llm::infrastructure::LlmProviderFactory;

// ---------------------------------------------------------------------------
// ReactorNode — the final reviewer in a multi-agent DAG.
//
// Unlike CriticNode (which only reviews intermediate tasks), the Reactor
// also produces the FINAL RESPONSE to deliver to the user.
//
// Fixed schema output:
//   task_ok   → bool    — is the synthesis acceptable?
//   add_tasks → array   — follow-up tasks if something is wrong/missing
//   suspend   → bool    — pause and ask the user a question
//   question  → string  — the question when suspend=true
//   response  → string  — the final polished answer for the user (when task_ok=true)
// ---------------------------------------------------------------------------

fn reactor_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "task_ok": {
                "type": "boolean",
                "description": "Set to true if the synthesis is complete and satisfactory. When true, you MUST also populate 'response'."
            },
            "response": {
                "type": "string",
                "description": "The final, polished answer to show the user. Only populate when task_ok is true. Write it in clear, friendly language suitable to deliver directly to the user. Leave empty string if task_ok is false."
            },
            "add_tasks": {
                "type": "array",
                "description": "Follow-up tasks to add if the synthesis is incomplete or needs more information.",
                "items": {
                    "type": "object",
                    "properties": {
                        "task": {
                            "type": "string",
                            "description": "A clear, self-contained description of the additional task."
                        },
                        "assigned_to": {
                            "type": "string",
                            "description": "The agent node ID that should handle this task."
                        }
                    },
                    "required": ["task", "assigned_to"]
                }
            },
            "suspend": {
                "type": "boolean",
                "description": "Set to true if you need to ask the user a clarifying question before producing the final response."
            },
            "question": {
                "type": "string",
                "description": "The question to ask the user. Required when suspend is true."
            }
        },
        "required": ["task_ok", "response", "add_tasks", "suspend"]
    })
}

/// Default system prompt for every ReactorNode.
const DEFAULT_REACTOR_SYSTEM_MSG: &str = "\
You are the final reviewer in a multi-agent workflow. You receive a synthesized \
response produced by specialist agents and you decide:\n\
\n\
1. If it is COMPLETE and CORRECT → set 'task_ok' to true and write the final, \
   polished, user-facing 'response'. Improve the wording if needed but keep all \
   the information. Do NOT just say 'looks good' — actually write the full response.\n\
2. If something is MISSING or INCORRECT → set 'task_ok' to false and add specific \
   follow-up tasks in 'add_tasks'.\n\
3. If you need MORE INFORMATION from the user → set 'suspend' to true and provide \
   a clear 'question'.\n\
\n\
Output ONLY valid JSON matching the schema. Do NOT include markdown or code fences.";

pub struct ReactorNode {
    #[allow(dead_code)]
    task_memory_repo: Option<Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>>,
}

impl ReactorNode {
    pub fn new(
        task_memory_repo: Option<
            Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>,
        >,
    ) -> Self {
        Self { task_memory_repo }
    }

    fn resolve_env_var(value: &str) -> Result<String, String> {
        if value.starts_with("${") && value.ends_with('}') {
            let var_name = &value[2..value.len() - 1];
            std::env::var(var_name)
                .map_err(|_| format!("ReactorNode: Environment variable '{}' not found", var_name))
        } else {
            Ok(value.to_string())
        }
    }
}

#[async_trait]
impl ExecutableNode for ReactorNode {
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
            .ok_or("ReactorNode: Missing 'provider' in config")?;

        let provider_kind = match provider_str.to_lowercase().as_str() {
            "openai" => ProviderKind::OpenAi,
            "gemini" => ProviderKind::Gemini,
            "anthropic" => ProviderKind::Anthropic,
            _ => return Err(format!("ReactorNode: Invalid provider '{}'.", provider_str).into()),
        };

        let api_key_raw = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or("ReactorNode: Missing 'api_key' in config")?;
        let api_key = Self::resolve_env_var(api_key_raw)?;

        let model = config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let verbose = config
            .get("verbose")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // --- 2. Collect input texts ---
        // All keys that start with "texts." are treated as context for the Reactor.
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

        // Skip if there's no substantive content to review.
        // `user_request` alone is not enough — we need a `synthesis_result` (or similar).
        let has_synthesis = inputs.iter().any(|(k, v)| {
            k != "system_message"
                && k != "user_request"
                && !matches!(v, Value::Null)
                && v.as_str().map(|s| !s.is_empty()).unwrap_or(true)
        });

        if !has_synthesis {
            colmena_log!("⏩ [ReactorNode] Skipped — no synthesis to review yet.");
            return Ok(Value::Null);
        }

        if formatted_texts.is_empty() {
            colmena_log!("⚠️ [ReactorNode] Skipped — no input texts provided.");
            return Ok(Value::Null);
        }

        // --- 3. Compose system message ---
        let extra_system_msg = inputs
            .get("system_message")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("system_message").and_then(|v| v.as_str()))
            .unwrap_or("");

        let extra_section = if !extra_system_msg.is_empty() {
            format!("\n\nAdditional instructions:\n{}", extra_system_msg)
        } else {
            String::new()
        };

        let schema = reactor_schema();

        let system_message = format!(
            "{}{}\n\nYou MUST output JSON matching this schema:\n{}",
            DEFAULT_REACTOR_SYSTEM_MSG,
            extra_section,
            serde_json::to_string_pretty(&schema)?
        );

        if verbose {
            colmena_log!("\n═══════════════════════════════════════");
            colmena_log!("⚡ [ReactorNode] VERBOSE — System Prompt:");
            colmena_log!("───────────────────────────────────────");
            colmena_log!("{}", system_message);
            colmena_log!("───────────────────────────────────────");
            colmena_log!("Context Texts:\n{}", formatted_texts);
            colmena_log!("═══════════════════════════════════════\n");
        } else {
            colmena_log!("⚡ [ReactorNode] Reviewing synthesis and producing final response...");
        }

        // --- 4. Call LLM ---
        let provider = LlmProvider::new(provider_kind.clone(), api_key, model)?;
        let mut llm_config = LlmConfig::new(provider);
        llm_config = llm_config.with_temperature(0.2)?;

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

        let params = crate::llm::application::AgentRunParams {
            session_id: &tid,
            prompt: String::new(),
            messages: Some(messages),
            config: llm_config,
            tools: vec![],
            tool_executor: &EmptyToolExecutor,
            max_iterations: Some(1),
            on_token: None,
        };

        let response = agent_service.run(params).await?;

        // Notify observer of usage
        if let Some(obs) = _observer {
            if let Some(usage) = response.usage() {
                use crate::dag_engine::domain::observer::NodeEvent;
                obs.on_event(NodeEvent::LlmUsage {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                });
            }
        }

        let raw = response.content();

        if verbose {
            colmena_log!("\n═══════════════════════════════════════");
            colmena_log!("⚡ [ReactorNode] VERBOSE — Raw LLM Response:");
            colmena_log!("───────────────────────────────────────");
            colmena_log!("{}", raw);
            colmena_log!("═══════════════════════════════════════\n");
        }

        // --- 5. Parse and return ---
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
        // Some LLMs escape '$' as '\$' inside JSON strings, which is invalid JSON.
        // Replace all occurrences to avoid parse failures.
        let clean_owned = clean.replace("\\$", "$");
        let clean = clean_owned.as_str();

        let parsed: Value = serde_json::from_str(clean)
            .map_err(|e| format!("ReactorNode: Failed to parse LLM JSON: {}. Raw: {}", e, raw))?;

        let task_ok = parsed
            .get("task_ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let response_text = parsed.get("response").cloned().unwrap_or(Value::Null);
        let add_tasks = parsed.get("add_tasks").cloned().unwrap_or(json!([]));
        let suspend = parsed
            .get("suspend")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let question = parsed.get("question").cloned().unwrap_or(Value::Null);

        colmena_log!(
            "⚡ [ReactorNode] Decision → task_ok={}, new_tasks={}, suspend={}",
            task_ok,
            add_tasks.as_array().map(|a| a.len()).unwrap_or(0),
            suspend
        );

        // Note: phase summary persistence is handled by the OrchestratorNode when this reactor
        // is invoked as an internal phase_reactor. When invoked standalone from a DAG graph,
        // summaries are not automatically saved (use a dedicated save node if needed).

        Ok(json!({
            "result":  response_text,
            "extra_info": {
                "task_ok":   task_ok,
                "add_tasks": add_tasks,
                "suspend":   suspend,
                "question":  question,
                "__colmena_status": if suspend { "SUSPENDED" } else { "OK" }
            }
        }))
    }

    fn description(&self) -> Option<&str> {
        Some("Final reviewer that evaluates a synthesized response and either produces the user-facing answer or requests corrections/more info.")
    }

    fn default_output(&self) -> Option<&str> {
        Some("result")
    }

    fn schema(&self) -> Value {
        reactor_schema()
    }
}
