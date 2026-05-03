use crate::colmena_log;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

use crate::llm::application::AgentService;
use crate::llm::domain::{
    ConversationKey, LlmConfig, LlmMessage, LlmProvider, LlmStreamPart, NodeIdPath, ProviderKind,
    SessionId,
};
use crate::llm::infrastructure::persistence::in_memory_conversation_repository::InMemoryConversationRepository;
use crate::llm::infrastructure::LlmProviderFactory;

/// The internal, fixed schema that every PlannerNode uses.
/// It produces an array of tasks, each with a name, assignee, and completed flag.
fn default_planner_schema() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "A clear, self-contained description of the task to accomplish."
                },
                "assigned_to": {
                    "type": "string",
                    "description": "The name of the agent (node) that should execute this task."
                },
                "completed": {
                    "type": "boolean",
                    "description": "Always set to false for new tasks."
                },
                "phase": {
                    "type": "integer",
                    "description": "Execution phase (starting at 1). Tasks in lower phases run first. Assign the same phase to tasks that can run simultaneously."
                },
                "parallel": {
                    "type": "boolean",
                    "description": "Set to true if this task can run at the same time as other tasks in the same phase. Set to false if it must run alone sequentially."
                }
            },
            "required": ["task", "assigned_to", "completed", "phase", "parallel"]
        }
    })
}

/// The default built-in system message included in every PlannerNode call.
const DEFAULT_PLANNER_SYSTEM_MSG: &str = include_str!("prompts/planner_system.md");

pub struct PlannerNode {
    task_memory_repo: Option<Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>>,
}

impl PlannerNode {
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
                .map_err(|_| format!("Environment variable '{}' not found", var_name))
        } else {
            Ok(value.to_string())
        }
    }
}

#[async_trait]
impl ExecutableNode for PlannerNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        // --- 0. Skip if tasks already exist in Postgres for this session_id ---
        // This prevents wasted LLM calls on Turn 2+: once the plan is loaded
        // into the DB the Orchestrator handles routing, so we short-circuit here.
        let session_id = state
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !session_id.is_empty() {
            if let Some(repo) = &self.task_memory_repo {
                let existing = repo.get_tasks_for_run(&session_id).await?;
                if !existing.is_empty() {
                    colmena_log!("⏭️  [PlannerNode] Plan already exists in DB ({} tasks) — skipping LLM call.", existing.len());
                    return Ok(Value::Null);
                }
            }
        }

        // --- 1. Resolve Provider Configuration ---
        let provider_str = config
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or("PlannerNode: Missing 'provider' in config")?;

        let provider_kind = match provider_str.to_lowercase().as_str() {
            "openai" => ProviderKind::OpenAi,
            "gemini" => ProviderKind::Gemini,
            "anthropic" => ProviderKind::Anthropic,
            _ => return Err(format!("PlannerNode: Invalid provider '{}'.", provider_str).into()),
        };

        let api_key_raw = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or("PlannerNode: Missing 'api_key' in config")?;
        let api_key = Self::resolve_env_var(api_key_raw)?;

        let model = config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Optional verbose flag for debugging: prints prompt sent and raw response received.
        let verbose = config
            .get("verbose")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // --- 2. Read Available Agents ---
        // Each entry can be:
        //   { "name": "my_agent", "description": "Handles X" }   ← preferred: rich metadata
        //   "my_agent"                                            ← still accepted: bare string
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

        if agents.is_empty() {
            colmena_log!("⚠️ [PlannerNode] No 'agents' field found in config. The LLM will assign tasks freely without agent constraints.");
        }

        // --- 3. Compose System Message ---
        let extra_system_msg = inputs
            .get("system_message")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("system_message").and_then(|v| v.as_str()))
            .unwrap_or("");

        // Build rich agent catalogue for the prompt
        let agents_section = if !agents.is_empty() {
            let agent_lines = agents
                .iter()
                .map(|(name, desc)| format!("  - \"{}\": {}", name, desc))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "\n\nAvailable specialist agents:\n{}\n\n\
                 The 'assigned_to' field MUST be EXACTLY one of the agent names shown above. \
                 Choose the agent whose description best matches the task. \
                 Do NOT invent new agent names.",
                agent_lines
            )
        } else {
            String::new()
        };

        // Build schema with enum constrained to actual agent names (names only — no descriptions)
        let schema = if !agents.is_empty() {
            let agent_values: Vec<Value> = agents
                .iter()
                .map(|(name, _)| Value::String(name.clone()))
                .collect();
            json!({
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "task": {
                            "type": "string",
                            "description": "A clear, self-contained description of the task to accomplish."
                        },
                        "assigned_to": {
                            "type": "string",
                            "enum": agent_values,
                            "description": "The agent node ID responsible for this task."
                        },
                        "completed": {
                            "type": "boolean",
                            "description": "Always false for new tasks."
                        },
                        "phase": {
                            "type": "integer",
                            "description": "Execution phase (starting at 1). Tasks in lower phases run first. Assign the same phase to tasks that can run simultaneously."
                        },
                        "parallel": {
                            "type": "boolean",
                            "description": "Set to true if this task can run at the same time as other tasks in the same phase. Set to false if it must run alone sequentially."
                        }
                    },
                    "required": ["task", "assigned_to", "completed", "phase", "parallel"]
                }
            })
        } else {
            default_planner_schema()
        };

        let extra_instructions_section = if !extra_system_msg.is_empty() {
            format!("\n\nAdditional instructions:\n{}", extra_system_msg)
        } else {
            String::new()
        };

        let system_message = format!(
            "{}{}{}\n\nYou MUST output JSON matching this schema:\n{}",
            DEFAULT_PLANNER_SYSTEM_MSG,
            agents_section,
            extra_instructions_section,
            serde_json::to_string_pretty(&schema)?
        );

        // --- 3. Gather and Format Texts (user request) ---
        let mut formatted_texts = String::new();

        for (key, val) in inputs {
            if key == "system_message" {
                continue; // Already used above
            }
            let text_key = key.strip_prefix("texts.").unwrap_or(key.as_str());
            let text_str = match val {
                Value::String(s) => s.clone(),
                Value::Null => continue,
                _ => val.to_string(),
            };
            let clean_text = if text_str.starts_with('"') && text_str.ends_with('"') {
                text_str[1..text_str.len() - 1].to_string()
            } else {
                text_str
            };
            formatted_texts.push_str(&format!("# {}\n\n{}\n\n", text_key, clean_text));
        }

        if let Some(texts_obj) = config.get("texts").and_then(|v| v.as_object()) {
            for (key, val) in texts_obj {
                if let Some(text_str) = val.as_str() {
                    formatted_texts.push_str(&format!("# {}\n\n{}\n\n", key, text_str));
                }
            }
        }

        if formatted_texts.is_empty() {
            colmena_log!("⚠️ [PlannerNode] Skipped execution because no input text was provided.");
            return Ok(Value::Null);
        }

        if verbose {
            colmena_log!("\n═══════════════════════════════════════");
            colmena_log!("🗂️  [PlannerNode] VERBOSE — System Prompt Sent:");
            colmena_log!("───────────────────────────────────────");
            colmena_log!("{}", system_message);
            colmena_log!("───────────────────────────────────────");
            colmena_log!("📥 User Input Texts:");
            colmena_log!("{}", formatted_texts);
            colmena_log!("═══════════════════════════════════════\n");
        } else {
            colmena_log!(
                "🗂️ [PlannerNode] Planning tasks (set verbose=true in config to see full prompt)"
            );
        }

        // --- 4. Call LLM ---
        let provider = LlmProvider::new(provider_kind.clone(), api_key, model)?;
        let mut llm_config = LlmConfig::new(provider);
        llm_config = llm_config.with_temperature(0.1)?;
        if let Some(budget) = config.get("thinking_budget").and_then(|v| v.as_u64()) {
            llm_config = llm_config.with_thinking_budget(budget as u32);
        }

        let llm_repo = LlmProviderFactory::create(provider_kind);
        let conversation_repo = Arc::new(InMemoryConversationRepository::new());
        let agent_service = AgentService::new(llm_repo, conversation_repo);

        let tid_val = uuid::Uuid::new_v4().to_string();
        let tid = ConversationKey {
            session_id: SessionId(tid_val.clone()),
            agent_session_id: None,
            node_id: NodeIdPath(tid_val),
        };

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
            tools_provider: None,
        };

        let response = agent_service.run(params).await?;

        let raw = response.content();

        if verbose {
            colmena_log!("\n═══════════════════════════════════════");
            colmena_log!("🗂️  [PlannerNode] VERBOSE — Raw LLM Response:");
            colmena_log!("───────────────────────────────────────");
            colmena_log!("{}", raw);
            colmena_log!("═══════════════════════════════════════\n");
        }

        // --- 5. Parse JSON Response ---
        let mut clean = raw.trim();
        if clean.starts_with("```json") {
            clean = clean.trim_start_matches("```json");
        } else if clean.starts_with("```") {
            clean = clean.trim_start_matches("```");
        }
        if clean.ends_with("```") {
            clean = clean.trim_end_matches("```");
        }
        clean = clean.trim();

        let parsed: Value = serde_json::from_str(clean).map_err(|e| {
            format!(
                "PlannerNode: Failed to parse LLM output as JSON: {}. Raw: {}",
                e, raw
            )
        })?;

        // --- 5b. Detect suspend request (questions instead of tasks) ---
        // The LLM may return { "questions": [...] } when it needs clarification.
        // Bare arrays are treated as a task list (backward-compatible).
        // Defensive: some models (notably gpt-4o-mini) echo the schema back as
        // the output, producing { "type": "array", "items": [<tasks...>] }.
        // Unwrap that shape into a bare array so downstream treats it as tasks.
        let normalized: Value = if parsed.is_array() {
            json!({ "tasks": parsed })
        } else if parsed
            .get("type")
            .and_then(|v| v.as_str())
            .map(|s| s.eq_ignore_ascii_case("array"))
            .unwrap_or(false)
            && parsed.get("items").map(|v| v.is_array()).unwrap_or(false)
        {
            colmena_log!("⚠️  [PlannerNode] LLM echoed schema wrapper ({{type:array, items:[...]}}); unwrapping items as task list.");
            json!({ "tasks": parsed.get("items").cloned().unwrap_or(json!([])) })
        } else {
            parsed
        };

        if let Some(questions) = normalized.get("questions") {
            if questions.is_array() && !questions.as_array().map(|a| a.is_empty()).unwrap_or(true) {
                colmena_log!("⏸️  [PlannerNode] Planner requested clarification before planning ({} questions).", questions.as_array().map(|a| a.len()).unwrap_or(0));
                return Ok(json!({
                    "__colmena_status": "SUSPENDED",
                    "result": {
                        "questions": questions
                    },
                    "extra_info": {
                        "raw_response": raw
                    }
                }));
            }
        }

        // --- 6. Return tasks ---
        let items = normalized.get("tasks").cloned().unwrap_or(json!([]));

        // Write it to global shared state so the Orchestrator can find it natively
        if let Some(state_obj) = state.as_object_mut() {
            state_obj.insert("plan".to_string(), json!({ "items": items.clone() }));
        }

        // Return gracefully (it will still be emitted as an output event)
        Ok(json!({
            "result": {
                "items": items
            },
            "extra_info": {
                "raw_response": raw
            }
        }))
    }

    fn description(&self) -> Option<&str> {
        Some("Breaks down a user request into a list of structured tasks using an LLM. The schema is built-in; only provider, api_key, model, and optional system_message are required.")
    }

    fn default_output(&self) -> Option<&str> {
        Some("result")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "planner",
            "config": {
                "provider": "string (required) - e.g. 'openai'",
                "api_key": "string (required) - can use ${ENV_VAR}",
                "model": "string (optional) - defaults to provider default",
                "system_message": "string (optional) - extra instructions appended to the default planner prompt"
            },
            "inputs": {
                "request": "the user's natural language request (or any named text input)",
                "system_message": "string (optional) - extra instructions"
            },
            "outputs": {
                "output.items": "array of task objects { task, assigned_to, completed }"
            }
        })
    }
}
