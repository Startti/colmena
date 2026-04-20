use crate::colmena_log;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::tool_configuration::ToolConfiguration;
use crate::llm::domain::{
    LlmConfig, LlmMessage, LlmProvider, LlmStreamPart, ProviderKind, SessionId, ToolExecutor,
};
use crate::llm::infrastructure::{ConversationRepositoryFactory, LlmProviderFactory};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::build_load_skill_tool_definition;
use crate::llm::application::AgentService;
use crate::skills::domain::{SkillRepository, SkillsConfig};
use crate::skills::infrastructure::{
    BuiltinSkillRepository, CompositeSkillRepository, FilesystemSkillRepository,
};
use std::path::PathBuf;
use std::sync::Weak;

#[derive(Debug, Clone)]
struct SkillLoadedLogEntry {
    skill_name: String,
    reference: Option<String>,
    source: String,
}

pub struct LlmNode {
    repository_factory: Arc<ConversationRepositoryFactory>,
    registry: Weak<dyn NodeRegistryPort>,
    task_memory_repo: Option<Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>>,
    /// Optional SecureValueService — propagated to DagToolExecutor during tool calls.
    secure_value_service:
        Option<Arc<crate::dag_engine::application::secure_value_service::SecureValueService>>,
}

impl LlmNode {
    pub fn new(
        repository_factory: Arc<ConversationRepositoryFactory>,
        registry: Weak<dyn NodeRegistryPort>,
        task_memory_repo: Option<
            Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>,
        >,
    ) -> Self {
        Self {
            repository_factory,
            registry,
            task_memory_repo,
            secure_value_service: None,
        }
    }

    /// Builder: attach a SecureValueService so it is forwarded to DagToolExecutor during tool calls.
    pub fn with_secure_values(
        mut self,
        secure_value_service: Arc<
            crate::dag_engine::application::secure_value_service::SecureValueService,
        >,
    ) -> Self {
        self.secure_value_service = Some(secure_value_service);
        self
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

    /// Parse `COLMENA_SKILLS_ALLOWED_DIRS` env var into a list of PathBufs.
    /// Separator: `:` on Unix, `;` on Windows. Missing env var → empty list.
    fn parse_allowed_dirs_env() -> Vec<PathBuf> {
        let raw = match std::env::var("COLMENA_SKILLS_ALLOWED_DIRS") {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let separator = if cfg!(windows) { ';' } else { ':' };
        raw.split(separator)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect()
    }

    /// Build a SkillRepository from the parsed config. Returns `None` if no skills are configured.
    /// Returns `Err(String)` on any validation failure — this must abort graph execution.
    fn build_skill_repository_from_config(
        config: &Value,
        inputs: &NodeInputs,
    ) -> Result<Option<Arc<dyn SkillRepository>>, String> {
        let raw_val = inputs
            .get("skills")
            .or_else(|| config.get("skills"));
        let raw_val = match raw_val {
            Some(v) => v,
            None => return Ok(None),
        };

        let skills_config = SkillsConfig::from_value(raw_val)
            .map_err(|e| format!("invalid 'skills' config: {}", e))?;
        if !skills_config.has_any() {
            return Ok(None);
        }

        // Determine graph directory.
        // Prefer __colmena_graph_path from inputs (injected upstream by the runner);
        // fall back to current working directory.
        let graph_dir: PathBuf = inputs
            .get("__colmena_graph_path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .and_then(|p| p.parent().map(|pp| pp.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let allowed = Self::parse_allowed_dirs_env();

        let builtin: Arc<dyn SkillRepository> = Arc::new(
            BuiltinSkillRepository::new(&skills_config.builtin)
                .map_err(|e| format!("loading builtin skills: {}", e))?,
        );
        let filesystem: Arc<dyn SkillRepository> = Arc::new(
            FilesystemSkillRepository::from_paths(&skills_config.paths, &graph_dir, &allowed)
                .map_err(|e| format!("loading filesystem skills: {}", e))?,
        );
        let composite = CompositeSkillRepository::new(builtin, filesystem)
            .map_err(|e| format!("composing skill repositories: {}", e))?;
        Ok(Some(Arc::new(composite)))
    }

    /// Resolve all ${var} placeholders (context, trigger, node outputs, etc.)
    /// Matches ${anything.with.dots} and looks it up in inputs
    fn resolve_context_vars(value: &str, inputs: &NodeInputs) -> String {
        let mut result = String::new();
        let mut last_end = 0;

        // Match any ${...} pattern, not just ${context.*}
        while let Some(start) = value[last_end..].find("${") {
            let absolute_start = last_end + start;
            result.push_str(&value[last_end..absolute_start]);

            if let Some(end) = value[absolute_start..].find('}') {
                let absolute_end = absolute_start + end;
                let var_path = &value[absolute_start + 2..absolute_end]; // e.g. "context.amadeus_token", "trigger.data", etc.

                // Look up in inputs with the full path
                // inputs keys are flattened, e.g. "context.amadeus_token", "trigger.prompt", etc.
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

    /// Recursively resolve ${context.var} placeholders in a NodeSchema structure
    fn resolve_context_in_node_schema(
        schema: &mut crate::dag_engine::domain::tool_configuration::NodeSchema,
        inputs: &NodeInputs,
    ) {
        for field in schema.values_mut() {
            // Resolve fixed value if it's a string
            if let Some(Value::String(s)) = field.fixed.as_mut() {
                *s = Self::resolve_context_vars(s, inputs);
            }

            // Recursively resolve in nested properties
            if let Some(properties) = field.properties.as_mut() {
                for nested_field in properties.values_mut() {
                    if let Some(Value::String(s)) = nested_field.fixed.as_mut() {
                        *s = Self::resolve_context_vars(s, inputs);
                    }
                }
            }
        }
    }

    fn resolve_template_vars(value: &str, inputs: &NodeInputs) -> String {
        let mut result = String::new();
        let mut last_end = 0;

        while let Some(start) = value[last_end..].find("{{") {
            let absolute_start = last_end + start;
            result.push_str(&value[last_end..absolute_start]);

            if let Some(end) = value[absolute_start..].find("}}") {
                let absolute_end = absolute_start + end + 1; // points to the last }
                let var_path = value[absolute_start + 2..absolute_end - 1].trim();

                let parts: Vec<&str> = var_path.splitn(2, '.').collect();
                let val_str = if parts.is_empty() || parts[0].is_empty() {
                    String::new()
                } else {
                    let root_key = parts[0];
                    if let Some(root_val) = inputs.get(root_key) {
                        if parts.len() == 1 {
                            match root_val {
                                Value::String(s) => s.clone(),
                                _ => serde_json::to_string(root_val).unwrap_or_default(),
                            }
                        } else {
                            let json_pointer = format!("/{}", parts[1].replace('.', "/"));
                            if let Some(nested_val) = root_val.pointer(&json_pointer) {
                                match nested_val {
                                    Value::String(s) => s.clone(),
                                    _ => serde_json::to_string(nested_val).unwrap_or_default(),
                                }
                            } else {
                                String::new()
                            }
                        }
                    } else {
                        String::new()
                    }
                };

                result.push_str(&val_str);
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

        // Prompt — accepts string OR any JSON value (arrays, objects are serialized).
        // This allows the synthesizer to receive `final_result` (a JSON array) directly.
        let prompt_raw_str: String;
        let prompt: &str = {
            let val = inputs
                .get("prompt")
                .or_else(|| config.get("prompt"))
                .or_else(|| inputs.get("task")) // Added fallback to "task"
                .or_else(|| config.get("task")); // Added fallback to "task"
            match val {
                Some(Value::String(s)) => {
                    prompt_raw_str = Self::resolve_template_vars(s, inputs);
                    if prompt_raw_str.is_empty() {
                        let node_name = inputs
                            .get("__node_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("(unknown)");
                        colmena_log!(
                            "⚠️ [LlmNode] Skipped (prompt resolved to empty) — node: \"{}\"",
                            node_name
                        );
                        return Ok(Value::Null);
                    }
                    &prompt_raw_str
                }
                Some(Value::Null) | None => {
                    let node_name = inputs
                        .get("__node_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(unknown)");
                    colmena_log!(
                        "⚠️ [LlmNode] Skipped (not active this turn) — node: \"{}\"",
                        node_name
                    );
                    return Ok(Value::Null);
                }
                Some(other) => {
                    // JSON array / object — serialize to pretty string so the LLM can read it
                    prompt_raw_str =
                        serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string());
                    &prompt_raw_str
                }
            }
        };

        // Optional user_request — if present, prepend it so the LLM has the original question.
        // Useful for the synthesizer pattern:
        //   user_request = original question from trigger
        //   prompt       = final_result (all completed task outputs)
        let combined_prompt_str: String;
        let prompt: &str = {
            let user_req = inputs
                .get("user_request")
                .and_then(|v| v.as_str())
                .or_else(|| config.get("user_request").and_then(|v| v.as_str()));
            if let Some(req) = user_req {
                combined_prompt_str = format!(
                    "User Request:\n{}\n\n---\n\nAgent Results:\n{}",
                    req, prompt
                );
                &combined_prompt_str
            } else {
                prompt
            }
        };

        // Verbose flag for debugging — prints prompt, system message, and raw response.
        let verbose = inputs
            .get("verbose")
            .and_then(|v| v.as_bool())
            .or_else(|| config.get("verbose").and_then(|v| v.as_bool()))
            .unwrap_or(false);

        // System Message (Optional)
        let system_message_str;
        let system_message = if let Some(sys) = inputs
            .get("system_message")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("system_message").and_then(|v| v.as_str()))
        {
            system_message_str = Self::resolve_template_vars(sys, inputs);
            Some(system_message_str.as_str())
        } else {
            None
        };

        // Thread ID (Optional - for Memory)
        // Priority: Global Session > Input Override > Config Sync
        let session_id = inputs
            .get("__colmena_session_id")
            .and_then(|v| v.as_str())
            .or_else(|| inputs.get("session_id").and_then(|v| v.as_str()))
            .or_else(|| config.get("session_id").and_then(|v| v.as_str()));

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
        let mut history_exists = false;

        // 2.1 Load History if Thread ID and Connection URL are present
        let mut repo_instance = None;
        if let (Some(tid), Some(url_raw)) = (session_id, connection_url_raw) {
            let connection_url = Self::resolve_env_var(url_raw)?;
            let repo = self
                .repository_factory
                .get_repository(&connection_url)
                .await?;
            repo_instance = Some(repo.clone());

            let tid = SessionId(tid.to_string());
            let conversation = repo.get_by_id(&tid).await?;
            // We only need to know if history exists to decide on system message
            history_exists = !conversation.messages.is_empty();
        }

        // 2.2 Add User Prompt (system message is pushed after tools are resolved — see below)
        let mut resolved_files = Vec::new();

        // Check if there are any files passed in the node inputs
        if let Some(files_val) = inputs.get("files").or_else(|| config.get("files")) {
            if let Some(files_arr) = files_val.as_array() {
                for file_obj in files_arr {
                    if let Some(obj) = file_obj.as_object() {
                        let mime_type = obj
                            .get("mime_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("application/octet-stream")
                            .to_string();

                        if let Some(data) = obj.get("data").and_then(|v| v.as_str()) {
                            use base64::{engine::general_purpose::STANDARD, Engine as _};

                            // It's a base64 inline string. Remove data URI scheme if present:
                            let base64_data = if data.starts_with("data:") {
                                data.find(',').map(|idx| &data[idx + 1..]).unwrap_or(data)
                            } else {
                                data
                            };

                            let filename = obj
                                .get("filename")
                                .and_then(|v| v.as_str())
                                .unwrap_or("upload.file")
                                .to_string();

                            if let Ok(bytes) = STANDARD.decode(base64_data) {
                                resolved_files.push(crate::llm::domain::FileData {
                                    mime_type,
                                    filename,
                                    bytes,
                                });
                            } else {
                                colmena_log!("WARN: Failed to decode base64 file data");
                            }
                        } else if let Some(path_str) = obj.get("path").and_then(|v| v.as_str()) {
                            let filename = obj
                                .get("filename")
                                .and_then(|v| v.as_str())
                                .unwrap_or_else(|| {
                                    std::path::Path::new(path_str)
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_str()
                                        .unwrap_or("upload.file")
                                })
                                .to_string();

                            // Read from local filesystem
                            if let Ok(bytes) = std::fs::read(path_str) {
                                resolved_files.push(crate::llm::domain::FileData {
                                    mime_type,
                                    filename,
                                    bytes,
                                });
                            } else {
                                colmena_log!("WARN: Failed to read file from path: {}", path_str);
                            }
                        }
                    }
                }
            }
        }

        let user_message = if resolved_files.is_empty() {
            LlmMessage::user(prompt.to_string())?
        } else {
            LlmMessage::user_with_files(prompt.to_string(), resolved_files)?
        };

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

        // Resolve context variables in both fixed_config and node_schema
        for tool_cfg in tool_configurations.values_mut() {
            // Legacy: Resolve context variables in fixed_config (deprecated)
            for val in tool_cfg.fixed_config.values_mut() {
                if let Value::String(s) = val {
                    *val = Value::String(Self::resolve_context_vars(s, inputs));
                }
            }

            // New: Resolve context variables in node_schema fixed values (recursive)
            if let Some(node_schema) = tool_cfg.node_schema.as_mut() {
                Self::resolve_context_in_node_schema(node_schema, inputs);
            }
        }

        // Build skill repository (if configured).
        let skill_repo: Option<Arc<dyn SkillRepository>> =
            Self::build_skill_repository_from_config(config, inputs)?;

        // Track skills loaded across the entire node execution (for summary).
        let skills_used_log: Arc<std::sync::Mutex<Vec<SkillLoadedLogEntry>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let tool_executor = {
            let mut executor = DagToolExecutor::new(registry, tool_configurations);
            // Propagate SecureValueService + session_id so tool calls decrypt secrets.
            if let (Some(svc), Some(sid)) = (self.secure_value_service.clone(), session_id) {
                executor = executor.with_secure_values(svc, sid.to_string());
            }
            if let Some(repo) = skill_repo.clone() {
                executor = executor.with_skills(repo.clone());

                let log_clone = skills_used_log.clone();
                let observer_clone = _observer.clone();
                executor = executor.with_skill_observer(Arc::new(
                    move |result: &crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::LoadSkillDispatchResult| {
                        if let Ok(mut log) = log_clone.lock() {
                            log.push(SkillLoadedLogEntry {
                                skill_name: result.skill_name.clone(),
                                reference: result.reference.clone(),
                                source: match result.source {
                                    crate::skills::domain::SkillSource::Builtin => "builtin".to_string(),
                                    crate::skills::domain::SkillSource::Path => "path".to_string(),
                                },
                            });
                        }
                        if let Some(obs) = &observer_clone {
                            obs.on_event(
                                crate::dag_engine::domain::observer::NodeEvent::SkillLoaded {
                                    tool_id: String::new(),
                                    skill_name: result.skill_name.clone(),
                                    reference: result.reference.clone(),
                                    source: match result.source {
                                        crate::skills::domain::SkillSource::Builtin => "builtin".to_string(),
                                        crate::skills::domain::SkillSource::Path => "path".to_string(),
                                    },
                                    size_bytes: result.size_bytes,
                                },
                            );
                        }
                    },
                ));
            }
            executor
        };

        // Create AgentService
        // Note: AgentService expects Arc<dyn ConversationRepository>.
        // We have repo_instance which is Arc<dyn ConversationRepository> (if memory enabled).
        // If memory is NOT enabled, we need a dummy/mock repository or handle it.
        // AgentService *requires* a repository to store history.
        // If the user didn't provide session_id, we can't persist history.
        // However, AgentService logic depends on it.
        // For now, if no memory is configured, we can use an in-memory repository or fail?
        // Or we can create a temporary in-memory repository for this execution?
        // Let's assume for now we use a temporary in-memory repo if no session_id provided,
        // but wait, AgentService assumes persistence.
        // If we don't provide a repo, AgentService can't work.
        // Actually, AgentService is designed for stateful agents.
        // If LlmNode is used without memory, it's just a simple call.
        // But we want to support tools even without persistent memory (single turn).
        // So we should provide an ephemeral repository.
        // Let's implement a simple EphemeralConversationRepository or use Mock?
        // Better: Use Sqlite with :memory:? Or just a simple struct.
        // For now, let's require session_id if tools are used? No, that's restrictive.

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

        let mut tools = if let Some(enabled) = enabled_tools_config {
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

        if let Some(repo) = skill_repo.as_ref() {
            tools.push(build_load_skill_tool_definition(repo));
        }

        // 2.2 Add System Message if present and history is empty.
        // When tools are enabled, append a pre-baked tool-use instruction block so the user
        // doesn't have to include these instructions manually in every graph.
        if let Some(sys_msg) = system_message {
            if !history_exists {
                let final_system_message = if !tools.is_empty() {
                    let tool_names: Vec<String> =
                        tools.iter().map(|t| format!("- {}", t.name)).collect();
                    format!(
                        "{}\n\n---\n## Tool Use Instructions\nYou have access to the following tools:\n{}\n\nRules:\n- ALWAYS use the available tools to answer questions that require real or live data. Never answer from your own knowledge when a tool can provide the data.\n- Call the most relevant tool before responding. Do not skip tool calls.\n- If a tool call fails, report the error clearly instead of guessing an answer.\n- Only respond without a tool call when the user's request is purely conversational and no tool is needed.",
                        sys_msg,
                        tool_names.join("\n")
                    )
                } else {
                    sys_msg.to_string()
                };
                messages.push(LlmMessage::system(final_system_message)?);
            }
        }

        // Use provided session_id or generate unique one for stateless calls
        let tid = session_id
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // Check if streaming is enabled
        let stream_enabled = inputs
            .get("stream")
            .and_then(|v| v.as_bool())
            .or_else(|| config.get("stream").and_then(|v| v.as_bool()))
            .unwrap_or(true);

        // Define on_token callback if streaming is enabled and observer is present
        let observer_for_stream = _observer.clone();
        let on_token: Option<Box<dyn Fn(LlmStreamPart) + Send + Sync>> =
            if let Some(obs) = observer_for_stream {
                Some(Box::new(move |part: LlmStreamPart| {
                    use crate::dag_engine::domain::observer::NodeEvent;
                    match part {
                        LlmStreamPart::Content(token) if stream_enabled => {
                            obs.on_event(NodeEvent::LlmToken { token })
                        }
                        LlmStreamPart::ToolCallChunk(chunk) if stream_enabled => {
                            obs.on_event(NodeEvent::LlmToolCall {
                                tool_id: chunk.id,
                                tool_name: chunk.name,
                                args_chunk: chunk.args_chunk,
                            })
                        }
                        LlmStreamPart::Usage(usage) if stream_enabled => {
                            obs.on_event(NodeEvent::LlmUsage {
                                prompt_tokens: usage.prompt_tokens,
                                completion_tokens: usage.completion_tokens,
                            })
                        }
                        LlmStreamPart::LlmToolCallStart(tc) => {
                            obs.on_event(NodeEvent::LlmToolCallStart {
                                tool_id: tc.id.clone(),
                                tool_name: tc.function.name.clone(),
                                tool_args: tc.function.arguments.clone(),
                            })
                        }
                        LlmStreamPart::LlmToolCallFinish(res) => {
                            obs.on_event(NodeEvent::LlmToolCallFinish {
                                tool_id: res.tool_call_id.clone(),
                                success: res.success,
                                output: res.output.clone(),
                            });
                        }
                        LlmStreamPart::LlmMessageStart => obs.on_event(NodeEvent::LlmMessageStart),
                        LlmStreamPart::LlmMessageFinish(usage) => {
                            obs.on_event(NodeEvent::LlmMessageFinish(usage));
                        }
                        _ => {}
                    }
                }))
            } else {
                None
            };

        // Create AgentService parameters
        let params = crate::llm::application::AgentRunParams {
            session_id: &SessionId(tid),
            prompt: prompt.to_string(),
            messages: Some(messages.clone()),
            config: llm_config,
            tools,
            tool_executor: &tool_executor,
            max_iterations: Some(10), // Max iterations
            on_token,
        };

        if verbose {
            colmena_log!("\n═══════════════════════════════════════");
            colmena_log!("🤖 [LlmNode] VERBOSE — Request:");
            colmena_log!("───────────────────────────────────────");
            if let Some(sys) = system_message {
                colmena_log!("System: {}", sys);
                colmena_log!("───────────────────────────────────────");
            }
            colmena_log!("Prompt: {}", prompt);
            colmena_log!("═══════════════════════════════════════\n");
        }

        let response = agent_service.run(params).await?;

        // 3.1 Notify observer of usage (even if not streaming)
        if let Some(obs) = _observer.clone() {
            if let Some(usage) = response.usage() {
                use crate::dag_engine::domain::observer::NodeEvent;
                obs.on_event(NodeEvent::LlmUsage {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                });
            }
        }

        if verbose {
            colmena_log!("\n═══════════════════════════════════════");
            colmena_log!("🤖 [LlmNode] VERBOSE — Response:");
            colmena_log!("───────────────────────────────────────");
            colmena_log!("{}", response.content());
            colmena_log!("═══════════════════════════════════════\n");
        }

        // Format result json in standardized structure
        let mut extra_info = json!({
            "usage": response.usage(),
            "tool_calls": response.tool_calls()
        });

        let result_json = json!({
            "result": response.content(),
            "extra_info": extra_info
        });

        // Check if we need to write to memory
        let write_to_memory = inputs
            .get("write_to_memory")
            .and_then(|v| v.as_bool())
            .or_else(|| config.get("write_to_memory").and_then(|v| v.as_bool()))
            .unwrap_or(false);

        let mut output_tasks = Vec::new();

        if write_to_memory {
            if let Some(repo) = &self.task_memory_repo {
                let raw_task_id = inputs
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .or_else(|| config.get("task_id").and_then(|v| v.as_str()));
                if let Some(raw_tid) = raw_task_id {
                    let task_id = Self::resolve_template_vars(raw_tid, inputs);
                    if !task_id.is_empty() {
                        // Store the standardized result structure in the DB
                        repo.update_task_result(&task_id, result_json.clone())
                            .await?;

                        let session_id = _state
                            .get("session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown_run")
                            .to_string();
                        if let Ok(tasks) = repo.get_tasks_for_run(&session_id).await {
                            for t in tasks {
                                output_tasks.push(json!({
                                    "id": t.id,
                                    "task_name": t.task_name,
                                    "assigned_to": t.assigned_to,
                                    "completed": t.completed,
                                    "result": t.result
                                }));
                            }
                        }
                    }
                }
            }
        }

        if write_to_memory && !output_tasks.is_empty() {
            extra_info["all_tasks"] = json!(output_tasks);
        }

        let skills_used_summary: Option<Value> = {
            let log = skills_used_log.lock().ok();
            log.and_then(|entries| {
                if entries.is_empty() {
                    None
                } else {
                    use std::collections::BTreeMap;
                    #[derive(Default)]
                    struct Agg {
                        source: String,
                        references_loaded: Vec<String>,
                        load_count: u32,
                    }
                    let mut agg: BTreeMap<String, Agg> = BTreeMap::new();
                    for e in entries.iter() {
                        let a = agg.entry(e.skill_name.clone()).or_default();
                        a.source = e.source.clone();
                        a.load_count += 1;
                        if let Some(r) = &e.reference {
                            if !a.references_loaded.contains(r) {
                                a.references_loaded.push(r.clone());
                            }
                        }
                    }
                    let arr: Vec<Value> = agg
                        .into_iter()
                        .map(|(name, a)| {
                            json!({
                                "name": name,
                                "source": a.source,
                                "references_loaded": a.references_loaded,
                                "load_count": a.load_count,
                            })
                        })
                        .collect();
                    Some(Value::Array(arr))
                }
            })
        };
        if let Some(skills_used) = skills_used_summary {
            extra_info["skills_used"] = skills_used;
        }

        // Output format
        let final_output = json!({
            "result": response.content(),
            "extra_info": extra_info
        });

        Ok(final_output)
    }

    fn description(&self) -> Option<&str> {
        Some("Call language models with conversation memory and tool calling capabilities. Supports OpenAI, Gemini, and Anthropic.")
    }

    fn default_input(&self) -> Option<&str> {
        Some("prompt")
    }

    fn default_output(&self) -> Option<&str> {
        Some("result")
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
                "session_id": "string (optional, enables memory)",
                "connection_url": "string (optional, database connection for memory)",
                "enabled_tools": "array of strings or '*' (optional, enables tool calling)",
                "tool_configurations": "map<string, ToolConfiguration> (optional, partial config for tools)",
                "write_to_memory": "boolean (optional, if true writes output to db and returns all_tasks)",
                "task_id": "string (optional, required if write_to_memory is true)"
            },
            "inputs": {
                "provider": "string (optional)",
                "api_key": "string (optional)",
                "model": "string (optional)",
                "system_message": "string (optional)",
                "prompt": "string (optional)",
                "temperature": "number (optional)",
                "max_tokens": "integer (optional)",
                "session_id": "string (optional, enables memory)",
                "connection_url": "string (optional)",
                "enabled_tools": "array of strings or '*' (optional)",
                "files": "array of objects [{mime_type, data|path}] (optional)"
            },
            "outputs": {
                "content": "string",
                "usage": "object",
                "tool_calls": "array (optional)"
            }
        })
    }
}
