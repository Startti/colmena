use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::tool_configuration::ToolConfiguration;
use crate::llm::domain::{
    LlmConfig, LlmMessage, LlmProvider, LlmStreamPart, ProviderKind, ThreadId, ToolExecutor,
};
use crate::llm::infrastructure::{ConversationRepositoryFactory, LlmProviderFactory};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
use crate::llm::application::AgentService;
use std::sync::Weak;

pub struct LlmNode {
    repository_factory: Arc<ConversationRepositoryFactory>,
    registry: Weak<dyn NodeRegistryPort>,
}

impl LlmNode {
    pub fn new(
        repository_factory: Arc<ConversationRepositoryFactory>,
        registry: Weak<dyn NodeRegistryPort>,
    ) -> Self {
        Self {
            repository_factory,
            registry,
        }
    }

    fn resolve_env_var(value: &str) -> Result<String, String> {
        if value.starts_with("${") && value.ends_with("}") {
            let var_name = &value[2..value.len() - 1];
            std::env::var(var_name)
                .map_err(|_| format!("Environment variable {} not found", var_name))
        } else {
            Ok(value.to_string())
        }
    }

    fn resolve_context_vars(value: &str, inputs: &NodeInputs) -> String {
        let mut result = String::new();
        let mut last_end = 0;

        while let Some(start) = value[last_end..].find("${context.") {
            let absolute_start = last_end + start;
            result.push_str(&value[last_end..absolute_start]);

            if let Some(end) = value[absolute_start..].find('}') {
                let absolute_end = absolute_start + end;
                let var_path = &value[absolute_start + 2..absolute_end]; // e.g. "context.amadeus_token"

                // Look up in inputs
                // inputs keys are flattened, e.g. "context.amadeus_token"
                let val = if let Some(v) = inputs.get(var_path) {
                    match v {
                        Value::String(s) => s.clone(),
                        _ => v.to_string(),
                    }
                } else {
                    // Keep original if not found
                    value[absolute_start..=absolute_end].to_string()
                };

                result.push_str(&val);
                last_end = absolute_end + 1;
            } else {
                result.push_str(&value[absolute_start..]);
                last_end = value.len();
                break;
            }
        }
        result.push_str(&value[last_end..]);
        result
    }
}

#[async_trait]
impl ExecutableNode for LlmNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        // --- 1. Resolve Configuration (Inputs > Config) ---

        // Provider
        let provider_str = inputs
            .get("provider")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("provider").and_then(|v| v.as_str()))
            .ok_or("Missing 'provider' in inputs or config")?;

        let provider_kind = match provider_str.to_lowercase().as_str() {
            "openai" => ProviderKind::OpenAi,
            "gemini" => ProviderKind::Gemini,
            "anthropic" => ProviderKind::Anthropic,
            "mock" => ProviderKind::Mock,
            _ => {
                return Err(format!(
                    "Invalid provider '{}'. Supported: openai, gemini, anthropic, mock",
                    provider_str
                )
                .into())
            }
        };

        // API Key
        let api_key_raw = inputs
            .get("api_key")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("api_key").and_then(|v| v.as_str()))
            .ok_or("Missing 'api_key' in inputs or config")?;

        let api_key = Self::resolve_env_var(api_key_raw)?;

        // Model
        let model = inputs
            .get("model")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("model").and_then(|v| v.as_str()))
            .map(|s| s.to_string());

        // Prompt
        let prompt = inputs
            .get("prompt")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("prompt").and_then(|v| v.as_str()))
            .ok_or("Missing 'prompt' in inputs or config")?;

        // System Message (Optional)
        let system_message = inputs
            .get("system_message")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("system_message").and_then(|v| v.as_str()));

        // Thread ID (Optional - for Memory)
        let thread_id = inputs
            .get("thread_id")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("thread_id").and_then(|v| v.as_str()));

        // Connection URL (Optional - for Memory Backend)
        let connection_url_raw = inputs
            .get("connection_url")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("connection_url").and_then(|v| v.as_str()));

        // --- 2. Prepare LLM Request ---

        let provider = LlmProvider::new(provider_kind.clone(), api_key, model)?;
        let mut llm_config = LlmConfig::new(provider); // Add extra config params here if needed

        // Optional Params
        if let Some(temp) = inputs
            .get("temperature")
            .and_then(|v| v.as_f64())
            .or_else(|| config.get("temperature").and_then(|v| v.as_f64()))
        {
            llm_config = llm_config.with_temperature(temp as f32)?;
        }

        if let Some(max_tokens) = inputs
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .or_else(|| config.get("max_tokens").and_then(|v| v.as_u64()))
        {
            llm_config = llm_config.with_max_tokens(max_tokens as u32)?;
        }

        let mut messages = Vec::new();

        // 2.1 Load History if Thread ID and Connection URL are present
        let mut repo_instance = None;
        if let (Some(tid), Some(url_raw)) = (thread_id, connection_url_raw) {
            let connection_url = Self::resolve_env_var(url_raw)?;
            let repo = self
                .repository_factory
                .get_repository(&connection_url)
                .await?;
            repo_instance = Some(repo.clone());

            let tid = ThreadId(tid.to_string());
            let conversation = repo.get_by_id(&tid).await?;
            messages.extend(conversation.messages);
        }

        // 2.2 Add System Message if present (and not already in history? For now just add it if provided)
        // Note: Usually system message is first. If history exists, maybe we shouldn't add it again?
        // Or maybe the history loading should handle this. For now, let's prepend if messages is empty.
        if let Some(sys_msg) = system_message {
            if messages.is_empty() {
                messages.push(LlmMessage::system(sys_msg.to_string())?);
            }
        }

        // 2.3 Add User Prompt
        let user_message = LlmMessage::user(prompt.to_string())?;
        messages.push(user_message.clone());

        // --- 3. Execute LLM Call (via AgentService) ---
        let llm_repo = LlmProviderFactory::create(provider_kind);
        let llm_repo_arc: Arc<dyn crate::llm::domain::LlmRepository> = llm_repo; // Already Arc

        // Create Tool Executor
        // We need to resolve the registry from Weak reference
        let registry = self
            .registry
            .upgrade()
            .ok_or("NodeRegistry has been dropped")?;

        // Parse tool_configurations
        let mut tool_configurations: HashMap<String, ToolConfiguration> = inputs
            .get("tool_configurations")
            .or_else(|| config.get("tool_configurations"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        // Resolve context variables in fixed_config
        for config in tool_configurations.values_mut() {
            for val in config.fixed_config.values_mut() {
                if let Value::String(s) = val {
                    *val = Value::String(Self::resolve_context_vars(s, inputs));
                }
            }
        }

        let tool_executor = DagToolExecutor::new(registry, tool_configurations);

        // Create AgentService
        // Note: AgentService expects Arc<dyn ConversationRepository>.
        // We have repo_instance which is Arc<dyn ConversationRepository> (if memory enabled).
        // If memory is NOT enabled, we need a dummy/mock repository or handle it.
        // AgentService *requires* a repository to store history.
        // If the user didn't provide thread_id, we can't persist history.
        // However, AgentService logic depends on it.
        // For now, if no memory is configured, we can use an in-memory repository or fail?
        // Or we can create a temporary in-memory repository for this execution?
        // Let's assume for now we use a temporary in-memory repo if no thread_id provided,
        // but wait, AgentService assumes persistence.
        // If we don't provide a repo, AgentService can't work.
        // Actually, AgentService is designed for stateful agents.
        // If LlmNode is used without memory, it's just a simple call.
        // But we want to support tools even without persistent memory (single turn).
        // So we should provide an ephemeral repository.
        // Let's implement a simple EphemeralConversationRepository or use Mock?
        // Better: Use Sqlite with :memory:? Or just a simple struct.
        // For now, let's require thread_id if tools are used? No, that's restrictive.

        // Let's use a temporary SQLite in-memory repo if none provided.
        // But creating a pool is expensive.
        // Maybe we can use a "NoOp" repository that stores nothing?
        // But AgentService reads history.
        // If we use a "Memory" repository (HashMap based), it works for the duration of the request.
        // We don't have a MemoryRepository in domain.

        // Let's use the repo_instance if available. If not, we create a temporary one?
        // Or we modify AgentService to make repo optional? No.

        // Let's assume for this phase that we use the provided repo or fail if tools are needed but no repo?
        // But AgentService is the *only* way we call LLM now (according to plan).
        // So we need a repo.

        let conversation_repo: Arc<dyn crate::llm::domain::ConversationRepository> =
            match repo_instance {
                Some(repo) => repo,
                None => {
                    // Fallback to a lightweight in-memory repository
                    // This allows stateless LLM calls without requiring database connections
                    use crate::llm::infrastructure::persistence::in_memory_conversation_repository::InMemoryConversationRepository;
                    Arc::new(InMemoryConversationRepository::new())
                }
            };

        let agent_service = AgentService::new(llm_repo_arc, conversation_repo);

        // Define tools based on enabled_tools config
        // enabled_tools can be:
        // - Array of specific tool names: ["add", "multiply"]
        // - "*" (wildcard for all tools)
        // - Not specified (no tools)
        let enabled_tools_config = inputs
            .get("enabled_tools")
            .or_else(|| config.get("enabled_tools"));

        let tools = if let Some(enabled) = enabled_tools_config {
            // Get all available tools from the executor
            let all_tools = tool_executor.available_tools().await;
            if let Some(wildcard) = enabled.as_str() {
                if wildcard == "*" {
                    // Enable all tools
                    all_tools
                } else {
                    // Single tool name as string
                    all_tools
                        .into_iter()
                        .filter(|t| t.name == wildcard)
                        .collect()
                }
            } else if let Some(tool_names) = enabled.as_array() {
                // Array of tool names
                let names: Vec<String> = tool_names
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();

                all_tools
                    .into_iter()
                    .filter(|t| names.contains(&t.name))
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            // No tools enabled
            Vec::new()
        };

        // Use provided thread_id or generate unique one for stateless calls
        let tid = thread_id
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // Check if streaming is enabled
        let stream_enabled = inputs
            .get("stream")
            .and_then(|v| v.as_bool())
            .or_else(|| config.get("stream").and_then(|v| v.as_bool()))
            .unwrap_or(false);

        // Define on_token callback if streaming is enabled and observer is present
        // Define on_token callback if streaming is enabled and observer is present
        let on_token: Option<Box<dyn Fn(LlmStreamPart) + Send + Sync>> = if stream_enabled {
            if let Some(obs) = _observer.clone() {
                Some(Box::new(move |part: LlmStreamPart| {
                    use crate::dag_engine::domain::observer::NodeEvent;
                    match part {
                        LlmStreamPart::Content(token) => {
                            obs.on_event(NodeEvent::LlmToken { token })
                        }
                        LlmStreamPart::ToolCallChunk(chunk) => {
                            obs.on_event(NodeEvent::LlmToolCall {
                                tool_id: chunk.id,
                                tool_name: chunk.name,
                                args_chunk: chunk.args_chunk,
                            })
                        }
                        LlmStreamPart::Usage(usage) => obs.on_event(NodeEvent::LlmUsage {
                            prompt_tokens: usage.prompt_tokens,
                            completion_tokens: usage.completion_tokens,
                        }),
                        LlmStreamPart::ToolCallStart(tc) => {
                            obs.on_event(NodeEvent::LlmToolCallStart {
                                tool_id: tc.id.clone(),
                                tool_name: tc.function.name.clone(),
                                tool_args: tc.function.arguments.clone(),
                            })
                        }
                        LlmStreamPart::ToolCallFinish(res) => {
                            obs.on_event(NodeEvent::LlmToolCallFinish {
                                tool_id: res.tool_call_id.clone(),
                                success: res.success,
                                output: res.output.clone(),
                            })
                        }
                    }
                }))
            } else {
                None
            }
        } else {
            None
        };

        // Create AgentService parameters
        let params = crate::llm::application::AgentRunParams {
            thread_id: &ThreadId(tid),
            prompt: prompt.to_string(),
            config: llm_config,
            tools,
            tool_executor: &tool_executor,
            max_iterations: Some(10), // Max iterations
            on_token,
        };

        let response = agent_service.run(params).await?;

        // Output format
        Ok(json!({
            "output": {
                "content": response.content(),
                "usage": response.usage(),
                "tool_calls": response.tool_calls()
            }
        }))
    }

    fn description(&self) -> Option<&str> {
        Some("Call language models with conversation memory and tool calling capabilities. Supports OpenAI, Gemini, and Anthropic.")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "llm_call",
            "config": {
                "provider": "string (openai, gemini, anthropic)",
                "api_key": "string",
                "model": "string (optional)",
                "system_message": "string (optional)",
                "prompt": "string (optional)",
                "temperature": "number (optional)",
                "max_tokens": "integer (optional)",
                "thread_id": "string (optional, enables memory)",
                "connection_url": "string (optional, database connection for memory)",
                "enabled_tools": "array of strings or '*' (optional, enables tool calling)",
                "tool_configurations": "map<string, ToolConfiguration> (optional, partial config for tools)"
            },
            "inputs": {
                "provider": "string (optional)",
                "api_key": "string (optional)",
                "model": "string (optional)",
                "system_message": "string (optional)",
                "prompt": "string (optional)",
                "temperature": "number (optional)",
                "max_tokens": "integer (optional)",
                "thread_id": "string (optional, enables memory)",
                "connection_url": "string (optional)",
                "enabled_tools": "array of strings or '*' (optional)"
            },
            "outputs": {
                "content": "string",
                "usage": "object",
                "tool_calls": "array (optional)"
            }
        })
    }
}
