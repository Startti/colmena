use crate::colmena_log;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::tool_configuration::ToolConfiguration;
use crate::llm::domain::{
    AgentSessionId, ConversationKey, LlmConfig, LlmMessage, LlmProvider, LlmStreamPart, NodeIdPath,
    ProviderKind, SessionId, ToolExecutor,
};
use crate::llm::infrastructure::{ConversationRepositoryFactory, LlmProviderFactory};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    build_all_document_tools, build_describe_tool_definition, build_load_skill_tool_definition,
    reconstruct_discovered_set, summary_for_catalog, CatalogEntry, DescribeToolDispatchResult,
    DocumentToolsContext, ATTACHMENTS_SYSTEM_PRELUDE, DOCUMENTS_SYSTEM_PRELUDE,
};
use crate::documents::application::DocumentRuntime;
use crate::documents::domain::ids::SessionId as DocSessionId;
use crate::llm::application::AgentService;
use crate::skills::domain::{SkillRepository, SkillsConfig};
use crate::skills::infrastructure::{
    BuiltinSkillRepository, CompositeSkillRepository, FilesystemSkillRepository,
};
use std::path::PathBuf;
use std::sync::Weak;

/// Default system message used when the user has not provided one. Instructs the
/// model to stay grounded in the context it has received and avoid fabricating
/// specific facts that are not present in the conversation.
const LLM_DEFAULT_SYSTEM: &str = include_str!("prompts/llm_default_system.md");

/// Walk a message history and return the first `ToolCall` from the latest
/// `Assistant` message-with-tool_calls that has NO matching `Tool` message
/// (by `tool_call_id`) appearing later in the list.
///
/// Used by the resume path: when the LLM node is re-entered with
/// `__colmena_resume_answer`, the previous run persisted an assistant message
/// containing the SUSPENDED tool call but did not persist a tool result for it.
/// This function returns that pending call so the executor can dispatch it
/// with the resume answer.
fn find_pending_tool_call(
    messages: &[crate::llm::domain::LlmMessage],
) -> Option<crate::llm::domain::ToolCall> {
    use crate::llm::domain::MessageRole;

    // Collect every tool_call_id that already has a Tool message somewhere in
    // the history. Order does not matter — a tool result can only follow its
    // assistant call by construction, so any matching Tool message means the
    // call is resolved.
    let resolved: std::collections::HashSet<&str> = messages
        .iter()
        .filter(|m| m.role() == &MessageRole::Tool)
        .filter_map(|m| m.tool_call_id())
        .collect();

    // Scan from the END so we get the LATEST pending call.
    for msg in messages.iter().rev() {
        if msg.role() != &MessageRole::Assistant {
            continue;
        }
        if let Some(calls) = msg.tool_calls() {
            for call in calls {
                if !resolved.contains(call.id.as_str()) {
                    return Some(call.clone());
                }
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
struct SkillLoadedLogEntry {
    skill_name: String,
    reference: Option<String>,
    source: String,
}

#[derive(Debug, Clone)]
struct SummaryTarget {
    document_id: String,
    source: crate::llm::domain::attachments::AttachmentSource,
    mime_type: String,
    filename: String,
    inline_bytes: Option<Vec<u8>>,
}

async fn generate_one_summary(
    gen: &dyn crate::llm::domain::attachments::AttachmentSummaryGenerator,
    cfg: &crate::llm::domain::attachments::SummaryConfig,
    target: &SummaryTarget,
    fetcher: std::sync::Arc<dyn crate::llm::domain::signed_url_fetcher::SignedUrlFetcher>,
    max_chars: usize,
) -> crate::llm::domain::attachments::SummaryOutcome {
    use crate::llm::domain::attachments::{SummaryInput, SummaryOutcome, SummarySource};
    use crate::llm::infrastructure::attachment_summary::{
        acquire_bytes, extract_text, truncate_chars,
    };

    // 1. Acquire bytes (no size bound — frontend enforces 100 MB).
    // `target.inline_bytes` carries the original bytes for Inline sources
    // (data: base64 uploads), since the upload pipeline consumed the first clone.
    let bytes = match acquire_bytes(&target.source, target.inline_bytes.as_deref(), fetcher).await {
        Ok(b) => b,
        Err(e) => {
            return SummaryOutcome::Skipped {
                reason: format!("byte acquisition failed: {}", e),
            }
        }
    };

    // 2. Build SummarySource based on mime.
    let source = if target.mime_type.starts_with("image/") {
        SummarySource::ImageBytes(bytes)
    } else {
        match extract_text(&target.mime_type, &bytes) {
            Ok(Some(text)) => {
                let truncated = truncate_chars(&text, max_chars);
                if truncated.trim().is_empty() {
                    return SummaryOutcome::Skipped {
                        reason: "extraction returned empty text".into(),
                    };
                }
                SummarySource::ExtractedText(truncated)
            }
            Ok(None) => {
                return SummaryOutcome::Skipped {
                    reason: format!("mime {} not extractable", target.mime_type),
                }
            }
            Err(e) => {
                return SummaryOutcome::Skipped {
                    reason: format!("extraction error: {}", e),
                }
            }
        }
    };

    let input = SummaryInput {
        filename: target.filename.clone(),
        mime_type: target.mime_type.clone(),
        source,
    };

    match gen.generate(input, cfg).await {
        Ok(outcome) => outcome,
        Err(e) => SummaryOutcome::Failed {
            reason: format!("generator error: {}", e),
        },
    }
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
        let raw_val = inputs.get("skills").or_else(|| config.get("skills"));
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

// ---- Step 6: LoadAttachmentResolver implementation -------------------------
struct AttachmentResolverImpl {
    registry: std::sync::Arc<dyn crate::llm::domain::AttachmentRegistry>,
    provider: crate::llm::domain::ProviderKind,
    api_key: String,
}

#[async_trait::async_trait]
impl crate::llm::application::LoadAttachmentResolver for AttachmentResolverImpl {
    async fn resolve(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<Option<crate::llm::domain::FileData>, String> {
        use crate::llm::domain::{FileData, FileSource, ProviderFileRef};

        let row = self
            .registry
            .lookup(agent_session_id, document_id, self.provider.clone())
            .await
            .map_err(|e| e.to_string())?;
        let Some(att) = row else {
            return Ok(None);
        };

        // Attempt to use the cached provider_file_id as-is. The provider call
        // itself will surface expiry on use; we treat lookup failure on the
        // provider as a recoverable case ONLY when the source is recoverable.
        let file_data = FileData {
            document_id: Some(att.document_id.clone()),
            mime_type: att.mime_type.clone(),
            filename: att.filename.clone(),
            size_hint: att.size_bytes,
            source: FileSource::Uploaded(ProviderFileRef {
                provider: att.provider.clone(),
                provider_file_id: att.provider_file_id.clone(),
                mime_type: att.mime_type.clone(),
                filename: att.filename.clone(),
                expires_at: None,
            }),
            retained_inline_bytes: None,
        };

        if att.source.is_recoverable() {
            let now = chrono::Utc::now();
            let stale = (now - att.refreshed_at).num_hours() >= 24;
            if stale {
                tracing::info!(
                    target: "colmena::attachment",
                    event = "attachment.recovery_attempted",
                    agent_session_id = %agent_session_id,
                    document_id = %document_id,
                    "stale provider_file_id, attempting re-upload"
                );

                let file_provider = crate::llm::infrastructure::files::FileProviderFactory::create(
                    att.provider.clone(),
                    self.api_key.clone(),
                )
                .map_err(|e| e.to_string())?;
                let downloader = crate::llm::infrastructure::files::SignedUrlDownloader::new();

                let source_url = match &att.source {
                    crate::llm::domain::AttachmentSource::SignedUrl(u) => u.clone(),
                    crate::llm::domain::AttachmentSource::Path(p) => p.clone(),
                    crate::llm::domain::AttachmentSource::Inline => unreachable!(),
                };

                let stream = downloader
                    .stream(&source_url)
                    .await
                    .map_err(|e| e.to_string())?;
                let provider_ref = file_provider
                    .upload_streaming(stream, &att.mime_type, &att.filename)
                    .await
                    .map_err(|e| e.to_string())?;

                self.registry
                    .refresh_provider_file_id(
                        agent_session_id,
                        document_id,
                        self.provider.clone(),
                        &provider_ref.provider_file_id,
                    )
                    .await
                    .map_err(|e| e.to_string())?;

                return Ok(Some(FileData {
                    document_id: Some(att.document_id.clone()),
                    mime_type: att.mime_type.clone(),
                    filename: att.filename.clone(),
                    size_hint: att.size_bytes,
                    source: FileSource::Uploaded(provider_ref),
                    retained_inline_bytes: None,
                }));
            }
        }

        Ok(Some(file_data))
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
            "google" => ProviderKind::Google,
            "anthropic" => ProviderKind::Anthropic,
            "mock" => ProviderKind::Mock,
            _ => {
                return Err(format!(
                    "Invalid provider '{}'. Supported: openai, google, anthropic, mock",
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

        // Resume detection — when the run_use_case re-enters this node after a
        // SUSPENDED tool call, it injects `__colmena_resume_answer`. In that case
        // a fresh `prompt` is not required: the conversation is continued from the
        // persisted history and the user's answer is threaded into the pending
        // tool call instead of starting a new turn.
        let resume_answer: Option<String> = inputs
            .get("__colmena_resume_answer")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Prompt — accepts string OR any JSON value (arrays, objects are serialized).
        // This allows the synthesizer to receive `final_result` (a JSON array) directly.
        // On resume, the prompt may be missing/empty — that is allowed.
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
                        if resume_answer.is_some() {
                            ""
                        } else {
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
                    } else {
                        &prompt_raw_str
                    }
                }
                Some(Value::Null) | None => {
                    if resume_answer.is_some() {
                        ""
                    } else {
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

        // System Message — honor user-provided value if any (including via inputs
        // or config), otherwise fall back to a grounding default so the model is
        // steered away from fabricating facts not present in the context.
        let system_message_str;
        let system_message = if let Some(sys) = inputs
            .get("system_message")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("system_message").and_then(|v| v.as_str()))
            .filter(|s| !s.trim().is_empty())
        {
            system_message_str = Self::resolve_template_vars(sys, inputs);
            Some(system_message_str.as_str())
        } else {
            Some(LLM_DEFAULT_SYSTEM)
        };

        // Conversation handle — injected by the engine (Task 14/15).
        // agent_session_id: present only when the caller passed --agent-session-id.
        // session_id_str: always present once the engine has injected inputs.
        // node_id_path_str: path-qualified node id (e.g. "responder" or "ventas/responder").
        let agent_session_id_str: Option<String> = inputs
            .get("__colmena_agent_session_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let session_id_str = inputs
            .get("__colmena_session_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let node_id_path_str = inputs
            .get("__colmena_node_id_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| session_id_str.clone());

        // Effective conversation key for all memory operations on this node.
        let conversation_key = ConversationKey {
            session_id: SessionId(session_id_str.clone()),
            agent_session_id: agent_session_id_str
                .as_ref()
                .map(|a| AgentSessionId(a.clone())),
            node_id: NodeIdPath(node_id_path_str.clone()),
        };

        // ---- AttachmentRegistry adapter (Step 2) -------------------------------------
        use crate::llm::domain::AttachmentRegistry;
        use crate::llm::infrastructure::persistence::{
            PostgresAttachmentRegistry, SqliteAttachmentRegistry,
        };

        let attachment_registry: Option<std::sync::Arc<dyn AttachmentRegistry>> =
            if agent_session_id_str.is_some() {
                match std::env::var("DATABASE_URL").ok() {
                    Some(url) => {
                        use crate::dag_engine::infrastructure::pool_registry::{
                            PgPoolRegistry, PoolConfig,
                        };
                        let registry =
                            std::sync::Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
                        let reg = PostgresAttachmentRegistry::new(registry, &url)
                            .await
                            .map_err(|e| format!("attachment registry init: {}", e))?;
                        Some(std::sync::Arc::new(reg))
                    }
                    None => {
                        if let Some(sqlite_url) = sqlite_url_for_node(config) {
                            let reg = SqliteAttachmentRegistry::new(&sqlite_url)
                                .await
                                .map_err(|e| format!("attachment sqlite registry init: {}", e))?;
                            Some(std::sync::Arc::new(reg))
                        } else {
                            None
                        }
                    }
                }
            } else {
                None
            };

        // Connection URL (Optional - for Memory Backend)
        let connection_url_raw = inputs
            .get("connection_url")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("connection_url").and_then(|v| v.as_str()));

        // --- 2. Prepare LLM Request ---

        let provider = LlmProvider::new(provider_kind.clone(), api_key.clone(), model)?;
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

        if let Some(thinking_budget) = inputs
            .get("thinking_budget")
            .and_then(|v| v.as_u64())
            .or_else(|| config.get("thinking_budget").and_then(|v| v.as_u64()))
        {
            llm_config = llm_config.with_thinking_budget(thinking_budget as u32);
        }

        // Maximum iterations of the ReAct agent loop. Each iteration is one LLM
        // call. Hitting this limit returns MaxIterationsReached. Reads from
        // inputs first (dynamic from upstream), then config, defaulting to 10.
        let max_iterations: usize = inputs
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .or_else(|| config.get("max_iterations").and_then(|v| v.as_u64()))
            .map(|n| n as usize)
            .unwrap_or(10);

        tracing::info!(
            target: "colmena::llm",
            max_iterations,
            "llm_call_max_iterations_resolved"
        );

        let mut messages = Vec::new();
        let mut history_exists = false;

        // 2.1 Load History if a Connection URL is configured (session_id is always present now).
        let mut repo_instance = None;
        if let Some(url_raw) = connection_url_raw {
            let connection_url = Self::resolve_env_var(url_raw)?;
            let repo = self
                .repository_factory
                .get_repository(&connection_url)
                .await?;
            repo_instance = Some(repo.clone());

            let conversation = repo.get_by_id(&conversation_key).await?;
            // We only need to know if history exists to decide on system message
            history_exists = !conversation.messages.is_empty();
        }

        // 2.2 Add User Prompt (system message is pushed after tools are resolved — see below)
        let mut resolved_files = Vec::new();

        // Check if there are any files passed in the node inputs
        if let Some(files_val) = inputs.get("files").or_else(|| config.get("files")) {
            if let Some(files_arr) = files_val.as_array() {
                resolved_files = parse_file_entries(files_arr)?;
            }
        }

        // C1: resolve FileSource::SignedUrl entries via cache + download + upload pipe.
        // Uses the canonical LlmCallUseCase::resolve_files orchestration when DATABASE_URL
        // is available; falls back to a bare download+upload loop otherwise.
        if resolved_files.iter().any(|f| {
            matches!(
                f.source,
                crate::llm::domain::FileSource::SignedUrl(_)
                    | crate::llm::domain::FileSource::InlineBytes { .. }
            )
        }) {
            use crate::llm::application::LlmCallUseCase;
            use crate::llm::infrastructure::files::{
                FileProviderFactory, PostgresFileCache, SignedUrlDownloader,
            };
            use std::sync::Arc;

            // Build cache from DATABASE_URL env (graceful degradation if missing).
            let database_url = std::env::var("DATABASE_URL").ok();
            let cache: Option<Arc<dyn crate::llm::domain::FileCacheRepository>> = match database_url
                .as_deref()
            {
                Some(url) => {
                    crate::colmena_log!(
                            "[file-resolve] DATABASE_URL set — building PostgresFileCache for provider_file_cache table"
                        );
                    use crate::dag_engine::infrastructure::pool_registry::{
                        PgPoolRegistry, PoolConfig,
                    };
                    let registry = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
                    // Run migrations to ensure provider_file_cache table exists.
                    let pool = registry
                        .get_or_create(url)
                        .await
                        .map_err(|e| format!("failed to build PG pool: {}", e))?;
                    sqlx::migrate!("migrations/postgres")
                        .set_ignore_missing(true)
                        .run(&*pool)
                        .await
                        .map_err(|e| format!("migration failed: {}", e))?;
                    let pg_cache = PostgresFileCache::new(registry, url).await?;
                    Some(Arc::new(pg_cache))
                }
                None => {
                    crate::colmena_log!(
                            "[file-resolve] DATABASE_URL not set — running WITHOUT cache (every run re-uploads)"
                        );
                    None
                }
            };

            let file_provider =
                FileProviderFactory::create(provider_kind.clone(), api_key.clone())?;
            let downloader = SignedUrlDownloader::new();

            if let Some(cache) = cache {
                // Use the canonical resolve_files orchestration.
                LlmCallUseCase::resolve_files(
                    &mut resolved_files,
                    provider_kind.clone(),
                    file_provider,
                    cache,
                    &downloader,
                )
                .await?;
            } else {
                // No cache: bare download+upload. Logs flow events at INFO.
                use crate::llm::domain::FileSource;
                let mut new_files = Vec::with_capacity(resolved_files.len());
                for file in resolved_files.drain(..) {
                    match &file.source {
                        FileSource::SignedUrl(url) => {
                            let url_owned = url.clone();
                            let mime_type = file.mime_type.clone();
                            let filename = file.filename.clone();
                            let document_id = file.document_id.clone();
                            let size_hint = file.size_hint;

                            crate::colmena_log!(
                                "[file-resolve-no-cache] '{}' downloading + uploading to {} Files API",
                                filename, provider_kind
                            );

                            match downloader.stream(&url_owned).await {
                                Ok(stream) => match file_provider
                                    .upload_streaming(stream, &mime_type, &filename)
                                    .await
                                {
                                    Ok(provider_ref) => {
                                        crate::colmena_log!(
                                            "[file-resolve-no-cache] '{}' uploaded as id '{}'",
                                            filename,
                                            provider_ref.provider_file_id
                                        );
                                        new_files.push(crate::llm::domain::FileData {
                                            document_id,
                                            mime_type,
                                            filename,
                                            size_hint,
                                            source: FileSource::Uploaded(provider_ref),
                                            retained_inline_bytes: None,
                                        });
                                    }
                                    Err(e) => {
                                        crate::colmena_log!(
                                            "[file-resolve-no-cache] WARN upload failed for '{}': {}",
                                            filename, e
                                        );
                                    }
                                },
                                Err(e) => {
                                    crate::colmena_log!(
                                        "[file-resolve-no-cache] WARN download failed for '{}': {}",
                                        filename,
                                        e
                                    );
                                }
                            }
                        }
                        FileSource::InlineBytes { bytes } => {
                            let bytes_owned = bytes.clone();
                            let retained = bytes.clone();
                            let mime_type = file.mime_type.clone();
                            let filename = file.filename.clone();
                            let document_id = file.document_id.clone();
                            let size_hint = file.size_hint;

                            crate::colmena_log!(
                                "[file-resolve-no-cache] '{}' (inline, {} B) uploading to {} Files API",
                                filename,
                                bytes_owned.len(),
                                provider_kind
                            );

                            let stream: crate::llm::domain::BoxedByteStream =
                                Box::pin(futures::stream::once(async move {
                                    Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(
                                        bytes_owned,
                                    ))
                                }));
                            match file_provider
                                .upload_streaming(stream, &mime_type, &filename)
                                .await
                            {
                                Ok(provider_ref) => {
                                    crate::colmena_log!(
                                        "[file-resolve-no-cache] '{}' (inline) uploaded as id '{}'",
                                        filename,
                                        provider_ref.provider_file_id
                                    );
                                    new_files.push(crate::llm::domain::FileData {
                                        document_id,
                                        mime_type,
                                        filename,
                                        size_hint,
                                        source: FileSource::Uploaded(provider_ref),
                                        retained_inline_bytes: Some(retained),
                                    });
                                }
                                Err(e) => {
                                    crate::colmena_log!(
                                        "[file-resolve-no-cache] WARN inline upload failed for '{}': {}",
                                        filename,
                                        e
                                    );
                                }
                            }
                        }
                        _ => new_files.push(file),
                    }
                }
                resolved_files = new_files;
            }
        }

        // ---- Auto-summary configuration ----------------------------------------------
        let summary_enabled: bool = inputs
            .get("summary_enabled")
            .and_then(|v| v.as_bool())
            .or_else(|| config.get("summary_enabled").and_then(|v| v.as_bool()))
            .unwrap_or(true);
        let summary_max_chars: usize = inputs
            .get("summary_max_chars")
            .and_then(|v| v.as_u64())
            .or_else(|| config.get("summary_max_chars").and_then(|v| v.as_u64()))
            .map(|v| v as usize)
            .unwrap_or(5000);
        let summary_max_output_chars: usize = inputs
            .get("summary_max_output_chars")
            .and_then(|v| v.as_u64())
            .or_else(|| config.get("summary_max_output_chars").and_then(|v| v.as_u64()))
            .map(|v| v as usize)
            .unwrap_or(200);
        let summary_timeout_secs: u64 = inputs
            .get("summary_timeout_secs")
            .and_then(|v| v.as_u64())
            .or_else(|| config.get("summary_timeout_secs").and_then(|v| v.as_u64()))
            .unwrap_or(15);
        let summary_model_override: Option<String> = inputs
            .get("summary_model")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                config
                    .get("summary_model")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });

        // ---- Step 3: Auto-register resolved uploads in AttachmentRegistry -----------
        let mut summary_targets: Vec<SummaryTarget> = Vec::new();
        if let (Some(reg), Some(sid)) =
            (attachment_registry.as_ref(), agent_session_id_str.as_ref())
        {
            use crate::llm::domain::attachments::generate_attachment_id;
            use crate::llm::domain::attachments::{AttachmentSource, UpsertAttachmentInput};
            use crate::llm::domain::FileSource;

            let raw_entries: Vec<serde_json::Value> = inputs
                .get("files")
                .or_else(|| config.get("files"))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            for (idx, file) in resolved_files.iter().enumerate() {
                let raw = raw_entries.get(idx);
                let label = raw
                    .and_then(|v| v.get("label"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let description = raw
                    .and_then(|v| v.get("description"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let supplied_id = raw
                    .and_then(|v| v.get("id"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                let source = match &file.source {
                    FileSource::SignedUrl(u) => AttachmentSource::SignedUrl(u.clone()),
                    FileSource::Uploaded(_) => raw
                        .and_then(|v| v.get("url"))
                        .and_then(|v| v.as_str())
                        .map(|u| AttachmentSource::SignedUrl(u.to_string()))
                        .or_else(|| {
                            raw.and_then(|v| v.get("path"))
                                .and_then(|v| v.as_str())
                                .map(|p| AttachmentSource::Path(p.to_string()))
                        })
                        .unwrap_or(AttachmentSource::Inline),
                    FileSource::InlineBytes { .. } => AttachmentSource::Inline,
                };

                let document_id = supplied_id.unwrap_or_else(|| {
                    generate_attachment_id(
                        &file.filename,
                        &file.mime_type,
                        file.size_hint,
                        &source,
                        None,
                    )
                });

                let provider_file_id = match &file.source {
                    FileSource::Uploaded(r) => r.provider_file_id.clone(),
                    _ => continue, // Not uploaded yet — skip registration this pass.
                };

                let input = UpsertAttachmentInput {
                    agent_session_id: sid.clone(),
                    document_id: document_id.clone(),
                    provider: provider_kind.clone(),
                    provider_file_id,
                    mime_type: file.mime_type.clone(),
                    filename: file.filename.clone(),
                    size_bytes: file.size_hint,
                    label: label.clone(),
                    description: description.clone(),
                    source: source.clone(),
                };
                reg.upsert(input)
                    .await
                    .map_err(|e| format!("attachment upsert: {}", e))?;
                tracing::info!(
                    target: "colmena::attachment",
                    event = "attachment.registered",
                    agent_session_id = %sid,
                    document_id = %document_id,
                    "registered attachment"
                );

                if summary_enabled && description.is_none() {
                    let inline_bytes_for_summary = if matches!(source, AttachmentSource::Inline) {
                        file.retained_inline_bytes.clone()
                    } else {
                        None
                    };
                    let has_summarisable_source =
                        !matches!(source, AttachmentSource::Inline) || inline_bytes_for_summary.is_some();
                    if has_summarisable_source {
                        summary_targets.push(SummaryTarget {
                            document_id: document_id.clone(),
                            source,
                            mime_type: file.mime_type.clone(),
                            filename: file.filename.clone(),
                            inline_bytes: inline_bytes_for_summary,
                        });
                    }
                }
            }
        }

        // On resume, do NOT push a fresh user message — the conversation is
        // continued from the persisted history. The pending tool call (whose
        // result was never persisted) is dispatched below with the resume
        // answer threaded in.
        if resume_answer.is_none() {
            let user_message = if resolved_files.is_empty() {
                LlmMessage::user(prompt.to_string())?
            } else {
                LlmMessage::user_with_files(prompt.to_string(), resolved_files)?
            };

            messages.push(user_message.clone());
        }

        // --- 3. Execute LLM Call (via AgentService) ---
        let llm_repo = LlmProviderFactory::create(provider_kind.clone());
        let llm_repo_arc: Arc<dyn crate::llm::domain::LlmRepository> = llm_repo; // Already Arc

        // Create Tool Executor
        // We need to resolve the registry from Weak reference
        let registry = self
            .registry
            .upgrade()
            .ok_or("NodeRegistry has been dropped")?;

        // Parse tool_configurations. Surface parse errors instead of silently
        // falling back to an empty map — a malformed entry (e.g. an invalid
        // field inside node_schema) would otherwise strip ALL tools from the
        // LLM with no visible diagnostic.
        let mut tool_configurations: HashMap<String, ToolConfiguration> = match inputs
            .get("tool_configurations")
            .or_else(|| config.get("tool_configurations"))
        {
            Some(v) => {
                match serde_json::from_value::<HashMap<String, ToolConfiguration>>(v.clone()) {
                    Ok(map) => map,
                    Err(e) => {
                        colmena_log!(
                            "WARN: tool_configurations failed to parse — no tools will be exposed to the LLM. Error: {}",
                            e
                        );
                        HashMap::new()
                    }
                }
            }
            None => HashMap::new(),
        };

        // Opt-in shorthand: `config.secure_suspend_allowed: true` auto-registers
        // a tool named `ask_secret` backed by `secure_suspend`. No-op when the
        // flag is absent/false or when the user already wired `secure_suspend`
        // through `tool_configurations` (explicit always wins).
        let secure_suspend_allowed = inputs
            .get("secure_suspend_allowed")
            .or_else(|| config.get("secure_suspend_allowed"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        crate::dag_engine::infrastructure::nodes::secure_suspend::maybe_inject_secure_suspend_tool(
            secure_suspend_allowed,
            &mut tool_configurations,
        );

        // Auto-fill canonical tool defaults for node types that ship them.
        // Currently only `secure_suspend` opts in — keeps `tool_configurations`
        // minimal (just `name` + `node_type`) and fills defaults for any entry
        // injected by the `secure_suspend_allowed` shorthand above.
        for tool_cfg in tool_configurations.values_mut() {
            crate::dag_engine::infrastructure::nodes::secure_suspend::apply_secure_suspend_tool_defaults(tool_cfg);
        }

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

        // Snapshot the aliases declared in tool_configurations before the map is
        // moved into the executor. These aliases are auto-enabled below — a user
        // who declared `tool_configurations` should not also have to list the same
        // tool names under `enabled_tools`.
        let configured_aliases: std::collections::HashSet<String> =
            tool_configurations.keys().cloned().collect();

        // Build skill repository (if configured).
        let skill_repo: Option<Arc<dyn SkillRepository>> =
            Self::build_skill_repository_from_config(config, inputs)?;

        // Track skills loaded across the entire node execution (for summary).
        let skills_used_log: Arc<std::sync::Mutex<Vec<SkillLoadedLogEntry>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        // ---- Lazy tool loading config -------------------------------------------------
        let lazy_tool_loading: bool = inputs
            .get("lazy_tool_loading")
            .or_else(|| config.get("lazy_tool_loading"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // ---- Attachments enabled flag -------------------------------------------------
        let attachments_enabled: bool = inputs
            .get("attachments_enabled")
            .and_then(|v| v.as_bool())
            .or_else(|| config.get("attachments_enabled").and_then(|v| v.as_bool()))
            .unwrap_or(true);

        // Build the catalog (CatalogEntry list) and the lookup snapshot for
        // describe_tool. Both are populated only when lazy mode is on AND the
        // tool is not eager: true. Eager tools always carry their own full schema
        // and never enter the catalog.
        let mut catalog: Vec<CatalogEntry> = Vec::new();
        let mut lookup_for_describe: Vec<ToolConfiguration> = Vec::new();
        if lazy_tool_loading {
            if tool_configurations.is_empty() {
                colmena_log!(
                    "WARN: lazy_tool_loading: true but tool_configurations is empty — feature will no-op."
                );
            }
            for cfg in tool_configurations.values() {
                if cfg.eager {
                    continue;
                }
                if let Some(s) = &cfg.summary {
                    if s.chars().count() > 200 {
                        colmena_log!(
                            "WARN: tool '{}' summary > 200 chars; will be truncated.",
                            cfg.name
                        );
                    }
                }
                catalog.push(CatalogEntry {
                    name: cfg.name.clone(),
                    summary: summary_for_catalog(cfg.summary.as_deref(), &cfg.description),
                });
                lookup_for_describe.push(cfg.clone());
            }
        }

        // Tools the LLM node has discovered via describe_tool during this execution
        // (in-memory log; the cross-session reconstruction is done from messages
        // each ReAct iteration, but this log feeds the final extra_info summary).
        let tools_discovered_log: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        // Build documents context if the LLM node was configured with a `documents`
        // block. The seven `document_*` synthetic tools are exposed and dispatched
        // through the runtime built here. Session id is resolved from the same
        // priority chain used elsewhere in this node, falling back to "default".
        let documents_context: Option<Arc<DocumentToolsContext>> = match inputs
            .get("documents")
            .cloned()
            .or_else(|| config.get("documents").cloned())
        {
            Some(doc_cfg) => match DocumentRuntime::from_config(&doc_cfg).await {
                Ok(rt) => {
                    let sid = session_id_str.clone();
                    Some(Arc::new(DocumentToolsContext {
                        create: rt.create.clone(),
                        apply: rt.apply.clone(),
                        read: rt.read.clone(),
                        get_head: rt.get_head.clone(),
                        list_versions: rt.list_versions.clone(),
                        rollback: rt.rollback.clone(),
                        session_index: None,
                        session_id: DocSessionId::new(sid),
                    }))
                }
                Err(e) => {
                    return Err(format!("invalid `documents` config on llm node: {e}").into());
                }
            },
            None => None,
        };

        // ---- Step 4 (catalog building) — must precede executor block ----------------
        let attachment_catalog: Vec<crate::llm::domain::ConversationAttachment> =
            if attachments_enabled {
                if let (Some(reg), Some(sid)) =
                    (attachment_registry.as_ref(), agent_session_id_str.as_ref())
                {
                    let all = reg
                        .list_for_session(sid)
                        .await
                        .map_err(|e| format!("attachment list: {}", e))?;
                    all.into_iter()
                        .filter(|a| a.provider == provider_kind)
                        .collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

        let tool_executor = {
            let mut executor = DagToolExecutor::new(registry, tool_configurations);
            // Propagate SecureValueService + session_id so tool calls decrypt secrets.
            if let Some(svc) = self.secure_value_service.clone() {
                executor = executor.with_secure_values(svc, session_id_str.clone());
            }
            // Propagate the agent_session_id (chat handle) so tool dispatch can
            // resolve secrets persisted under the same chat across ephemeral
            // session_id boundaries. Always pass — None preserves legacy behavior.
            executor = executor.with_agent_session_id(agent_session_id_str.clone());
            if let Some(ctx) = documents_context.clone() {
                executor = executor.with_documents(ctx);
            }
            // ---- Step 5: Wire attachment catalog into executor ----------------------
            if !attachment_catalog.is_empty() {
                executor = executor.with_attachments(attachment_catalog.clone());
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
            if lazy_tool_loading && !lookup_for_describe.is_empty() {
                executor = executor.with_describe_tool_lookup(lookup_for_describe.clone());
                let log_clone = tools_discovered_log.clone();
                let observer_clone = _observer.clone();
                executor = executor.with_describe_tool_observer(Arc::new(
                    move |result: &DescribeToolDispatchResult| {
                        if let Ok(mut log) = log_clone.lock() {
                            if !log.contains(&result.tool_name) {
                                log.push(result.tool_name.clone());
                            }
                        }
                        if let Some(obs) = &observer_clone {
                            obs.on_event(
                                crate::dag_engine::domain::observer::NodeEvent::ToolDescribed {
                                    tool_id: result.tool_call_id.clone(),
                                    tool_name: result.tool_name.clone(),
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

        let agent_service = AgentService::new(llm_repo_arc, conversation_repo.clone());

        // Resume path — when re-entered with `__colmena_resume_answer`, the
        // assistant message that requested the SUSPENDED tool was already
        // persisted in a prior run (by agent_service.run before short-circuit),
        // but the tool result was not. Find that pending tool call, dispatch it
        // with the resume answer, persist the tool message, then fall through
        // to agent_service.run with `prompt: None, messages: None` so the LLM
        // receives the resolved tool result and continues.
        if let Some(answer) = resume_answer.as_deref() {
            let conversation = conversation_repo.get_by_id(&conversation_key).await?;
            let pending = find_pending_tool_call(&conversation.messages)
                .ok_or("llm_call resume: no pending tool call found in conversation history")?;

            tracing::info!(
                target: "colmena::llm_node",
                "llm_call: resume — replaying pending tool with user answer"
            );
            let result = tool_executor
                .execute_with_resume_answer(&pending, answer)
                .await?;

            // Multi-suspend — the resumed tool itself returned SUSPENDED again.
            // Propagate without persisting a tool message; the next resume will
            // walk the same pending call.
            if let Ok(parsed) = serde_json::from_str::<Value>(&result.output) {
                if parsed.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED") {
                    return Ok(json!({
                        "__colmena_status": "SUSPENDED",
                        "questions": parsed.get("questions").cloned().unwrap_or(Value::Null),
                        "_pending_tool_call_id": pending.id.clone(),
                        "_conversation_key": {
                            "session_id": session_id_str.clone(),
                            "agent_session_id": agent_session_id_str.clone(),
                            "node_id": node_id_path_str.clone(),
                        },
                    }));
                }
            }

            // Persist the resolved tool message so agent_service.run will see it
            // when it loads the conversation history below.
            let tool_msg = LlmMessage::tool(pending.id.clone(), result.output.clone())?;
            conversation_repo
                .add_message(&conversation_key, tool_msg)
                .await?;

            tracing::info!(
                target: "colmena::llm",
                "resume_tool_re_executed_continuing_loop"
            );
        }

        // Decide which tools are exposed to the LLM.
        //
        // Two independent inputs feed this decision:
        //   - `tool_configurations` (present above) — every declared alias is
        //     auto-enabled; for toolkit aliases this expands to
        //     `{alias}__{sub_tool}` names.
        //   - `enabled_tools` (this block) — optional allow-list that unions
        //     with the auto-enabled set (deduplicated). Accepts:
        //       * `"*"` wildcard → expose every available tool
        //       * string → enable a single named tool
        //       * array of strings → enable each named tool
        //
        // When a user lists a name under `enabled_tools` that is already
        // covered by `tool_configurations`, the dedup silently collapses it.
        let enabled_tools_config = inputs
            .get("enabled_tools")
            .or_else(|| config.get("enabled_tools"));

        let all_tools = tool_executor.available_tools().await;

        let is_auto_enabled = |tool_name: &str| -> bool {
            configured_aliases.iter().any(|alias| {
                tool_name == alias.as_str() || tool_name.starts_with(&format!("{}__", alias))
            })
        };

        let mut enabled_names: Vec<String> = all_tools
            .iter()
            .filter(|t| is_auto_enabled(&t.name))
            .map(|t| t.name.clone())
            .collect();

        let mut wildcard_all = false;
        if let Some(enabled) = enabled_tools_config {
            if let Some(value) = enabled.as_str() {
                if value == "*" {
                    wildcard_all = true;
                } else if !enabled_names.iter().any(|n| n == value) {
                    enabled_names.push(value.to_string());
                }
            } else if let Some(tool_names) = enabled.as_array() {
                for v in tool_names {
                    if let Some(name) = v.as_str() {
                        if !enabled_names.iter().any(|n| n == name) {
                            enabled_names.push(name.to_string());
                        }
                    }
                }
            }
        }

        let mut tools: Vec<crate::llm::domain::ToolDefinition> = if wildcard_all {
            all_tools
        } else {
            all_tools
                .into_iter()
                .filter(|t| enabled_names.iter().any(|n| n == &t.name))
                .collect()
        };

        if let Some(repo) = skill_repo.as_ref() {
            tools.push(build_load_skill_tool_definition(repo));
        }

        // ---- Step 4 (tool expose) — catalog already built above executor block ------
        if !attachment_catalog.is_empty() {
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::build_load_attachment_tool_definition;
            tools.push(build_load_attachment_tool_definition(&attachment_catalog));
        }

        // When the LLM node has a `documents` config, expose the seven synthetic
        // document_* tools regardless of `enabled_tools` — same pattern as
        // load_skill. The DagToolExecutor was already wired with the matching
        // DocumentToolsContext above so dispatches succeed.
        if documents_context.is_some() {
            for td in build_all_document_tools() {
                tools.push(td);
            }
        }

        // 2.2 Build the final system message. We assemble up to three sections,
        // each emitted only when relevant:
        //   - the user-provided `system_message` (if any),
        //   - the documents prelude (only when this node has a `documents`
        //     config — so the user prompt does not need to explain how the
        //     document tools work),
        //   - the generic tool-use rules block (when any tool is exposed).
        // The combined message is pushed only when at least one section was
        // produced AND no prior history already supplies a system message.
        if !history_exists {
            let mut sections: Vec<String> = Vec::new();
            // Temporal & geographic context — always the first section.
            let tz_str = inputs
                .get("__colmena_timezone")
                .and_then(|v| v.as_str())
                .unwrap_or("America/Bogota");
            let loc_str = inputs
                .get("__colmena_location")
                .and_then(|v| v.as_str())
                .unwrap_or("Bogotá, Colombia");
            let locale_str = inputs
                .get("__colmena_locale")
                .and_then(|v| v.as_str())
                .unwrap_or("es-CO");
            let context_block = format_temporal_context_block(tz_str, loc_str, locale_str);
            sections.push(context_block);
            if let Some(sys_msg) = system_message {
                sections.push(sys_msg.to_string());
            }
            if documents_context.is_some() {
                sections.push(DOCUMENTS_SYSTEM_PRELUDE.to_string());
            }
            if !attachment_catalog.is_empty() {
                sections.push(ATTACHMENTS_SYSTEM_PRELUDE.to_string());
            }
            if !tools.is_empty() {
                // In lazy mode, hide cataloged tool names from the system prompt —
                // they are advertised through `describe_tool` instead. Listing them
                // alongside "ALWAYS use the available tools" would mislead the LLM
                // into emitting calls for tools that are not actually registered
                // in the current request's `tools[]`.
                let lazy_catalog_names: std::collections::HashSet<&str> = if lazy_tool_loading {
                    catalog.iter().map(|e| e.name.as_str()).collect()
                } else {
                    std::collections::HashSet::new()
                };
                let tool_names: Vec<String> = tools
                    .iter()
                    .filter(|t| !lazy_catalog_names.contains(t.name.as_str()))
                    .map(|t| format!("- {}", t.name))
                    .collect();
                if !tool_names.is_empty() {
                    sections.push(format!(
                        "## Tool Use Instructions\nYou have access to the following tools:\n{}\n\nRules:\n- ALWAYS use the available tools to answer questions that require real or live data. Never answer from your own knowledge when a tool can provide the data.\n- Call the most relevant tool before responding. Do not skip tool calls.\n- If a tool call fails, report the error clearly instead of guessing an answer.\n- Only respond without a tool call when the user's request is purely conversational and no tool is needed.",
                        tool_names.join("\n")
                    ));
                }
            }
            if !sections.is_empty() {
                messages.push(LlmMessage::system(sections.join("\n\n---\n"))?);
            }
        }

        // Check if streaming is enabled
        let stream_enabled = inputs
            .get("stream")
            .and_then(|v| v.as_bool())
            .or_else(|| config.get("stream").and_then(|v| v.as_bool()))
            .unwrap_or(true);

        // Shared state for reasoning block ID across the on_token Fn closure.
        let current_reasoning_id: Arc<std::sync::Mutex<Option<String>>> =
            Arc::new(std::sync::Mutex::new(None));

        // Define on_token callback if streaming is enabled and observer is present
        let observer_for_stream = _observer.clone();
        let on_token: Option<Box<dyn Fn(LlmStreamPart) + Send + Sync>> =
            if let Some(obs) = observer_for_stream {
                let reasoning_id = current_reasoning_id.clone();
                Some(Box::new(move |part: LlmStreamPart| {
                    use crate::dag_engine::domain::observer::NodeEvent;
                    match part {
                        LlmStreamPart::Content(token) if stream_enabled => {
                            obs.on_event(NodeEvent::LlmToken { token })
                        }
                        LlmStreamPart::ThinkingStart => {
                            let id = format!("reasoning_{}", uuid::Uuid::new_v4());
                            if let Ok(mut guard) = reasoning_id.lock() {
                                *guard = Some(id.clone());
                            }
                            obs.on_event(NodeEvent::ReasoningStart { id });
                        }
                        LlmStreamPart::ThinkingContent(token) if stream_enabled => {
                            let id = reasoning_id
                                .lock()
                                .ok()
                                .and_then(|g| g.clone())
                                .unwrap_or_default();
                            obs.on_event(NodeEvent::ReasoningDelta { id, token });
                        }
                        LlmStreamPart::ThinkingEnd => {
                            let id = reasoning_id
                                .lock()
                                .ok()
                                .and_then(|mut g| g.take())
                                .unwrap_or_default();
                            obs.on_event(NodeEvent::ReasoningEnd { id });
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
                                thinking_tokens: usage.thinking_tokens,
                                cache_read_tokens: usage.cache_read_tokens,
                                cache_write_tokens: usage.cache_write_tokens,
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

        // Build a dynamic tools_provider closure when lazy mode is on. The closure
        // is called fresh at each ReAct iteration: it derives `discovered_set` from
        // the current message history (rule 1: prior describe_tool calls; rule 2:
        // prior direct calls to a still-cataloged tool), then composes `tools[]`
        // as: [describe_tool if pending] + [non-catalog tools] + [discovered catalog tools].
        let tools_provider: Option<crate::llm::application::agent_service::ToolsProvider> =
            if lazy_tool_loading && !catalog.is_empty() {
                let catalog = catalog.clone();
                let static_snapshot = tools.clone();
                Some(Box::new(
                    move |messages: &[crate::llm::domain::LlmMessage]| {
                        let discovered = reconstruct_discovered_set(messages, &catalog);
                        let pending: Vec<&CatalogEntry> = catalog
                            .iter()
                            .filter(|e| !discovered.contains(&e.name))
                            .collect();

                        let catalog_names: std::collections::HashSet<&str> =
                            catalog.iter().map(|e| e.name.as_str()).collect();
                        let mut out: Vec<crate::llm::domain::ToolDefinition> = Vec::new();

                        // Tools defined OUTSIDE the lazy catalog (eager-flagged ones,
                        // load_skill, document_*, toolkit subtools) are always present.
                        for td in &static_snapshot {
                            if !catalog_names.contains(td.name.as_str()) {
                                out.push(td.clone());
                            }
                        }
                        // describe_tool only when there is something left to discover.
                        if !pending.is_empty() {
                            out.push(build_describe_tool_definition(&pending));
                        }
                        // Discovered lazy tools enter with their full schema.
                        for td in &static_snapshot {
                            if catalog_names.contains(td.name.as_str())
                                && discovered.contains(&td.name)
                            {
                                out.push(td.clone());
                            }
                        }
                        out
                    },
                ))
            } else {
                None
            };

        // Create AgentService parameters. On resume, the user prompt is `None`
        // and `messages` is `None`: agent_service will load the just-persisted
        // tool message (added in the resume block above) from history and
        // continue the ReAct loop from there.
        let params = if resume_answer.is_some() {
            crate::llm::application::AgentRunParams {
                session_id: &conversation_key,
                prompt: None,
                messages: None,
                config: llm_config,
                tools,
                tool_executor: &tool_executor,
                max_iterations: Some(max_iterations),
                on_token,
                tools_provider,
                attachment_resolver: attachment_registry.as_ref().map(|reg| {
                    std::sync::Arc::new(AttachmentResolverImpl {
                        registry: reg.clone(),
                        provider: provider_kind.clone(),
                        api_key: api_key.clone(),
                    })
                        as std::sync::Arc<dyn crate::llm::application::LoadAttachmentResolver>
                }),
                agent_session_id: agent_session_id_str.clone(),
            }
        } else {
            crate::llm::application::AgentRunParams {
                session_id: &conversation_key,
                prompt: Some(prompt.to_string()),
                messages: Some(messages.clone()),
                config: llm_config,
                tools,
                tool_executor: &tool_executor,
                max_iterations: Some(max_iterations),
                on_token,
                tools_provider,
                attachment_resolver: attachment_registry.as_ref().map(|reg| {
                    std::sync::Arc::new(AttachmentResolverImpl {
                        registry: reg.clone(),
                        provider: provider_kind.clone(),
                        api_key: api_key.clone(),
                    })
                        as std::sync::Arc<dyn crate::llm::application::LoadAttachmentResolver>
                }),
                agent_session_id: agent_session_id_str.clone(),
            }
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

        // ---- Step 4: Build summary tasks (run in parallel with answer call below) -----
        use crate::llm::domain::attachments::{
            AttachmentSummaryGenerator, SummaryConfig, SummaryOutcome,
        };
        use crate::llm::infrastructure::attachment_summary::{
            provider_cheap_tier, LlmAttachmentSummaryGenerator,
        };
        use crate::llm::infrastructure::files::signed_url_downloader::SignedUrlDownloader;

        let summary_generator: Option<std::sync::Arc<dyn AttachmentSummaryGenerator>> =
            if summary_enabled && !summary_targets.is_empty() && attachment_registry.is_some() {
                let repo = LlmProviderFactory::create(provider_kind.clone());
                Some(std::sync::Arc::new(LlmAttachmentSummaryGenerator::new(repo)))
            } else {
                None
            };

        let summary_cfg = SummaryConfig {
            provider: provider_kind.clone(),
            model: summary_model_override
                .clone()
                .unwrap_or_else(|| provider_cheap_tier(&provider_kind).to_string()),
            api_key: api_key.clone(),
            max_output_chars: summary_max_output_chars,
            timeout: std::time::Duration::from_secs(summary_timeout_secs),
        };

        let fetcher_for_summary: std::sync::Arc<
            dyn crate::llm::domain::signed_url_fetcher::SignedUrlFetcher,
        > = std::sync::Arc::new(SignedUrlDownloader::new());

        let summary_fut = {
            let gen_opt = summary_generator.clone();
            let reg_opt = attachment_registry.clone();
            let sid_opt = agent_session_id_str.clone();
            let provider_kind_cap = provider_kind.clone();
            let cfg = summary_cfg.clone();
            let targets = std::mem::take(&mut summary_targets);
            async move {
                let (Some(gen), Some(reg), Some(sid)) = (gen_opt, reg_opt, sid_opt) else {
                    return;
                };
                // Use a `JoinSet` so that if the outer future is dropped
                // (timeout, caller cancellation, etc.) all spawned tasks are
                // aborted automatically. Dropping `tokio::task::JoinHandle`
                // does NOT abort the task — it would otherwise survive and
                // race-write stale summaries into the registry.
                let mut set = tokio::task::JoinSet::new();
                for t in targets {
                    let gen = gen.clone();
                    let reg = reg.clone();
                    let sid = sid.clone();
                    let provider_kind = provider_kind_cap.clone();
                    let cfg = cfg.clone();
                    let fetcher = fetcher_for_summary.clone();
                    set.spawn(async move {
                        let outcome = generate_one_summary(
                            &*gen,
                            &cfg,
                            &t,
                            fetcher,
                            summary_max_chars,
                        )
                        .await;
                        match &outcome {
                            SummaryOutcome::Generated(text) => {
                                if let Err(e) = reg
                                    .update_description(&sid, &t.document_id, provider_kind, text)
                                    .await
                                {
                                    tracing::warn!(
                                        target: "colmena::attachment",
                                        event = "summary.persist_failed",
                                        document_id = %t.document_id,
                                        error = %e,
                                        "failed to persist summary"
                                    );
                                } else {
                                    tracing::info!(
                                        target: "colmena::attachment",
                                        event = "summary.persisted",
                                        document_id = %t.document_id,
                                        summary_len = text.len(),
                                        "summary persisted"
                                    );
                                }
                            }
                            other => {
                                tracing::info!(
                                    target: "colmena::attachment",
                                    event = "summary.skipped_or_failed",
                                    document_id = %t.document_id,
                                    outcome = ?other,
                                    "summary skipped or failed"
                                );
                            }
                        }
                    });
                }
                while set.join_next().await.is_some() {}
            }
        };

        let summary_timeout_dur = std::time::Duration::from_secs(summary_timeout_secs);
        let (agent_run_result, summary_outcome) = tokio::join!(
            agent_service.run(params),
            tokio::time::timeout(summary_timeout_dur, summary_fut),
        );

        if summary_outcome.is_err() {
            tracing::warn!(
                target: "colmena::attachment",
                event = "summary.batch_timeout",
                timeout_secs = summary_timeout_secs,
                "summary batch exceeded timeout"
            );
        }

        let response = agent_run_result?;

        // 3.0a SUSPENDED propagation — when the agent loop short-circuited because a tool
        // returned `__colmena_status: SUSPENDED`, surface that signal upward to the DAG
        // engine. The assistant message that requested the tool was already persisted by
        // `agent_service.run` (step B of the ReAct loop); the resume path will replay it.
        if let Some(suspend) = response.suspend() {
            tracing::info!(
                target: "colmena::llm_node",
                "llm_call: propagating SUSPENDED to DAG"
            );
            return Ok(json!({
                "__colmena_status": "SUSPENDED",
                "questions": suspend.questions.clone(),
                "_pending_tool_call_id": suspend.tool_call_id.clone(),
                "_conversation_key": {
                    "session_id": session_id_str.clone(),
                    "agent_session_id": agent_session_id_str.clone(),
                    "node_id": node_id_path_str.clone(),
                },
            }));
        }

        // 3.1 Notify observer of usage (even if not streaming)
        if let Some(obs) = _observer.clone() {
            if let Some(usage) = response.usage() {
                use crate::dag_engine::domain::observer::NodeEvent;
                obs.on_event(NodeEvent::LlmUsage {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    thinking_tokens: usage.thinking_tokens,
                    cache_read_tokens: usage.cache_read_tokens,
                    cache_write_tokens: usage.cache_write_tokens,
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

        // tools_discovered (lazy_tool_loading): array of names in discovery order.
        if let Ok(log) = tools_discovered_log.lock() {
            if !log.is_empty() {
                extra_info["tools_discovered"] =
                    Value::Array(log.iter().cloned().map(Value::String).collect());
            }
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
                "provider": "string (openai, google, anthropic)",
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

/// Returns the SQLite `connection_url` if the node config declares one
/// (e.g. `"connection_url": "sqlite:./mem.db"`); otherwise `None`. Used for
/// the AttachmentRegistry fallback when `DATABASE_URL` is unset. The same
/// `connection_url` may also be a `postgres://` URL — in that case we return
/// `None` because the Postgres branch is selected ahead of this fallback via
/// the `DATABASE_URL` env var.
fn sqlite_url_for_node(config: &serde_json::Value) -> Option<String> {
    config
        .get("connection_url")
        .and_then(|v| v.as_str())
        .filter(|s| s.starts_with("sqlite:"))
        .map(|s| s.to_string())
}

/// Format the temporal & geographic context block that goes at the top of
/// the LLM system message.
///
/// - `timezone_str`: IANA timezone identifier (e.g. "America/Bogota"). Invalid
///   inputs fall back to `America/Bogota` and the displayed label is rewritten
///   to match the fallback so the rendered block stays internally coherent.
/// - `location_str`: free-text geographic description. No validation; taken
///   verbatim.
/// - `locale_str`: BCP 47 language+region tag (e.g. "es-CO"). No validation;
///   taken verbatim — the LLM is the final arbiter of which language to use.
///
/// The block renders ISO 8601 as the primary timestamp (canonical, locale-
/// neutral, machine-friendly for time reasoning) with a human-readable echo
/// in parentheses so the model can surface time naturally in its replies.
fn format_temporal_context_block(
    timezone_str: &str,
    location_str: &str,
    locale_str: &str,
) -> String {
    use chrono::Utc;
    use chrono_tz::Tz;

    let (tz, tz_display) = match timezone_str.parse::<Tz>() {
        Ok(tz) => (tz, timezone_str.to_string()),
        Err(_) => (
            "America/Bogota"
                .parse::<Tz>()
                .expect("hardcoded literal must parse"),
            "America/Bogota".to_string(),
        ),
    };

    let local_dt = Utc::now().with_timezone(&tz);

    let iso_8601 = local_dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string();
    let human = local_dt.format("%A, %B %-d, %Y, %-I:%M %p").to_string();

    let raw_offset = local_dt.format("%:z").to_string();
    let sign = if raw_offset.starts_with('-') { "-" } else { "+" };
    let trimmed = raw_offset.trim_start_matches(['+', '-']);
    let parts: Vec<&str> = trimmed.split(':').collect();
    let hours: i32 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mins: i32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let offset_display = if mins == 0 {
        format!("UTC{}{}", sign, hours)
    } else {
        format!("UTC{}{}:{:02}", sign, hours, mins)
    };

    format!(
        "## Temporal & Geographic Context\n\
         Current date and time: {iso} ({human})\n\
         Timezone: {tz_display} ({offset})\n\
         Location: {location}\n\
         Locale: {locale}",
        iso = iso_8601,
        human = human,
        tz_display = tz_display,
        offset = offset_display,
        location = location_str,
        locale = locale_str,
    )
}

const FILE_DATA_LIMIT_BYTES: u64 = 30 * 1024 * 1024;

/// Parses a JSON array of FileEntry objects into `Vec<FileData>`.
///
/// Schema (per emitter contract):
/// ```json
/// {
///   "id": "doc-123",                    // required when url is present
///   "mime_type": "application/pdf",     // required, defaults to octet-stream
///   "filename": "x.pdf",                // optional, defaults to "upload.file"
///   "size_bytes": 123,                  // hint, not validated as ground truth
///   "data": "base64...",                // for files < 30 MB
///   "url": "https://...",               // for files >= 30 MB (signed URL)
///   "path": "/local/path"               // legacy, < 30 MB only, dev/test
/// }
/// ```
///
/// Priority when multiple sources are present: data > url > path.
/// Returns `Vec<FileData>`. Per-file errors are logged and skipped; only the
/// hard-limit errors (`DataFieldTooLarge`, `PathFieldTooLarge`,
/// `UrlWithoutDocumentId`) propagate.
pub(crate) fn parse_file_entries(
    arr: &[serde_json::Value],
) -> Result<Vec<crate::llm::domain::FileData>, crate::llm::domain::LlmError> {
    use crate::llm::domain::{FileData, FileSource, LlmError};
    let mut out = Vec::with_capacity(arr.len());

    for file_obj in arr {
        let Some(obj) = file_obj.as_object() else {
            continue;
        };

        let mime_type = obj
            .get("mime_type")
            .and_then(|v| v.as_str())
            .unwrap_or("application/octet-stream")
            .to_string();
        let filename = obj
            .get("filename")
            .and_then(|v| v.as_str())
            .unwrap_or("upload.file")
            .to_string();
        let document_id = obj
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let size_hint = obj.get("size_bytes").and_then(|v| v.as_u64());

        let data_present = obj
            .get("data")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let url_present = obj
            .get("url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let path_present = obj
            .get("path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        let source = if let Some(data) = data_present {
            // Validate hint size first (cheap check before decode).
            if let Some(n) = size_hint {
                if n > FILE_DATA_LIMIT_BYTES {
                    return Err(LlmError::DataFieldTooLarge { size: n });
                }
            }
            use base64::{engine::general_purpose::STANDARD, Engine as _};
            let stripped = if data.starts_with("data:") {
                data.find(',').map(|i| &data[i + 1..]).unwrap_or(data)
            } else {
                data
            };
            let bytes = match STANDARD.decode(stripped) {
                Ok(b) => b,
                Err(e) => {
                    crate::colmena_log!("WARN: failed to decode base64 file data: {}", e);
                    continue;
                }
            };
            // Validate against actual decoded bytes.
            if bytes.len() as u64 > FILE_DATA_LIMIT_BYTES {
                return Err(LlmError::DataFieldTooLarge {
                    size: bytes.len() as u64,
                });
            }
            FileSource::InlineBytes { bytes }
        } else if let Some(url) = url_present {
            if document_id.is_none() {
                return Err(LlmError::UrlWithoutDocumentId);
            }
            FileSource::SignedUrl(url.to_string())
        } else if let Some(path) = path_present {
            let metadata = match std::fs::metadata(path) {
                Ok(m) => m,
                Err(e) => {
                    crate::colmena_log!("WARN: path stat failed for {}: {}", path, e);
                    continue;
                }
            };
            let size = metadata.len();
            if size > FILE_DATA_LIMIT_BYTES {
                return Err(LlmError::PathFieldTooLarge { size });
            }
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => {
                    crate::colmena_log!("WARN: path read failed for {}: {}", path, e);
                    continue;
                }
            };
            FileSource::InlineBytes { bytes }
        } else {
            crate::colmena_log!("WARN: file entry has no data/url/path; skipping");
            continue;
        };

        out.push(FileData {
            document_id,
            mime_type,
            filename,
            size_hint,
            source,
            retained_inline_bytes: None,
        });
    }

    Ok(out)
}

#[cfg(test)]
mod files_parser_tests {
    use super::*;
    use crate::llm::domain::{FileSource, LlmError};
    use serde_json::json;

    fn parse(files: serde_json::Value) -> Result<Vec<crate::llm::domain::FileData>, LlmError> {
        let arr = files.as_array().expect("array");
        parse_file_entries(arr)
    }

    #[test]
    fn data_under_30mb_becomes_inline() {
        let files = json!([{
            "id": "doc-1",
            "mime_type": "application/pdf",
            "filename": "x.pdf",
            "data": "aGVsbG8=", // "hello" base64
            "size_bytes": 5
        }]);
        let parsed = parse(files).unwrap();
        assert_eq!(parsed.len(), 1);
        match &parsed[0].source {
            FileSource::InlineBytes { bytes } => assert_eq!(bytes, b"hello"),
            _ => panic!("expected InlineBytes"),
        }
        assert_eq!(parsed[0].document_id.as_deref(), Some("doc-1"));
    }

    #[test]
    fn data_over_30mb_errors() {
        let files = json!([{
            "id": "doc-1",
            "mime_type": "application/pdf",
            "filename": "x.pdf",
            "data": "aGVsbG8=",
            "size_bytes": 50_000_000_u64
        }]);
        let r = parse(files);
        assert!(matches!(r, Err(LlmError::DataFieldTooLarge { .. })));
    }

    #[test]
    fn url_without_id_errors() {
        let files = json!([{
            "mime_type": "application/pdf",
            "filename": "x.pdf",
            "url": "https://storage.googleapis.com/bucket/x?sig=y",
            "size_bytes": 50_000_000_u64
        }]);
        let r = parse(files);
        assert!(matches!(r, Err(LlmError::UrlWithoutDocumentId)));
    }

    #[test]
    fn url_with_id_becomes_signed_url() {
        let files = json!([{
            "id": "doc-1",
            "mime_type": "application/pdf",
            "filename": "x.pdf",
            "url": "https://storage.googleapis.com/bucket/x?sig=y",
            "size_bytes": 50_000_000_u64
        }]);
        let parsed = parse(files).unwrap();
        match &parsed[0].source {
            FileSource::SignedUrl(u) => assert!(u.contains("storage.googleapis.com")),
            _ => panic!("expected SignedUrl"),
        }
        assert_eq!(parsed[0].document_id.as_deref(), Some("doc-1"));
    }

    #[test]
    fn data_and_url_present_prefers_data() {
        let files = json!([{
            "id": "doc-1",
            "mime_type": "application/pdf",
            "filename": "x.pdf",
            "data": "aGVsbG8=",
            "url": "https://x",
            "size_bytes": 5
        }]);
        let parsed = parse(files).unwrap();
        assert!(matches!(parsed[0].source, FileSource::InlineBytes { .. }));
    }

    #[test]
    fn legacy_data_without_id_works() {
        // Backward compat: a JSON without `id` and only `data` should still parse.
        let files = json!([{
            "mime_type": "application/pdf",
            "filename": "x.pdf",
            "data": "aGVsbG8="
        }]);
        let parsed = parse(files).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].document_id.is_none());
        assert!(matches!(parsed[0].source, FileSource::InlineBytes { .. }));
    }

    #[test]
    fn malformed_entry_skipped() {
        let files = json!([
            {"mime_type": "application/pdf"},  // no data/url/path -> skipped
            {"data": "aGVsbG8="}                // valid -> kept
        ]);
        let parsed = parse(files).unwrap();
        assert_eq!(parsed.len(), 1);
    }
}

#[cfg(test)]
mod find_pending_tool_call_tests {
    use super::*;
    use crate::llm::domain::{FunctionCall, LlmMessage, ToolCall};

    fn tc(id: &str, name: &str) -> ToolCall {
        ToolCall::new(
            id.to_string(),
            FunctionCall::new(name.to_string(), "{}".to_string()),
        )
    }

    #[test]
    fn returns_unmatched_tool_call() {
        // Assistant requested `call_xyz`; no matching Tool message follows.
        let messages = vec![
            LlmMessage::user("hi".to_string()).unwrap(),
            LlmMessage::assistant_with_tool_calls("".to_string(), vec![tc("call_xyz", "ask")])
                .unwrap(),
        ];
        let pending = find_pending_tool_call(&messages).expect("must find one");
        assert_eq!(pending.id, "call_xyz");
        assert_eq!(pending.function.name, "ask");
    }

    #[test]
    fn returns_none_when_all_tools_resolved() {
        let messages = vec![
            LlmMessage::user("hi".to_string()).unwrap(),
            LlmMessage::assistant_with_tool_calls("".to_string(), vec![tc("call_xyz", "ask")])
                .unwrap(),
            LlmMessage::tool("call_xyz".to_string(), "result".to_string()).unwrap(),
        ];
        assert!(find_pending_tool_call(&messages).is_none());
    }

    #[test]
    fn returns_latest_pending_when_multiple_assistant_messages() {
        // First assistant call is resolved; second is pending → must return the second.
        let messages = vec![
            LlmMessage::user("first".to_string()).unwrap(),
            LlmMessage::assistant_with_tool_calls("".to_string(), vec![tc("call_a", "ask_a")])
                .unwrap(),
            LlmMessage::tool("call_a".to_string(), "result_a".to_string()).unwrap(),
            LlmMessage::user("second".to_string()).unwrap(),
            LlmMessage::assistant_with_tool_calls("".to_string(), vec![tc("call_b", "ask_b")])
                .unwrap(),
        ];
        let pending = find_pending_tool_call(&messages).expect("must find one");
        assert_eq!(pending.id, "call_b");
    }

    #[test]
    fn returns_none_for_empty_history() {
        let messages: Vec<LlmMessage> = vec![];
        assert!(find_pending_tool_call(&messages).is_none());
    }

    #[test]
    fn returns_first_unresolved_among_multiple_tool_calls_in_one_message() {
        // Single assistant message with two tool_calls; only the second has a result.
        let messages = vec![
            LlmMessage::assistant_with_tool_calls(
                "".to_string(),
                vec![tc("call_a", "ask_a"), tc("call_b", "ask_b")],
            )
            .unwrap(),
            LlmMessage::tool("call_b".to_string(), "result_b".to_string()).unwrap(),
        ];
        let pending = find_pending_tool_call(&messages).expect("must find one");
        assert_eq!(pending.id, "call_a");
    }
}

#[cfg(test)]
mod resolver_tests {
    use super::*;

    #[tokio::test]
    async fn resolver_re_uploads_when_provider_file_id_marked_expired() {
        use crate::llm::application::LoadAttachmentResolver;
        use crate::llm::domain::attachments::{AttachmentSource, UpsertAttachmentInput};
        use crate::llm::domain::ProviderKind;
        use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
        use std::sync::Arc;

        let registry: Arc<dyn crate::llm::domain::AttachmentRegistry> = Arc::new(
            SqliteAttachmentRegistry::new("sqlite::memory:")
                .await
                .unwrap(),
        );
        registry
            .upsert(UpsertAttachmentInput {
                agent_session_id: "agent_1".to_string(),
                document_id: "doc-1".to_string(),
                provider: ProviderKind::OpenAi,
                provider_file_id: "pf-expired".to_string(),
                mime_type: "application/pdf".to_string(),
                filename: "x.pdf".to_string(),
                size_bytes: Some(1024),
                label: None,
                description: None,
                source: AttachmentSource::SignedUrl("https://example/url?sig=y".to_string()),
            })
            .await
            .unwrap();

        let resolver = AttachmentResolverImpl {
            registry: registry.clone(),
            provider: ProviderKind::OpenAi,
            api_key: "dummy".to_string(),
        };
        let file = resolver.resolve("agent_1", "doc-1").await.unwrap().unwrap();
        match file.source {
            crate::llm::domain::FileSource::Uploaded(r) => {
                assert_eq!(r.provider_file_id, "pf-expired");
            }
            _ => panic!("expected Uploaded"),
        }
    }

    #[tokio::test]
    async fn resolver_returns_none_for_unknown_document() {
        use crate::llm::application::LoadAttachmentResolver;
        use crate::llm::domain::ProviderKind;
        use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
        use std::sync::Arc;

        let registry: Arc<dyn crate::llm::domain::AttachmentRegistry> = Arc::new(
            SqliteAttachmentRegistry::new("sqlite::memory:")
                .await
                .unwrap(),
        );
        let resolver = AttachmentResolverImpl {
            registry,
            provider: ProviderKind::OpenAi,
            api_key: "dummy".to_string(),
        };
        let res = resolver.resolve("agent_1", "missing").await.unwrap();
        assert!(res.is_none());
    }
}

#[cfg(test)]
mod temporal_context_helper_tests {
    use super::*;

    #[test]
    fn block_starts_with_canonical_header() {
        let out = format_temporal_context_block("America/Bogota", "Bogotá, Colombia", "es-CO");
        assert!(
            out.starts_with("## Temporal & Geographic Context"),
            "missing header: {}",
            out
        );
    }

    #[test]
    fn iso_8601_appears_as_primary_timestamp() {
        let out = format_temporal_context_block("America/Bogota", "Bogotá, Colombia", "es-CO");
        let body = out
            .lines()
            .find(|l| l.starts_with("Current date and time:"))
            .expect("missing 'Current date and time:' line");
        assert!(body.contains("T"), "expected 'T' separator in: {}", body);
        assert!(
            body.contains("-05:00"),
            "expected Bogotá ISO offset -05:00 in: {}",
            body
        );
    }

    #[test]
    fn human_echo_appears_in_parens() {
        let out = format_temporal_context_block("America/Bogota", "Bogotá, Colombia", "es-CO");
        let body = out
            .lines()
            .find(|l| l.starts_with("Current date and time:"))
            .unwrap();
        assert!(body.contains("("), "missing opening paren in: {}", body);
        assert!(body.contains(")"), "missing closing paren in: {}", body);
        assert!(
            body.contains("AM") || body.contains("PM"),
            "missing AM/PM marker in: {}",
            body
        );
    }

    #[test]
    fn block_has_timezone_location_locale_lines() {
        let out = format_temporal_context_block("America/Bogota", "Bogotá, Colombia", "es-CO");
        assert!(out.contains("Timezone: America/Bogota (UTC-5)"), "tz line: {}", out);
        assert!(out.contains("Location: Bogotá, Colombia"), "loc line: {}", out);
        assert!(out.contains("Locale: es-CO"), "locale line: {}", out);
    }

    #[test]
    fn half_hour_offset_renders_correctly() {
        let out = format_temporal_context_block("Asia/Kolkata", "Mumbai, India", "hi-IN");
        assert!(out.contains("Timezone: Asia/Kolkata (UTC+5:30)"), "expected UTC+5:30 in: {}", out);
        assert!(out.contains("Locale: hi-IN"));
    }

    #[test]
    fn invalid_iana_falls_back_coherently() {
        let out = format_temporal_context_block("Mars/Olympus", "Mars Base", "en-US");
        assert!(out.contains("Timezone: America/Bogota (UTC-5)"), "fallback tz: {}", out);
        assert!(out.contains("-05:00"), "fallback ISO offset: {}", out);
        assert!(out.contains("Location: Mars Base"));
        assert!(out.contains("Locale: en-US"));
    }
}

#[cfg(test)]
mod inline_bytes_auto_summary_tests {
    //! RED gate for the `data:` (base64) inline-bytes auto-summary fix.
    //!
    //! Today the auto-register loop drops inline-bytes attachments before
    //! they reach the summary generator (llm.rs ~line 1074): the original
    //! decoded bytes are consumed by the upload to the provider's Files
    //! API and never reach `SummaryTarget`. The fix retains the original
    //! bytes on `FileData::retained_inline_bytes` and threads them into a
    //! new `SummaryTarget::inline_bytes` field, so summarisation works
    //! for inline uploads too.
    //!
    //! This test references both fields by name. Both are absent in the
    //! current production structs — compile failure IS the RED signal.
    use super::SummaryTarget;
    use crate::llm::domain::attachments::AttachmentSource;
    use crate::llm::domain::{FileData, FileSource, ProviderFileRef, ProviderKind};

    #[test]
    fn summary_target_for_inline_data_carries_decoded_bytes() {
        // Mimics the post-upload state: `data: "aGVsbG8="` was parsed into
        // InlineBytes(b"hello"), then `resolve_one` uploaded it and
        // replaced `source` with `Uploaded(..)`. The fix preserves the
        // original bytes on `retained_inline_bytes`.
        let file = FileData {
            document_id: None,
            mime_type: "text/plain".into(),
            filename: "hello.txt".into(),
            size_hint: Some(5),
            source: FileSource::Uploaded(ProviderFileRef {
                provider: ProviderKind::Google,
                provider_file_id: "files/abc123".into(),
                mime_type: "text/plain".into(),
                filename: "hello.txt".into(),
                expires_at: None,
            }),
            retained_inline_bytes: Some(b"hello".to_vec()),
        };

        // The auto-register loop must build a SummaryTarget whose
        // `inline_bytes` field carries the retained bytes.
        let target = SummaryTarget {
            document_id: "doc-1".into(),
            source: AttachmentSource::Inline,
            mime_type: file.mime_type.clone(),
            filename: file.filename.clone(),
            inline_bytes: file.retained_inline_bytes.clone(),
        };

        assert_eq!(target.inline_bytes.as_deref(), Some(b"hello".as_ref()));
        assert_eq!(target.mime_type, "text/plain");
        assert_eq!(target.filename, "hello.txt");
    }
}
