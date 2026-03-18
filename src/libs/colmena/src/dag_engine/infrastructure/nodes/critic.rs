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
// Fixed schema — always the same for every CriticNode.
// After reviewing the current task's result, the Critic decides:
//   - task_ok       → result is acceptable, move on
//   - add_tasks     → add these new tasks to the queue
//   - suspend       → pause and ask the user a clarifying question
// ---------------------------------------------------------------------------
fn critic_schema(agent_enum: Option<Vec<Value>>) -> Value {
    let assigned_to_schema = if let Some(values) = agent_enum {
        json!({
            "type": "string",
            "enum": values,
            "description": "The agent node ID that should handle this task."
        })
    } else {
        json!({
            "type": "string",
            "description": "The agent node ID that should handle this task."
        })
    };

    json!({
        "type": "object",
        "properties": {
            "task_ok": {
                "type": "boolean",
                "description": "Set to true if the current task result is satisfactory and no further action is needed for it."
            },
            "add_tasks": {
                "type": "array",
                "description": "List of additional tasks to add to the queue if the result is incomplete or more work is needed.",
                "items": {
                    "type": "object",
                    "properties": {
                        "task": {
                            "type": "string",
                            "description": "A clear, self-contained description of the additional task."
                        },
                        "assigned_to": assigned_to_schema
                    },
                    "required": ["task", "assigned_to"]
                }
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
        "required": ["task_ok", "add_tasks", "suspend"]
    })
}

/// Default system prompt baked into every CriticNode.
const DEFAULT_CRITIC_SYSTEM_MSG: &str = "\
You are a critical reviewer in a multi-agent system. Your role is to evaluate \
the result produced by a specialist agent for a specific task and decide whether \
it is complete and satisfactory. \
\n\
Rules:\n\
- If the result fully addresses the task, set 'task_ok' to true and 'add_tasks' to [].\n\
- If the result is incomplete or lacks important details, set 'task_ok' to false \
  and add specific follow-up tasks in 'add_tasks'.\n\
- If you need more information from the user to make a decision, set 'suspend' to \
  true and provide a clear, concise 'question'.\n\
- Be strict but fair. Only flag issues that genuinely affect the quality of the result.\n\
Output ONLY valid JSON matching the schema. Do NOT include markdown or code fences.";

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
        state: &mut Value,
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

        // --- 2. Read available agents (for add_tasks enum constraint) ---
        // Same format as PlannerNode: array of { name, description } or bare strings.
        let agents: Vec<(String, String)> = config
            .get("agents")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        if let Some(obj) = v.as_object() {
                            // Full object: { "name": "...", "description": "..." }
                            let name = obj.get("name").and_then(|n| n.as_str())?.to_string();
                            let desc = obj
                                .get("description")
                                .and_then(|d| d.as_str())
                                .unwrap_or("No description provided.")
                                .to_string();
                            Some((name, desc))
                        } else if let Some(node_id) = v.as_str() {
                            // Bare string: look up description from __graph_nodes in state
                            let desc = state
                                .get("__graph_nodes")
                                .and_then(|g| g.get(node_id))
                                .and_then(|cfg| cfg.get("description"))
                                .and_then(|d| d.as_str())
                                .unwrap_or("No description provided.")
                                .to_string();
                            Some((node_id.to_string(), desc))
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        // --- 3. Collect input texts ---
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
            println!("⚠️ [CriticNode] Skipped — no input texts provided.");
            return Ok(Value::Null);
        }

        // --- 4. Compose system message ---
        let extra_system_msg = inputs
            .get("system_message")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("system_message").and_then(|v| v.as_str()))
            .unwrap_or("");

        // Optional agent catalogue section (so Critic knows which agents it can assign to)
        let agents_section = if !agents.is_empty() {
            let lines = agents
                .iter()
                .map(|(name, desc)| format!("  - \"{}\": {}", name, desc))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "\n\nAvailable specialist agents (use only these names in 'add_tasks.assigned_to'):\n{}",
                lines
            )
        } else {
            String::new()
        };

        let extra_section = if !extra_system_msg.is_empty() {
            format!("\n\nAdditional critic guidelines:\n{}", extra_system_msg)
        } else {
            String::new()
        };

        // Build the schema with optional enum constraint on assigned_to
        let agent_enum = if !agents.is_empty() {
            Some(
                agents
                    .iter()
                    .map(|(name, _)| Value::String(name.clone()))
                    .collect(),
            )
        } else {
            None
        };
        let schema = critic_schema(agent_enum);

        let system_message = format!(
            "{}{}{}\n\nYou MUST output JSON matching this schema:\n{}",
            DEFAULT_CRITIC_SYSTEM_MSG,
            agents_section,
            extra_section,
            serde_json::to_string_pretty(&schema)?
        );

        if verbose {
            println!("\n═══════════════════════════════════════");
            println!("🔎 [CriticNode] VERBOSE — System Prompt:");
            println!("───────────────────────────────────────");
            println!("{}", system_message);
            println!("───────────────────────────────────────");
            println!("Context Texts:\n{}", formatted_texts);
            println!("═══════════════════════════════════════\n");
        } else {
            println!("🔎 [CriticNode] Reviewing task result...");
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
            println!("\n═══════════════════════════════════════");
            println!("🔎 [CriticNode] VERBOSE — Raw LLM Response:");
            println!("───────────────────────────────────────");
            println!("{}", raw);
            println!("═══════════════════════════════════════\n");
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
        let add_tasks = parsed.get("add_tasks").cloned().unwrap_or(json!([]));
        let suspend = parsed
            .get("suspend")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let question = parsed.get("question").cloned().unwrap_or(Value::Null);

        println!(
            "🔎 [CriticNode] Decision → task_ok={}, new_tasks={}, suspend={}",
            task_ok,
            add_tasks.as_array().map(|a| a.len()).unwrap_or(0),
            suspend
        );

        Ok(json!({
            "result": task_ok,
            "extra_info": {
                "add_tasks": add_tasks,
                "suspend":   suspend,
                "question":  question,
                "__colmena_status": if suspend { "SUSPENDED" } else { "OK" }
            }
        }))
    }

    fn description(&self) -> Option<&str> {
        Some("Reviews the result of a specialist agent and decides if the task is complete, needs more work, or requires user input.")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "critic",
            "config": {
                "provider": "openai | gemini | anthropic",
                "api_key": "string or ${ENV_VAR}",
                "model": "optional model name",
                "verbose": "bool (default false)",
                "agents": "optional array of { name, description } or bare strings",
                "system_message": "optional extra instructions concatenated with the default prompt"
            },
            "inputs": {
                "texts.*": "Named text inputs for the Critic to review (e.g. texts.clothing_result, texts.current_task)"
            },
            "outputs": {
                "task_ok":   "bool — true if the result is satisfactory",
                "add_tasks": "array of { task, assigned_to } to queue",
                "suspend":   "bool — true if user input is needed",
                "question":  "string — the question to ask the user when suspend=true",
                "__colmena_status": "SUSPENDED | OK"
            }
        })
    }
}
