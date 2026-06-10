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

use crate::crdt_documents::{ArtifactId, CrdtDocumentsRuntime};
use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    build_all_crdt_doc_tools, build_all_document_tools, build_describe_tool_definition,
    build_load_skill_tool_definition, reconstruct_discovered_set, summary_for_catalog,
    CatalogEntry, CrdtDocsContext, DescribeToolDispatchResult, DocumentToolsContext,
    ATTACHMENTS_SYSTEM_PRELUDE, DOCUMENTS_SYSTEM_PRELUDE,
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
const LLM_DEFAULT_SYSTEM: &str = include_str!("../../../../text/prompts/llm_default_system.md");

/// Filter the catalog of available tools down to the set the LLM should see.
///
/// Two inputs drive the decision:
///   - `configured_aliases` — names declared via `tool_configurations`; each is
///     auto-enabled. For toolkit-style nodes, the catalog already contains the
///     expanded `{alias}__{sub_tool}` entries, so the prefix-match below picks
///     them up automatically.
///   - `enabled_tools_config` — optional allow-list at `inputs.enabled_tools`
///     or `config.enabled_tools`. Accepts:
///       * `"*"` wildcard → expose every available tool
///       * a string → enable a single named alias
///       * an array of strings → enable each named alias
pub(crate) fn filter_enabled_tools(
    all_tools: Vec<crate::llm::domain::ToolDefinition>,
    enabled_tools_config: Option<&Value>,
    configured_aliases: &std::collections::HashSet<String>,
) -> Vec<crate::llm::domain::ToolDefinition> {
    use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::find_package;

    // PASS 1 — parse user input into raw_includes, raw_excludes, wildcard.
    // `configured_aliases` (from tool_configurations) are seeded into
    // raw_includes so they are auto-enabled without needing to appear in
    // `enabled_tools`.
    let mut raw_includes: Vec<String> = configured_aliases.iter().cloned().collect();
    let mut raw_excludes: Vec<String> = Vec::new();
    let mut wildcard_all = false;

    let parse_entry = |s: &str,
                       raw_includes: &mut Vec<String>,
                       raw_excludes: &mut Vec<String>,
                       wildcard_all: &mut bool| {
        if s == "*" {
            *wildcard_all = true;
        } else if let Some(stripped) = s.strip_prefix('!') {
            if stripped.is_empty() {
                eprintln!("filter_enabled_tools: empty exclusion entry '!' ignored");
            } else {
                raw_excludes.push(stripped.to_string());
            }
        } else if !raw_includes.iter().any(|n| n == s) {
            raw_includes.push(s.to_string());
        }
    };

    if let Some(enabled) = enabled_tools_config {
        if let Some(arr) = enabled.as_array() {
            for v in arr {
                if let Some(s) = v.as_str() {
                    parse_entry(s, &mut raw_includes, &mut raw_excludes, &mut wildcard_all);
                }
            }
        } else if let Some(s) = enabled.as_str() {
            parse_entry(s, &mut raw_includes, &mut raw_excludes, &mut wildcard_all);
        }
    }

    // PASS 2 — expand package aliases on both sides.
    //
    // Each entry in raw_includes / raw_excludes is checked against the
    // TOOLKIT_PACKAGES registry. If it's a known package, it expands to the
    // package's tool list; otherwise it's kept as-is (exact name or
    // `{alias}__` toolkit prefix — see back-compat note below).
    let expand = |name: &str| -> Vec<String> {
        if let Some(pkg) = find_package(name) {
            pkg.tools.iter().map(|t| t.to_string()).collect()
        } else {
            vec![name.to_string()]
        }
    };

    let mut final_includes: std::collections::HashSet<String> = std::collections::HashSet::new();
    for n in &raw_includes {
        for expanded in expand(n) {
            final_includes.insert(expanded);
        }
    }

    let mut final_excludes: std::collections::HashSet<String> = std::collections::HashSet::new();
    for n in &raw_excludes {
        for expanded in expand(n) {
            final_excludes.insert(expanded);
        }
    }

    // Back-compat: include any tool whose name matches `{alias}__` for any
    // alias in raw_includes (covers api_explorer-style toolkits that use
    // the double-underscore prefix convention instead of TOOLKIT_PACKAGES).
    for alias in &raw_includes {
        let prefix = format!("{}__", alias);
        for tool in &all_tools {
            if tool.name.starts_with(&prefix) {
                final_includes.insert(tool.name.clone());
            }
        }
    }

    // PASS 3 — filter: apply set-difference (includes - excludes).
    // Wildcard short-circuits the includes check but exclusions still apply.
    all_tools
        .into_iter()
        .filter(|t| {
            if final_excludes.contains(&t.name) {
                return false;
            }
            if wildcard_all {
                return true;
            }
            final_includes.contains(&t.name)
        })
        .collect()
}

/// Resolve an `enabled_tools` config into `(includes, excludes)` for a
/// closed bundle of synthetic tools (gsheets / gdocs).
///
/// Unlike `filter_enabled_tools` (which operates over the executor catalog),
/// synthetic-tool blocks build their tool set inline AFTER the executor
/// catalog has been filtered. They need the same semantics — toolkit-package
/// alias expansion, `"*"` wildcard, and `!entry` exclusion — but applied to
/// a hard-coded list of known names.
///
/// `all_known` is the universe of tool names exposed by the synthetic block.
/// Returned sets contain `&'static str` slices borrowed from `all_known`.
///
/// Entries that don't match `"*"`, a toolkit-package alias, or any name in
/// `all_known` are silently ignored — they may belong to a different
/// synthetic block (e.g. a `gdocs_*` tool listed alongside `gsheets`).
pub(crate) fn resolve_synthetic_enabled_tools<'a>(
    enabled_tools_config: Option<&Value>,
    all_known: &'a [&'a str],
) -> (
    std::collections::HashSet<&'a str>,
    std::collections::HashSet<&'a str>,
) {
    use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::find_package;

    let mut wants: std::collections::HashSet<&'a str> = std::collections::HashSet::new();
    let mut excludes: std::collections::HashSet<&'a str> = std::collections::HashSet::new();

    let absorb = |s: &str, target: &mut std::collections::HashSet<&'a str>| {
        if s == "*" {
            for t in all_known {
                target.insert(*t);
            }
        } else if let Some(pkg) = find_package(s) {
            for t in pkg.tools {
                if let Some(matched) = all_known.iter().find(|n| **n == *t).copied() {
                    target.insert(matched);
                }
            }
        } else if let Some(matched) = all_known.iter().find(|n| **n == s).copied() {
            target.insert(matched);
        }
    };

    let mut absorb_one = |s: &str| {
        if let Some(stripped) = s.strip_prefix('!') {
            if !stripped.is_empty() {
                absorb(stripped, &mut excludes);
            }
        } else {
            absorb(s, &mut wants);
        }
    };

    match enabled_tools_config {
        Some(Value::String(s)) => absorb_one(s.as_str()),
        Some(Value::Array(arr)) => {
            for v in arr {
                if let Some(s) = v.as_str() {
                    absorb_one(s);
                }
            }
        }
        _ => {}
    }

    (wants, excludes)
}

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

    // 1b. Short-circuit for tabular attachments (CSV / XLSX).
    //
    // Auto-summary option B (shipped 2026-06-10): instead of feeding the raw
    // CSV text through a cheap-tier LLM (slow + token cost) — or returning
    // `Skipped` for XLSX (no useful summary today) — build a structured
    // summary locally from the parser used by `sql_inspect_attachment`. The
    // LLM sees schema + sample rows + total row count in the catalog block
    // from turn 1, with zero LLM tokens spent on summarization.
    //
    // Falls through to the existing extract_text/LLM path for any other
    // mime (PDF, plain text, markdown, etc.).
    {
        use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools::build_tabular_summary;
        if let Some(structured) = build_tabular_summary(&target.mime_type, &target.filename, &bytes)
        {
            return SummaryOutcome::Generated(structured);
        }
    }

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
    /// Optional storage adapter — used by the AttachmentResolver to lazy-upload
    /// `provider: Generated` artifacts to a chat provider's Files API on first
    /// `load_attachment` call.
    storage: Option<Arc<dyn crate::storage::domain::OutputStorageRepository>>,
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
            storage: None,
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

    /// Builder: attach the OutputStorageRepository so the AttachmentResolver
    /// can read bytes for `provider: Generated` rows when doing cross-provider
    /// lazy upload.
    pub fn with_storage(
        mut self,
        storage: Arc<dyn crate::storage::domain::OutputStorageRepository>,
    ) -> Self {
        self.storage = Some(storage);
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

    /// Resolve the full list of skill names visible to this LLM call by unioning
    /// the explicit `skills` array with all SKILL.md directories found under
    /// `skills_path` / `skills_paths`. Deduped by name.
    ///
    /// - `skills: [...]` — plain array of skill names (existing explicit form)
    /// - `skills_path: "<dir>"` — scan that directory; each immediate subdir
    ///   containing a `SKILL.md` contributes its directory name as a skill name
    /// - `skills_paths: [...]` — same semantics for multiple directories
    ///
    /// Missing `skills_path` directory → hard error.
    /// Empty directory (no SKILL.md subdirs) → contributes nothing, no error.
    pub async fn resolve_skill_names(config: &Value) -> Result<Vec<String>, String> {
        // Read explicit `skills` array (flat list of names).
        let explicit: Vec<String> = config
            .get("skills")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let mut all = std::collections::BTreeSet::<String>::new();
        for name in explicit {
            all.insert(name);
        }

        // Collect all directory paths from skills_path + skills_paths.
        let mut paths: Vec<String> = config
            .get("skills_paths")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(single) = config.get("skills_path").and_then(|v| v.as_str()) {
            paths.push(single.to_string());
        }

        for path in paths {
            let names = list_skills_in_path(&path).await?;
            for name in names {
                all.insert(name);
            }
        }

        Ok(all.into_iter().collect())
    }

    /// Build a SkillRepository from the parsed config. Returns `None` if no skills are configured.
    /// Returns `Err(String)` on any validation failure — this must abort graph execution.
    fn build_skill_repository_from_config(
        config: &Value,
        inputs: &NodeInputs,
    ) -> Result<Option<Arc<dyn SkillRepository>>, String> {
        let raw_val = inputs.get("skills").or_else(|| config.get("skills"));

        let mut skills_config = match raw_val {
            Some(v) => SkillsConfig::from_value(v)
                .map_err(|e| format!("invalid 'skills' config: {}", e))?,
            None => SkillsConfig::default(),
        };

        // Expand skills_path / skills_paths into individual skill-dir paths.
        // Each is a parent directory; every immediate subdir containing SKILL.md
        // becomes an additional entry in skills_config.paths.
        {
            let mut extra_roots: Vec<String> = config
                .get("skills_paths")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(single) = config.get("skills_path").and_then(|v| v.as_str()) {
                extra_roots.push(single.to_string());
            }
            for root in &extra_roots {
                let skill_dirs = list_skill_dirs_sync(root)
                    .map_err(|e| format!("skills_path '{}' not readable: {}", root, e))?;
                for dir in skill_dirs {
                    skills_config.paths.push(dir);
                }
            }
        }

        // If there's nothing to load (no builtins, no paths), short-circuit.
        if skills_config.builtin.is_empty() && skills_config.paths.is_empty() {
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
    /// Storage adapter for `ProviderKind::Generated` rows. When the LLM calls
    /// `load_attachment` on a generated artifact for the first time from a
    /// given provider, we read bytes via this and upload them to that
    /// provider's Files API. None disables cross-provider lazy upload (only
    /// pre-resolved rows work).
    storage: Option<std::sync::Arc<dyn crate::storage::domain::OutputStorageRepository>>,
}

#[async_trait::async_trait]
impl crate::llm::application::LoadAttachmentResolver for AttachmentResolverImpl {
    async fn resolve(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<Option<crate::llm::domain::FileData>, String> {
        use crate::llm::domain::{
            attachments::UpsertAttachmentInput, AttachmentSource, FileData, FileSource,
            ProviderFileRef, ProviderKind,
        };

        let row = self
            .registry
            .lookup(agent_session_id, document_id, self.provider.clone())
            .await
            .map_err(|e| e.to_string())?;
        let att = match row {
            Some(a) => a,
            None => {
                // Fallback: maybe it's a Generated artifact that hasn't been
                // uploaded to this provider yet. Lazy cross-provider upload.
                let gen_row = self
                    .registry
                    .lookup(agent_session_id, document_id, ProviderKind::Generated)
                    .await
                    .map_err(|e| e.to_string())?;
                let Some(gen) = gen_row else {
                    return Ok(None);
                };
                let storage = self.storage.as_ref().ok_or_else(|| {
                    "load_attachment: generated artifact present but no OutputStorageRepository \
                     is wired — cannot resolve bytes for cross-provider upload"
                        .to_string()
                })?;

                tracing::info!(
                    target: "colmena::attachment",
                    event = "attachment.cross_provider_lazy_upload",
                    agent_session_id = %agent_session_id,
                    document_id = %document_id,
                    target_provider = %self.provider,
                    storage_key = %gen.provider_file_id,
                    "generated artifact has no row for current provider, uploading on demand"
                );

                let bytes = storage.read(&gen.provider_file_id).await.map_err(|e| {
                    format!(
                        "storage read for storage_key '{}': {e}",
                        gen.provider_file_id
                    )
                })?;

                let file_provider = crate::llm::infrastructure::files::FileProviderFactory::create(
                    self.provider.clone(),
                    self.api_key.clone(),
                )
                .map_err(|e| e.to_string())?;
                let stream = futures::stream::once(async move {
                    Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(bytes.bytes))
                });
                let provider_ref = file_provider
                    .upload_streaming(Box::pin(stream), &gen.mime_type, &gen.filename)
                    .await
                    .map_err(|e| e.to_string())?;

                // Persist the resolved row so subsequent load_attachment calls
                // from this provider hit the fast path.
                let upsert = UpsertAttachmentInput {
                    agent_session_id: agent_session_id.to_string(),
                    document_id: document_id.to_string(),
                    provider: self.provider.clone(),
                    provider_file_id: provider_ref.provider_file_id.clone(),
                    mime_type: gen.mime_type.clone(),
                    filename: gen.filename.clone(),
                    size_bytes: gen.size_bytes,
                    label: gen.label.clone(),
                    description: gen.description.clone(),
                    // Keep the original source URL — it's how we'd re-fetch
                    // bytes if THIS provider's file_id ever expires.
                    source: AttachmentSource::SignedUrl(match gen.source.value() {
                        Some(u) => u.to_string(),
                        None => String::new(),
                    }),
                    storage_key: None,
                    origin: None,
                };
                if let Err(e) = self.registry.upsert(upsert).await {
                    tracing::warn!(
                        target: "colmena::attachment",
                        error = %e,
                        "failed to persist cross-provider upload row — \
                         will re-upload next time"
                    );
                }

                crate::llm::domain::ConversationAttachment {
                    agent_session_id: agent_session_id.to_string(),
                    document_id: document_id.to_string(),
                    provider: self.provider.clone(),
                    provider_file_id: provider_ref.provider_file_id,
                    mime_type: gen.mime_type,
                    filename: gen.filename,
                    size_bytes: gen.size_bytes,
                    label: gen.label,
                    description: gen.description,
                    source: gen.source,
                    registered_at: gen.registered_at,
                    refreshed_at: chrono::Utc::now(),
                    storage_key: None,
                    origin: None,
                    last_used_at: None,
                }
            }
        };

        // D10: a successful `load_attachment` resolution counts as "using"
        // the attachment. Touch `last_used_at` so the GC's
        // `COALESCE(last_used_at, registered_at) < cutoff` staleness check
        // treats actively-read attachments as fresh — otherwise a doc read
        // via load_attachment but never forwarded would be reaped TTL days
        // after registration. Best-effort and non-fatal, mirroring
        // AttachmentStreamResolverImpl on the Plan A forward path.
        if let Err(e) = self
            .registry
            .touch_last_used(agent_session_id, document_id)
            .await
        {
            tracing::warn!(
                target: "colmena::attachment",
                error = %e,
                agent_session_id = %agent_session_id,
                document_id = %document_id,
                "touch_last_used failed in load_attachment (non-fatal)"
            );
        }

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
            .or_else(|| {
                config
                    .get("summary_max_output_chars")
                    .and_then(|v| v.as_u64())
            })
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

                // Plan A — Foundation: persist bytes uniformly to
                // OutputStorageRepository so this attachment is reachable via
                // `$attachment:<document_id>` downstream, regardless of where
                // it originated. Inline files reuse `retained_inline_bytes`;
                // SignedUrl files are re-fetched (acceptable for Plan A).
                // TODO(plan-a-opt): share bytes with provider upload to avoid re-fetch.
                let storage_key = if let Some(storage) = self.storage.as_ref() {
                    persist_attachment_bytes(
                        storage.as_ref(),
                        file.retained_inline_bytes.as_deref(),
                        &source,
                        &file.mime_type,
                        &file.filename,
                        sid.as_str(),
                        &document_id,
                    )
                    .await
                } else {
                    None
                };

                let origin = crate::llm::domain::attachments::origin::USER_UPLOAD.to_string();
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
                    storage_key,
                    origin: Some(origin),
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
                    let has_summarisable_source = !matches!(source, AttachmentSource::Inline)
                        || inline_bytes_for_summary.is_some();
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
        //
        // Plan B (D6): the LLM no longer receives file content in the initial
        // user message. The catalog block prepended to the system message
        // (Plan A Task 11) tells the model which documents are available; the
        // model calls load_attachment(document_id) to read content, or
        // references "$attachment:<document_id>" in tool args to forward
        // bytes without reading them. `resolved_files` is intentionally still
        // computed — bytes are persisted to OutputStorageRepository and
        // registered in the attachment catalog further upstream.
        if resume_answer.is_none() {
            let user_message = build_initial_user_message(prompt, &resolved_files)?;
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
        // LLM with no visible diagnostic, and the model would improvise tool
        // calls as plain text. Hard-fail with a pedagogical message so the
        // graph author sees the exact field that broke.
        let mut tool_configurations: HashMap<String, ToolConfiguration> = match inputs
            .get("tool_configurations")
            .or_else(|| config.get("tool_configurations"))
        {
            Some(v) => {
                match serde_json::from_value::<HashMap<String, ToolConfiguration>>(v.clone()) {
                    Ok(map) => map,
                    Err(e) => {
                        return Err(format!(
                            "llm_call: tool_configurations failed to parse: {e}.\n\
                             Hint: each entry needs `name` and `node_type`. Inside `node_schema`, \
                             every LLM-visible field needs `type` (string|number|integer|boolean|object|array). \
                             Fields with `fixed` may omit `type`. Fix the graph configuration and re-run."
                        )
                        .into());
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
            // NOTE: F-T14 step A3 expanded lazy's coverage to synthetic
            // crdt_doc_* tools, so a fully-empty catalog now only happens when
            // there are no tool_configurations AND no crdt_documents context.
            // We check for that case AFTER both sources have populated the
            // catalog (below, near the crdt_doc_* registration block).
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

        // Build crdt_documents context if the LLM node was configured with a
        // `crdt_documents` block. The five v1 synthetic tools are exposed and
        // dispatched through the runtime built here. artifact_id MUST be in the
        // config (LLM never sets it); session_id is not relevant for the v1 CRDT
        // tools since the registry is per-artifact, not per-session.
        //
        // For WsPeer mode, we also need to retain the peer handle so we can
        // shutdown the WS cleanly at end-of-execute (flush pending updates
        // before closing the socket). The context only holds `Arc<Doc>` +
        // `Arc<AtomicBool>` cloned from the peer; ownership of the peer
        // itself lives in this option.
        let mut crdt_ws_peer_for_shutdown: Option<crate::crdt_documents::WsPeerArtifact> = None;
        let crdt_docs_context: Option<Arc<CrdtDocsContext>> = match inputs
            .get("crdt_documents")
            .cloned()
            .or_else(|| config.get("crdt_documents").cloned())
        {
            Some(crdt_cfg) => {
                let artifact_id_str = crdt_cfg
                    .get("artifact_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| -> Box<dyn Error + Send + Sync> {
                        "crdt_documents config requires `artifact_id`".into()
                    })?;
                let artifact_id: ArtifactId = artifact_id_str.parse().map_err(|_| -> Box<dyn Error + Send + Sync> {
                    "crdt_documents config has invalid `artifact_id` (expected art_<26-char-ULID>)".into()
                })?;
                // Mode selection (descending priority):
                //
                // 1. `ws_url` present in config → WsPeer mode. The agent
                //    opens a WS peer connection to a remote CRDT documents
                //    service. The agent's worker is stateless; the
                //    service holds the authoritative Y.Doc.
                // 2. No `ws_url`, process-wide singleton installed (e.g.
                //    `crdt-yws-graph` subcommand, future ADP worker
                //    bootstrap that colocates server + executor) → Local
                //    mode using the singleton runtime. Mutations are
                //    visible live to any browser connected to that
                //    server because they share the same Arc<Doc>.
                // 3. Neither → Local mode with a freshly-built runtime
                //    (plain `dag_engine run`, autonomous CLI). No live
                //    server is involved; persistence is to disk only.
                if let Some(ws_url) = crdt_cfg.get("ws_url").and_then(Value::as_str) {
                    match crate::crdt_documents::WsPeerArtifact::connect(
                        ws_url,
                        artifact_id.clone(),
                        "agent",
                        agent_session_id_str.as_deref(),
                    )
                    .await
                    {
                        Ok(peer) => {
                            let http_base = ws_url_to_http_base(ws_url);
                            let ctx = CrdtDocsContext::new_ws_peer(
                                &peer,
                                agent_session_id_str.clone(),
                                http_base,
                            );
                            crdt_ws_peer_for_shutdown = Some(peer);
                            Some(Arc::new(ctx))
                        }
                        Err(e) => {
                            return Err(
                                format!("crdt_documents ws_peer connect failed: {e}").into()
                            );
                        }
                    }
                } else {
                    let runtime_arc = if let Some(shared) =
                        crate::crdt_documents::process_runtime::get_global()
                    {
                        shared
                    } else {
                        match CrdtDocumentsRuntime::from_config(&crdt_cfg).await {
                            Ok(rt) => Arc::new(rt),
                            Err(e) => {
                                return Err(format!(
                                    "invalid `crdt_documents` config on llm node: {e}"
                                )
                                .into());
                            }
                        }
                    };
                    Some(Arc::new(CrdtDocsContext::new_local(
                        runtime_arc,
                        artifact_id,
                        agent_session_id_str.clone(),
                    )))
                }
            }
            None => None,
        };

        // ---- Step 4 (catalog building) — must precede executor block ----------------
        // Include both rows for the current provider AND `Generated` rows
        // (outputs from image_generation/edit/tts). The latter are resolved
        // lazily by the AttachmentResolver: on first `load_attachment` call,
        // bytes are read via OutputStorageRepository, uploaded to the current
        // provider's Files API, and a sibling row is upserted.
        let attachment_catalog: Vec<crate::llm::domain::ConversationAttachment> =
            if attachments_enabled {
                if let (Some(reg), Some(sid)) =
                    (attachment_registry.as_ref(), agent_session_id_str.as_ref())
                {
                    let all = reg
                        .list_for_session(sid)
                        .await
                        .map_err(|e| format!("attachment list: {}", e))?;
                    let mut by_doc: std::collections::HashMap<
                        String,
                        crate::llm::domain::ConversationAttachment,
                    > = std::collections::HashMap::new();
                    for a in all.into_iter().filter(|a| {
                        // Keep: rows for the current provider, synthetic
                        // `Generated` rows (image/tts artifacts), and ALL user
                        // uploads regardless of which provider they were first
                        // registered under. The last clause ensures a document
                        // uploaded in a turn that used provider X stays visible
                        // (and injectable via `$attachment`) in a later turn
                        // that uses provider Y — the catalog must span the whole
                        // agent_session_id, not just same-provider turns.
                        a.provider == provider_kind
                            || a.provider == crate::llm::domain::ProviderKind::Generated
                            || a.origin.as_deref()
                                == Some(crate::llm::domain::attachments::origin::USER_UPLOAD)
                    }) {
                        // Prefer provider-specific row over the synthetic
                        // Generated row when both exist (= cross-provider
                        // lazy upload has already run for this artifact).
                        match by_doc.get(&a.document_id) {
                            Some(existing)
                                if existing.provider == provider_kind
                                    && a.provider
                                        == crate::llm::domain::ProviderKind::Generated =>
                            {
                                // Keep existing provider-specific entry.
                            }
                            _ => {
                                by_doc.insert(a.document_id.clone(), a);
                            }
                        }
                    }
                    by_doc.into_values().collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

        // F-T15 — resolve the ConversationRepository EARLIER (it's used both
        // here for the executor's recall_history wiring AND later by
        // AgentService for its history operations). Falls back to an in-memory
        // repo when the operator hasn't configured persistent memory; either
        // way recall_history works for the duration of the run.
        let conversation_repo: Arc<dyn crate::llm::domain::ConversationRepository> =
            match repo_instance.clone() {
                Some(repo) => repo,
                None => {
                    use crate::llm::infrastructure::persistence::in_memory_conversation_repository::InMemoryConversationRepository;
                    Arc::new(InMemoryConversationRepository::new())
                }
            };

        let tool_executor = {
            let mut executor = DagToolExecutor::new(registry, tool_configurations);
            executor = executor
                .with_conversation_history(conversation_repo.clone(), conversation_key.clone());
            // Per-llm_call override of the tool-result string cap. Inputs win
            // over config so a graph can dynamically widen the cap when it
            // expects a large legitimate payload (e.g. a long document body).
            let max_tool_result_bytes = inputs
                .get("max_tool_result_bytes")
                .and_then(|v| v.as_u64())
                .or_else(|| config.get("max_tool_result_bytes").and_then(|v| v.as_u64()))
                .map(|v| v as usize);
            if let Some(cap) = max_tool_result_bytes {
                executor = executor.with_max_tool_result_bytes(cap);
            }
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
            if let Some(ctx) = crdt_docs_context.clone() {
                executor = executor.with_crdt_documents(ctx);
            }
            // ---- Step 5: Wire attachment catalog into executor ----------------------
            if !attachment_catalog.is_empty() {
                executor = executor.with_attachments(attachment_catalog.clone());
            }
            // Bulk T0 (2026-06-09): also thread the OutputStorageRepository so
            // dispatchers that need attachment bytes can call
            // `fetch_attachment_bytes` / `fetch_attachment_stream` /
            // `register_attachment_bytes` from a single shared source of truth.
            // Unblocks sql_bulk_insert_from_attachment + E-T7b + G items 4/5/8.
            if let Some(storage) = self.storage.clone() {
                executor = executor.with_attachment_storage(storage);
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

        // `conversation_repo` was resolved earlier (~line 1753) so the
        // tool_executor could wire it for the recall_history dispatch. Reuse
        // the same Arc here for AgentService — both must read/write to the
        // same backing store.
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
            let maybe_pending = find_pending_tool_call(&conversation.messages);
            if let Some(pending) = maybe_pending {
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
                    if parsed.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED")
                    {
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
            } else {
                // Defense-in-depth: if the engine's per-node gating
                // (run_use_case.rs §4.1) is broken and we received
                // __colmena_resume_answer despite having no pending tool
                // call, fall through to the fresh-run path instead of
                // aborting the DAG.
                //
                // Spec: docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md §4.2.1
                let node_name = inputs
                    .get("__node_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unknown)");
                tracing::warn!(
                    target: "colmena::llm_node",
                    node_id = node_name,
                    "llm_call: resume_answer present but no pending tool call in history; \
                     falling through to fresh run (engine routing may be broken)"
                );
                // Intentional fallthrough — control continues to the
                // standard agent_service.run path below.
            }
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

        let mut tools: Vec<crate::llm::domain::ToolDefinition> =
            filter_enabled_tools(all_tools, enabled_tools_config, &configured_aliases);

        if let Some(repo) = skill_repo.as_ref() {
            tools.push(build_load_skill_tool_definition(repo));
        }

        // ---- Step 4 (tool expose) — catalog already built above executor block ------
        if !attachment_catalog.is_empty() {
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::build_load_attachment_tool_definition;
            tools.push(build_load_attachment_tool_definition(&attachment_catalog));
        }

        // ---- Bulk T4: expose synthetic SQL bulk tools when the operator opted in
        // via tool_configurations. The dispatcher (DagToolExecutor) intercepts
        // calls by name; the tool definition here is what the LLM sees in the
        // tools[] array of the request. Same pattern as load_attachment but
        // gated on tool_configurations membership instead of an attachment catalog.
        //
        // Membership is checked against `configured_aliases` (snapshot taken at
        // line ~1618 before the map is moved into the executor) — the raw
        // `tool_configurations` map is no longer accessible at this point.
        {
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools::{
                build_sql_bulk_insert_tool_definition,
                build_sql_inspect_attachment_tool_definition, SQL_BULK_INSERT_TOOL_NAME,
                SQL_INSPECT_ATTACHMENT_TOOL_NAME,
            };
            if configured_aliases.contains(SQL_INSPECT_ATTACHMENT_TOOL_NAME) {
                tools.push(build_sql_inspect_attachment_tool_definition());
            }
            if configured_aliases.contains(SQL_BULK_INSERT_TOOL_NAME) {
                tools.push(build_sql_bulk_insert_tool_definition());
            }
            // attachment_run_python (post item 13, 2026-06-10) — opt-in by name.
            // No fixed_config required; the dispatcher just needs the shared
            // attachment plumbing (Bulk T0) which is wired automatically when
            // the LlmNode's `storage` is set.
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_run_python::{
                build_attachment_run_python_tool_definition, ATTACHMENT_RUN_PYTHON_TOOL_NAME,
            };
            if configured_aliases.contains(ATTACHMENT_RUN_PYTHON_TOOL_NAME) {
                tools.push(build_attachment_run_python_tool_definition());
            }
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

        // When the LLM node has a `crdt_documents` config, expose the synthetic
        // crdt_doc_* tools. The executor was already wired with the matching
        // CrdtDocsContext above so dispatches succeed.
        //
        // F-T14 step A3 — when `lazy_tool_loading: true` is also set, register
        // each crdt_doc_* tool as a CatalogEntry so the existing lazy mechanism
        // hides their full schemas until the agent calls describe_tool(name).
        // load_skill is always eager — it's the entry point for skill discovery
        // and small enough to carry every iteration.
        if crdt_docs_context.is_some() {
            for td in build_all_crdt_doc_tools() {
                if lazy_tool_loading {
                    catalog.push(CatalogEntry {
                        name: td.name.clone(),
                        summary: summary_for_catalog(td.summary.as_deref(), &td.description),
                    });
                }
                tools.push(td);
            }
        }

        // F-T15 — expose recall_history synthetic tool whenever the LLM node
        // has persisted memory (which it always does — repo_instance defaults
        // to InMemoryConversationRepository even without explicit memory
        // config). The executor wiring below pairs it with conversation_repo
        // + conversation_key so the dispatch can read the persisted history.
        // Always eager: it's small and complements the rolling-summary block.
        {
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::tool_recall_history;
            tools.push(tool_recall_history());
        }

        // E-T8 — expose the 9 synthetic Google Sheets tools (gsheets_*) when
        // their names appear in `enabled_tools` (or `enabled_tools: "*"`).
        // Unlike crdt_doc_* / document_* tools, gsheets has no per-node
        // context object — credentials are sourced from process-level env
        // (ADC or `GOOGLE_APPLICATION_CREDENTIALS`), so the only opt-in signal
        // is the user listing them under `enabled_tools`.
        //
        // ALL 9 tool DEFINITIONS are published (for schema discovery), even
        // though E-T7 only wired 7 dispatchers in `dag_tool_executor`. The
        // xlsx pair (`gsheets_create_from_xlsx`, `gsheets_export_xlsx`) will
        // surface a router-level error on invocation until E-T7b lands —
        // their schemas are still useful for the agent to plan against.
        //
        // Honors `lazy_tool_loading`: when enabled, each gsheets tool is
        // also registered as a `CatalogEntry` so its full schema stays hidden
        // until the agent calls `describe_tool(name)` — same pattern as the
        // crdt_doc_* block above.
        {
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
                gsheets_tool_add_sheet, gsheets_tool_create_from_xlsx,
                gsheets_tool_create_spreadsheet, gsheets_tool_delete_sheet,
                gsheets_tool_export_xlsx, gsheets_tool_list_sheets, gsheets_tool_read,
                gsheets_tool_run_python, gsheets_tool_set_cell, gsheets_tool_set_range,
                GSHEETS_ADD_SHEET_TOOL, GSHEETS_CREATE_FROM_XLSX_TOOL,
                GSHEETS_CREATE_SPREADSHEET_TOOL, GSHEETS_DELETE_SHEET_TOOL,
                GSHEETS_EXPORT_XLSX_TOOL, GSHEETS_LIST_SHEETS_TOOL, GSHEETS_READ_TOOL,
                GSHEETS_SET_CELL_TOOL, GSHEETS_SET_RANGE_TOOL, TOOL_GSHEETS_RUN_PYTHON,
            };

            let all_gsheets: [&str; 10] = [
                GSHEETS_CREATE_SPREADSHEET_TOOL,
                GSHEETS_CREATE_FROM_XLSX_TOOL,
                GSHEETS_EXPORT_XLSX_TOOL,
                GSHEETS_LIST_SHEETS_TOOL,
                GSHEETS_ADD_SHEET_TOOL,
                GSHEETS_DELETE_SHEET_TOOL,
                GSHEETS_READ_TOOL,
                GSHEETS_SET_CELL_TOOL,
                GSHEETS_SET_RANGE_TOOL,
                TOOL_GSHEETS_RUN_PYTHON,
            ];

            // Resolve `enabled_tools` → (wants, excludes). Supports `"*"`,
            // the `"gsheets"` toolkit-package alias, exact tool names, and
            // `"!<entry>"` exclusions on any of the above. Final set is
            // `wants - excludes`. See `resolve_synthetic_enabled_tools` for
            // the full semantics — kept consistent with `filter_enabled_tools`.
            let (wants, excludes) =
                resolve_synthetic_enabled_tools(enabled_tools_config, &all_gsheets);

            let gsheets_entries: [(&str, fn() -> crate::llm::domain::ToolDefinition); 10] = [
                (
                    GSHEETS_CREATE_SPREADSHEET_TOOL,
                    gsheets_tool_create_spreadsheet,
                ),
                (GSHEETS_CREATE_FROM_XLSX_TOOL, gsheets_tool_create_from_xlsx),
                (GSHEETS_EXPORT_XLSX_TOOL, gsheets_tool_export_xlsx),
                (GSHEETS_LIST_SHEETS_TOOL, gsheets_tool_list_sheets),
                (GSHEETS_ADD_SHEET_TOOL, gsheets_tool_add_sheet),
                (GSHEETS_DELETE_SHEET_TOOL, gsheets_tool_delete_sheet),
                (GSHEETS_READ_TOOL, gsheets_tool_read),
                (GSHEETS_SET_CELL_TOOL, gsheets_tool_set_cell),
                (GSHEETS_SET_RANGE_TOOL, gsheets_tool_set_range),
                (TOOL_GSHEETS_RUN_PYTHON, gsheets_tool_run_python),
            ];

            for (name, builder) in gsheets_entries {
                // Skip if the user did not opt in, was explicitly excluded,
                // OR if a `tool_configurations` entry / earlier
                // `filter_enabled_tools` pass already added it (dedup by
                // tool name keeps the catalog single-valued).
                if !wants.contains(name) || excludes.contains(name) {
                    continue;
                }
                if tools.iter().any(|t| t.name == name) {
                    continue;
                }
                let td = builder();
                if lazy_tool_loading {
                    catalog.push(CatalogEntry {
                        name: td.name.clone(),
                        summary: summary_for_catalog(td.summary.as_deref(), &td.description),
                    });
                }
                tools.push(td);
            }
        }

        // ---- gdocs synthetic tools — Subsystem G v1 (22 tools) ------------------
        //
        // Same pattern as the gsheets block above: builds the `wants` set
        // from `enabled_tools`, then pushes each opted-in tool definition
        // onto `tools`. Improvement over gsheets: also resolves the
        // `gdocs` / `gdocsread` toolkit-package aliases (via `find_package`)
        // so `enabled_tools: ["gdocs"]` works as flag-only activation.
        //
        // Honors `lazy_tool_loading` identically.
        {
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
                dispatch_gdocs_acknowledge_human_changes as _dispatch_unused,
                gdocs_tool_acknowledge_human_changes, gdocs_tool_add_tab,
                gdocs_tool_append_markdown, gdocs_tool_apply_edits, gdocs_tool_create,
                gdocs_tool_create_from_docx, gdocs_tool_create_from_markdown,
                gdocs_tool_create_named_range, gdocs_tool_delete_text, gdocs_tool_export,
                gdocs_tool_insert_after_text, gdocs_tool_insert_before_text,
                gdocs_tool_insert_between, gdocs_tool_list_named_ranges, gdocs_tool_list_tabs,
                gdocs_tool_read_as_markdown, gdocs_tool_read_outline,
                gdocs_tool_replace_named_range, gdocs_tool_replace_section,
                gdocs_tool_replace_text, gdocs_tool_share, gdocs_tool_style_text,
                GDOCS_ACKNOWLEDGE_HUMAN_CHANGES_TOOL, GDOCS_ADD_TAB_TOOL,
                GDOCS_APPEND_MARKDOWN_TOOL, GDOCS_APPLY_EDITS_TOOL, GDOCS_CREATE_FROM_DOCX_TOOL,
                GDOCS_CREATE_FROM_MARKDOWN_TOOL, GDOCS_CREATE_NAMED_RANGE_TOOL, GDOCS_CREATE_TOOL,
                GDOCS_DELETE_TEXT_TOOL, GDOCS_EXPORT_TOOL, GDOCS_INSERT_AFTER_TEXT_TOOL,
                GDOCS_INSERT_BEFORE_TEXT_TOOL, GDOCS_INSERT_BETWEEN_TOOL,
                GDOCS_LIST_NAMED_RANGES_TOOL, GDOCS_LIST_TABS_TOOL, GDOCS_READ_AS_MARKDOWN_TOOL,
                GDOCS_READ_OUTLINE_TOOL, GDOCS_REPLACE_NAMED_RANGE_TOOL,
                GDOCS_REPLACE_SECTION_TOOL, GDOCS_REPLACE_TEXT_TOOL, GDOCS_SHARE_TOOL,
                GDOCS_STYLE_TEXT_TOOL,
            };
            // Silence the unused-import lint — we only import the dispatch
            // symbol here so the compiler enforces the link in the
            // generated re-export block; it's not called from this file.
            let _ = _dispatch_unused;

            let all_gdocs: [&str; 22] = [
                GDOCS_CREATE_TOOL,
                GDOCS_CREATE_FROM_MARKDOWN_TOOL,
                GDOCS_CREATE_FROM_DOCX_TOOL,
                GDOCS_SHARE_TOOL,
                GDOCS_EXPORT_TOOL,
                GDOCS_LIST_TABS_TOOL,
                GDOCS_ADD_TAB_TOOL,
                GDOCS_READ_AS_MARKDOWN_TOOL,
                GDOCS_READ_OUTLINE_TOOL,
                GDOCS_LIST_NAMED_RANGES_TOOL,
                GDOCS_REPLACE_TEXT_TOOL,
                GDOCS_INSERT_AFTER_TEXT_TOOL,
                GDOCS_INSERT_BEFORE_TEXT_TOOL,
                GDOCS_INSERT_BETWEEN_TOOL,
                GDOCS_DELETE_TEXT_TOOL,
                GDOCS_REPLACE_SECTION_TOOL,
                GDOCS_APPEND_MARKDOWN_TOOL,
                GDOCS_APPLY_EDITS_TOOL,
                GDOCS_STYLE_TEXT_TOOL,
                GDOCS_CREATE_NAMED_RANGE_TOOL,
                GDOCS_REPLACE_NAMED_RANGE_TOOL,
                GDOCS_ACKNOWLEDGE_HUMAN_CHANGES_TOOL,
            ];

            // Resolve `enabled_tools` → (wants, excludes). Supports `"*"`,
            // `"gdocs"` / `"gdocsread"` toolkit-package aliases, exact tool
            // names, and `"!<entry>"` exclusions. See
            // `resolve_synthetic_enabled_tools` for the full semantics.
            let (wants, excludes) =
                resolve_synthetic_enabled_tools(enabled_tools_config, &all_gdocs);

            let gdocs_entries: [(&str, fn() -> crate::llm::domain::ToolDefinition); 22] = [
                (GDOCS_CREATE_TOOL, gdocs_tool_create),
                (
                    GDOCS_CREATE_FROM_MARKDOWN_TOOL,
                    gdocs_tool_create_from_markdown,
                ),
                (GDOCS_CREATE_FROM_DOCX_TOOL, gdocs_tool_create_from_docx),
                (GDOCS_SHARE_TOOL, gdocs_tool_share),
                (GDOCS_EXPORT_TOOL, gdocs_tool_export),
                (GDOCS_LIST_TABS_TOOL, gdocs_tool_list_tabs),
                (GDOCS_ADD_TAB_TOOL, gdocs_tool_add_tab),
                (GDOCS_READ_AS_MARKDOWN_TOOL, gdocs_tool_read_as_markdown),
                (GDOCS_READ_OUTLINE_TOOL, gdocs_tool_read_outline),
                (GDOCS_LIST_NAMED_RANGES_TOOL, gdocs_tool_list_named_ranges),
                (GDOCS_REPLACE_TEXT_TOOL, gdocs_tool_replace_text),
                (GDOCS_INSERT_AFTER_TEXT_TOOL, gdocs_tool_insert_after_text),
                (GDOCS_INSERT_BEFORE_TEXT_TOOL, gdocs_tool_insert_before_text),
                (GDOCS_INSERT_BETWEEN_TOOL, gdocs_tool_insert_between),
                (GDOCS_DELETE_TEXT_TOOL, gdocs_tool_delete_text),
                (GDOCS_REPLACE_SECTION_TOOL, gdocs_tool_replace_section),
                (GDOCS_APPEND_MARKDOWN_TOOL, gdocs_tool_append_markdown),
                (GDOCS_APPLY_EDITS_TOOL, gdocs_tool_apply_edits),
                (GDOCS_STYLE_TEXT_TOOL, gdocs_tool_style_text),
                (GDOCS_CREATE_NAMED_RANGE_TOOL, gdocs_tool_create_named_range),
                (
                    GDOCS_REPLACE_NAMED_RANGE_TOOL,
                    gdocs_tool_replace_named_range,
                ),
                (
                    GDOCS_ACKNOWLEDGE_HUMAN_CHANGES_TOOL,
                    gdocs_tool_acknowledge_human_changes,
                ),
            ];

            for (name, builder) in gdocs_entries {
                if !wants.contains(name) || excludes.contains(name) {
                    continue;
                }
                if tools.iter().any(|t| t.name == name) {
                    continue;
                }
                let td = builder();
                if lazy_tool_loading {
                    catalog.push(CatalogEntry {
                        name: td.name.clone(),
                        summary: summary_for_catalog(td.summary.as_deref(), &td.description),
                    });
                }
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
            // CRDT recent-changes auto-context. Append after the temporal
            // block so the order is: temporal → workbook-changes → user
            // instructions → tool rules. The helper returns `None` when
            // there is no session_id, no cursor delta, or no events.
            if let Some(ctx) = crdt_docs_context.as_ref() {
                use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
                    build_recent_changes_block, CRDT_SPREADSHEET_PROTOCOL_PRELUDE,
                };
                if let Some(block) = build_recent_changes_block(ctx.as_ref()).await {
                    sections.push(block);
                }
                // CRDT spreadsheet operating manual. Auto-injected so users
                // can speak naturally ("compará Q3 y Q4") without naming
                // tools or patterns. ~150 tokens fixed cost; pays back via
                // fewer iterations on naive prompts. Skills are still
                // loaded lazily-by-reference for the heavy detail.
                sections.push(CRDT_SPREADSHEET_PROTOCOL_PRELUDE.to_string());
                let _ = ctx; // already used above
            }
            if let Some(sys_msg) = system_message {
                sections.push(sys_msg.to_string());
            }
            if documents_context.is_some() {
                sections.push(DOCUMENTS_SYSTEM_PRELUDE.to_string());
            }
            if !attachment_catalog.is_empty() {
                sections.push(ATTACHMENTS_SYSTEM_PRELUDE.to_string());
                // Plan A: append the per-document catalog block so the LLM
                // knows which `document_id`s are available in the session
                // (for `load_attachment(...)` and `$attachment:<id>`
                // placeholder use). Plan B (D6): this catalog is now the
                // ONLY way the LLM learns about attachments in turn 1 —
                // file content is no longer autoinjected into the user
                // message; see `build_initial_user_message`.
                if let Some(catalog_block) =
                    crate::llm::application::attachment_catalog::render_catalog(&attachment_catalog)
                {
                    sections.push(catalog_block);
                }
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
                    // Tools list goes via tools[] JSON; we only nudge usage policy here.
                    // Trimmed from a 4-bullet ~600-char block to a single line for F-T14.
                    sections.push(format!(
                        "## Tools\nAvailable: {}.\nPrefer tools over guessing. Report errors clearly.",
                        tool_names.iter().map(|t| t.trim_start_matches("- ")).collect::<Vec<_>>().join(", ")
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
                        storage: self.storage.clone(),
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
                        storage: self.storage.clone(),
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
                Some(std::sync::Arc::new(LlmAttachmentSummaryGenerator::new(
                    repo,
                )))
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
                        let outcome =
                            generate_one_summary(&*gen, &cfg, &t, fetcher, summary_max_chars).await;
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

        // CRDT cleanup per mode:
        //   - Local + locally-owned runtime: drain snapshot writers so the
        //     last mutations land on disk before the tokio runtime tears
        //     down (writers are detached tokio::spawn tasks).
        //   - Local + singleton runtime: skip — the singleton is owned by
        //     the host process and must outlive this call.
        //   - WsPeer: flush pending outbound updates and close the socket
        //     cleanly. Without this, the last few CRDT updates queued in
        //     the channel might not reach the server before the host
        //     process exits.
        // Advance the agent's cursor for this artifact so the NEXT turn's
        // auto-summary block omits events we already saw during this turn.
        // `max_event_id_observed` is updated by every tool dispatcher after
        // `backend.record_event()`. We persist it via the same backend so
        // both Local and WsPeer modes work. Errors are deliberately
        // swallowed — failing to update the cursor means the next turn
        // re-shows old events, which is annoying but not fatal.
        if let Some(ctx) = crdt_docs_context.as_ref() {
            if let Some(sid) = ctx.session_id() {
                let max = ctx.max_event_id_observed();
                if max > 0 {
                    let _ = ctx
                        .backend()
                        .upsert_cursor(sid, ctx.artifact_id(), max)
                        .await;
                }
            }
        }
        if let Some(ctx) = crdt_docs_context.as_ref() {
            if let CrdtDocsContext::Local { runtime, .. } = ctx.as_ref() {
                let is_shared = crate::crdt_documents::process_runtime::get_global()
                    .as_ref()
                    .is_some_and(|shared| Arc::ptr_eq(shared, runtime));
                if !is_shared {
                    runtime.shutdown().await;
                }
            }
        }
        if let Some(mut peer) = crdt_ws_peer_for_shutdown.take() {
            peer.shutdown().await;
        }

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

// ---------------------------------------------------------------------------
// skills_path / skills_paths helpers
// ---------------------------------------------------------------------------

/// Scan `path` (a parent directory) and return the names of every immediate
/// subdirectory that contains a `SKILL.md` file.
///
/// - Missing `path` → `Err(String)` (hard error)
/// - Exists but no skill subdirs → `Ok(vec![])` (no error)
async fn list_skills_in_path(path: &str) -> Result<Vec<String>, String> {
    let mut out = vec![];
    let mut rd = tokio::fs::read_dir(path)
        .await
        .map_err(|e| format!("skills_path '{}' not readable: {}", path, e))?;
    while let Some(entry) = rd
        .next_entry()
        .await
        .map_err(|e| format!("reading entry in '{}': {}", path, e))?
    {
        if !entry.path().is_dir() {
            continue;
        }
        if entry.path().join("SKILL.md").exists() {
            if let Some(name) = entry.file_name().to_str() {
                out.push(name.to_string());
            }
        }
    }
    Ok(out)
}

/// Synchronous counterpart to [`list_skills_in_path`] — used inside the
/// (sync) `build_skill_repository_from_config` function to expand a parent
/// directory into individual skill-dir absolute paths.
///
/// Returns the full absolute path to each immediate subdir that contains
/// `SKILL.md`. Missing `path` → `Err`.
fn list_skill_dirs_sync(path: &str) -> Result<Vec<String>, std::io::Error> {
    let mut out = vec![];
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let ep = entry.path();
        if ep.is_dir() && ep.join("SKILL.md").exists() {
            out.push(ep.to_string_lossy().into_owned());
        }
    }
    Ok(out)
}

/// Derive the HTTP base URL for the CRDT documents REST API from a
/// `ws_url` like `ws://host:port/yjs` → `http://host:port`. Mirrors
/// the conventional pairing (WS at `/yjs`, REST at the root) used by
/// `crdt_documents::server`. Conservative: if the input doesn't start
/// with `ws://` or `wss://`, return it unchanged and let the agent
/// surface HTTP errors when the backend is used.
fn ws_url_to_http_base(ws_url: &str) -> String {
    let http = if let Some(rest) = ws_url.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = ws_url.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        ws_url.to_string()
    };
    http.trim_end_matches("/yjs")
        .trim_end_matches('/')
        .to_string()
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
    let sign = if raw_offset.starts_with('-') {
        "-"
    } else {
        "+"
    };
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

/// Persist the bytes of an inbound attachment (`inputs.files[]`) to the
/// `OutputStorageRepository` so the file can later be resolved by
/// `$attachment:<document_id>` references regardless of where it originated.
///
/// Resolution strategy (Plan A — Foundation):
///   1. If `retained_inline_bytes` is `Some(_)` → upload those bytes directly.
///      This covers both `FileSource::InlineBytes` (data:/path) entries and
///      `FileSource::Uploaded` entries that retained their inline bytes after
///      provider upload.
///   2. Else, if `attachment_source` is `AttachmentSource::SignedUrl(url)` →
///      re-fetch the URL via HTTP and persist the bytes. The original
///      download has already happened (to upload to the provider's Files
///      API), but those bytes are not kept around.
///      TODO(plan-a-opt): share bytes with provider upload to avoid re-fetch.
///   3. Else → return `None` (no storage_key persisted; the attachment is
///      still registered, but downstream `$attachment:<id>` consumers will
///      not resolve it).
///
/// Returns `None` on any failure (logged at warn level); persistence is
/// best-effort — the LLM call must continue even when storage is offline.
async fn persist_attachment_bytes(
    storage: &dyn crate::storage::domain::OutputStorageRepository,
    retained_inline_bytes: Option<&[u8]>,
    attachment_source: &crate::llm::domain::attachments::AttachmentSource,
    mime_type: &str,
    filename: &str,
    agent_session_id: &str,
    document_id: &str,
) -> Option<String> {
    use crate::llm::domain::attachments::AttachmentSource;
    use crate::storage::domain::StoreRequest;

    let bytes_for_storage: Option<Vec<u8>> = if let Some(b) = retained_inline_bytes {
        Some(b.to_vec())
    } else if let AttachmentSource::SignedUrl(url) = attachment_source {
        // Re-fetch the bytes. We intentionally do not share an HTTP client
        // here because this is an out-of-band, best-effort persistence path
        // — perf is dominated by the provider upload that already happened.
        // TODO(plan-a-opt): share bytes with provider upload to avoid re-fetch.
        match reqwest::get(url.as_str()).await {
            Ok(resp) => match resp.error_for_status() {
                Ok(ok_resp) => match ok_resp.bytes().await {
                    Ok(b) => Some(b.to_vec()),
                    Err(e) => {
                        tracing::warn!(
                            target: "colmena::attachment",
                            error = %e,
                            document_id = %document_id,
                            "failed to read signed-url bytes for storage persistence"
                        );
                        None
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        target: "colmena::attachment",
                        error = %e,
                        document_id = %document_id,
                        "signed-url returned non-success status during storage persistence"
                    );
                    None
                }
            },
            Err(e) => {
                tracing::warn!(
                    target: "colmena::attachment",
                    error = %e,
                    document_id = %document_id,
                    "failed to fetch signed-url bytes for storage persistence"
                );
                None
            }
        }
    } else {
        tracing::debug!(
            target: "colmena::attachment",
            document_id = %document_id,
            source_kind = attachment_source.kind_str(),
            "no path to persist attachment bytes; $attachment:<id> lookup will fail downstream"
        );
        None
    };

    let bytes = bytes_for_storage?;
    let size = bytes.len();

    let req = StoreRequest {
        bytes,
        mime_type: mime_type.to_string(),
        filename: filename.to_string(),
        session_id: None,
        agent_session_id: Some(agent_session_id.to_string()),
    };

    match storage.store(req).await {
        Ok(out) => Some(out.storage_key),
        Err(e) => {
            tracing::warn!(
                target: "colmena::attachment",
                error = %e,
                document_id = %document_id,
                mime = %mime_type,
                size_bytes = size,
                agent_session_id = %agent_session_id,
                filename = %filename,
                "failed to persist bytes to storage; attachment registered without storage_key"
            );
            None
        }
    }
}

/// Build the initial user message that opens a fresh LLM turn.
///
/// Plan B (D6): the LLM no longer receives file content in turn 1. The
/// catalog block prepended to the system message (Plan A Task 11) tells
/// the model which documents are available; the model calls
/// `load_attachment(document_id)` to read content, or references
/// `"$attachment:<document_id>"` in tool args to forward bytes without
/// reading them. This trades a round-trip for cost savings — see
/// `docs/developer_guide/31_load_attachment.md`.
///
/// `_resolved_files` is intentionally unused HERE — bytes are still
/// persisted to `OutputStorageRepository` and registered in the
/// attachment catalog further upstream in `execute()`.
///
/// The parameter is kept (instead of being removed) because the
/// `first_turn_user_message_does_not_carry_files_after_plan_b` regression
/// test in this module passes a non-empty `files` slice and asserts that
/// the produced message still has no attached files. Without the
/// parameter, that test would have to fake a different shape and would no
/// longer document the Plan B invariant at the call site. If you remove
/// the param, update the regression test accordingly.
fn build_initial_user_message(
    prompt: &str,
    _resolved_files: &[crate::llm::domain::FileData],
) -> Result<LlmMessage, crate::llm::domain::LlmError> {
    LlmMessage::user(prompt.to_string())
}

#[cfg(test)]
mod build_initial_user_message_tests {
    use super::*;
    use crate::llm::domain::{FileData, FileSource};

    fn inline_file(doc_id: &str) -> FileData {
        FileData {
            document_id: Some(doc_id.to_string()),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_hint: Some(5),
            source: FileSource::InlineBytes {
                bytes: b"hello".to_vec(),
            },
            retained_inline_bytes: Some(b"hello".to_vec()),
        }
    }

    #[test]
    fn first_turn_user_message_does_not_carry_files_after_plan_b() {
        // Plan B (D6): the LLM no longer receives file content in the
        // initial user message. The catalog block in the system message
        // tells the model what's available; the model calls
        // load_attachment to read.
        let files = vec![inline_file("doc-1"), inline_file("doc-2")];
        let msg = build_initial_user_message("read the docs", &files).unwrap();

        assert_eq!(msg.role().as_str(), "user");
        assert_eq!(msg.content(), "read the docs");
        assert!(
            msg.files().is_none() || msg.files().map(|f| f.is_empty()).unwrap_or(true),
            "Plan B: initial user message MUST NOT carry files; got: {:?}",
            msg.files()
        );
    }

    #[test]
    fn empty_files_slice_still_produces_user_message() {
        let msg = build_initial_user_message("hi", &[]).unwrap();
        assert_eq!(msg.role().as_str(), "user");
        assert_eq!(msg.content(), "hi");
        assert!(msg.files().is_none() || msg.files().map(|f| f.is_empty()).unwrap_or(true));
    }
}

#[cfg(test)]
mod persist_attachment_bytes_tests {
    use super::*;
    use crate::llm::domain::attachments::AttachmentSource;
    use crate::storage::domain::{MockOutputStorageRepository, StoredOutput};
    use mockall::predicate::*;

    fn stored(key: &str) -> StoredOutput {
        StoredOutput {
            storage_key: key.to_string(),
            read_url: format!("https://example/{}", key),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_bytes: 5,
        }
    }

    #[tokio::test]
    async fn inline_bytes_are_persisted_and_storage_key_is_returned() {
        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_store()
            .times(1)
            .withf(|req| {
                req.bytes == b"hello"
                    && req.mime_type == "application/pdf"
                    && req.filename == "x.pdf"
                    && req.agent_session_id.as_deref() == Some("agent_1")
                    && req.session_id.is_none()
            })
            .returning(|_| Ok(stored("sk-inline-test")));

        let key = persist_attachment_bytes(
            &storage,
            Some(b"hello"),
            &AttachmentSource::Inline,
            "application/pdf",
            "x.pdf",
            "agent_1",
            "doc-1",
        )
        .await;

        assert_eq!(key.as_deref(), Some("sk-inline-test"));
    }

    #[tokio::test]
    async fn inline_path_takes_precedence_over_signed_url() {
        // If we have retained bytes (e.g. inline file that was uploaded to
        // provider), we must NOT re-fetch the URL — bytes are already in RAM.
        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_store()
            .times(1)
            .withf(|req| req.bytes == b"local")
            .returning(|_| Ok(stored("sk-inline-priority")));

        let key = persist_attachment_bytes(
            &storage,
            Some(b"local"),
            &AttachmentSource::SignedUrl("http://127.0.0.1:1/never-fetched".into()),
            "application/pdf",
            "x.pdf",
            "agent_1",
            "doc-1",
        )
        .await;

        assert_eq!(key.as_deref(), Some("sk-inline-priority"));
    }

    #[tokio::test]
    async fn inline_source_without_retained_bytes_returns_none() {
        // No bytes available, source is Inline (no URL to fetch) → cannot
        // persist; storage.store must NOT be called.
        let storage = MockOutputStorageRepository::new();
        // No expect_store — strict mock will fail if called.

        let key = persist_attachment_bytes(
            &storage,
            None,
            &AttachmentSource::Inline,
            "application/pdf",
            "x.pdf",
            "agent_1",
            "doc-1",
        )
        .await;

        assert!(key.is_none());
    }

    #[tokio::test]
    async fn signed_url_with_no_retained_bytes_fetches_and_persists() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/file.pdf"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"remote-bytes".to_vec()))
            .mount(&server)
            .await;

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_store()
            .times(1)
            .withf(|req| req.bytes == b"remote-bytes")
            .returning(|_| Ok(stored("sk-url-test")));

        let url = format!("{}/file.pdf", server.uri());
        let key = persist_attachment_bytes(
            &storage,
            None,
            &AttachmentSource::SignedUrl(url),
            "application/pdf",
            "x.pdf",
            "agent_1",
            "doc-url",
        )
        .await;

        assert_eq!(key.as_deref(), Some("sk-url-test"));
    }

    #[tokio::test]
    async fn storage_error_returns_none_without_propagating() {
        use crate::storage::domain::StorageError;

        let mut storage = MockOutputStorageRepository::new();
        storage
            .expect_store()
            .times(1)
            .returning(|_| Err(StorageError::BackendUnavailable("nope".into())));

        let key = persist_attachment_bytes(
            &storage,
            Some(b"hello"),
            &AttachmentSource::Inline,
            "application/pdf",
            "x.pdf",
            "agent_1",
            "doc-1",
        )
        .await;

        assert!(key.is_none());
    }
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
                storage_key: None,
                origin: None,
            })
            .await
            .unwrap();

        let resolver = AttachmentResolverImpl {
            registry: registry.clone(),
            provider: ProviderKind::OpenAi,
            api_key: "dummy".to_string(),
            storage: None,
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
            storage: None,
        };
        let res = resolver.resolve("agent_1", "missing").await.unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn resolver_returns_none_when_only_generated_row_exists_but_no_storage() {
        // When a Generated row exists but storage adapter is missing, the
        // resolver cannot upload bytes and must error (so the LLM gets a
        // clear "attachment unavailable" rather than a successful but empty
        // resolution).
        use crate::llm::application::LoadAttachmentResolver;
        use crate::llm::domain::attachments::{AttachmentSource, UpsertAttachmentInput};
        use crate::llm::domain::AttachmentRegistry;
        use crate::llm::domain::ProviderKind;
        use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
        use std::sync::Arc;

        let registry = Arc::new(
            SqliteAttachmentRegistry::new("sqlite::memory:")
                .await
                .unwrap(),
        );
        registry
            .upsert(UpsertAttachmentInput {
                agent_session_id: "agent_1".to_string(),
                document_id: "gen-att-xyz".to_string(),
                provider: ProviderKind::Generated,
                provider_file_id: "local://abc".to_string(),
                mime_type: "image/png".to_string(),
                filename: "image_0.png".to_string(),
                size_bytes: Some(100),
                label: None,
                description: Some("Image generated".to_string()),
                source: AttachmentSource::SignedUrl("data:image/png;base64,XX".to_string()),
                storage_key: None,
                origin: None,
            })
            .await
            .unwrap();

        let resolver = AttachmentResolverImpl {
            registry: registry.clone(),
            provider: ProviderKind::OpenAi,
            api_key: "dummy".to_string(),
            storage: None, // no storage adapter
        };

        // No storage → must error (not silently return None) so the LLM is
        // informed that the artifact exists but cannot be loaded.
        let res = resolver.resolve("agent_1", "gen-att-xyz").await;
        assert!(res.is_err(), "expected error when storage missing");
        let err = res.unwrap_err();
        assert!(
            err.contains("OutputStorageRepository"),
            "error message should mention storage: {err}"
        );
    }

    #[tokio::test]
    async fn resolver_touches_last_used_at_on_successful_load() {
        // D10: a `load_attachment` invocation counts as "using" the
        // attachment — resolve() must update `last_used_at` so the GC's
        // `COALESCE(last_used_at, registered_at) < cutoff` staleness check
        // treats actively-read attachments as fresh. Without this, a doc
        // read every day via load_attachment but never forwarded would be
        // reaped TTL days after registration. Mirrors the touch that
        // AttachmentStreamResolverImpl performs on the Plan A path.
        use crate::llm::application::LoadAttachmentResolver;
        use crate::llm::domain::attachments::{AttachmentSource, UpsertAttachmentInput};
        use crate::llm::domain::AttachmentRegistry;
        use crate::llm::domain::ProviderKind;
        use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
        use std::sync::Arc;

        let registry: Arc<dyn AttachmentRegistry> = Arc::new(
            SqliteAttachmentRegistry::new("sqlite::memory:")
                .await
                .unwrap(),
        );
        // Inline source → not recoverable → resolve() takes the fast path
        // (no provider re-upload), exercising the common success branch.
        registry
            .upsert(UpsertAttachmentInput {
                agent_session_id: "agent_1".to_string(),
                document_id: "doc-1".to_string(),
                provider: ProviderKind::OpenAi,
                provider_file_id: "file-abc".to_string(),
                mime_type: "application/pdf".to_string(),
                filename: "doc.pdf".to_string(),
                size_bytes: Some(100),
                label: None,
                description: None,
                source: AttachmentSource::Inline,
                storage_key: Some("chat-attachments/agent_1/doc.pdf".to_string()),
                origin: None,
            })
            .await
            .unwrap();

        // Precondition: last_used_at is NULL right after upsert.
        let before = registry
            .lookup("agent_1", "doc-1", ProviderKind::OpenAi)
            .await
            .unwrap()
            .expect("row should exist after upsert");
        assert!(
            before.last_used_at.is_none(),
            "precondition: last_used_at must be NULL right after upsert"
        );

        let resolver = AttachmentResolverImpl {
            registry: registry.clone(),
            provider: ProviderKind::OpenAi,
            api_key: "dummy".to_string(),
            storage: None,
        };

        let res = resolver.resolve("agent_1", "doc-1").await.unwrap();
        assert!(res.is_some(), "expected a successful resolution");

        // D10 assertion: last_used_at must now be populated.
        let after = registry
            .lookup("agent_1", "doc-1", ProviderKind::OpenAi)
            .await
            .unwrap()
            .expect("row should still exist");
        assert!(
            after.last_used_at.is_some(),
            "D10: load_attachment resolve() must touch last_used_at"
        );
    }
}

#[cfg(test)]
mod attachment_catalog_integration_tests {
    //! Plan A — verify the per-document attachment catalog is rendered from
    //! the same shape we ship to the LLM. We register an attachment via the
    //! real `SqliteAttachmentRegistry`, list it for the session, render the
    //! catalog block with the same helper used in `execute()`, and confirm
    //! the resulting system-message section contains the document_id, the
    //! `load_attachment(...)` hint, and the `$attachment:<id>` forwarder.
    //! This is the right layer below `execute()` — `execute()` itself wires
    //! provider repos and is heavyweight to exercise in a unit test.

    use crate::llm::application::attachment_catalog::render_catalog;
    use crate::llm::domain::attachments::{AttachmentSource, UpsertAttachmentInput};
    use crate::llm::domain::{AttachmentRegistry, ProviderKind};
    use crate::llm::infrastructure::persistence::SqliteAttachmentRegistry;
    use std::sync::Arc;

    #[tokio::test]
    async fn catalog_block_lists_registered_doc_with_usage_hints() {
        let registry: Arc<dyn AttachmentRegistry> = Arc::new(
            SqliteAttachmentRegistry::new("sqlite::memory:")
                .await
                .unwrap(),
        );
        registry
            .upsert(UpsertAttachmentInput {
                agent_session_id: "agent_catalog_test".to_string(),
                document_id: "doc-test".to_string(),
                provider: ProviderKind::OpenAi,
                provider_file_id: "pf-test".to_string(),
                mime_type: "application/pdf".to_string(),
                filename: "report.pdf".to_string(),
                size_bytes: Some(2 * 1024 * 1024),
                label: Some("Q3 report".to_string()),
                description: Some("Quarterly results".to_string()),
                source: AttachmentSource::Inline,
                storage_key: Some("sk-abc".to_string()),
                origin: Some("user_upload".to_string()),
            })
            .await
            .unwrap();

        let listed = registry
            .list_for_session("agent_catalog_test")
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);

        let block = render_catalog(&listed).expect("non-empty list must produce a catalog block");

        // Simulate the section-joining done in `execute()`.
        let assembled = ["existing system text".to_string(), block].join("\n\n---\n");

        assert!(
            assembled.contains("Documents available in this session:"),
            "header missing in assembled system message:\n{assembled}"
        );
        assert!(
            assembled.contains("[doc-test]"),
            "document_id missing in assembled system message:\n{assembled}"
        );
        assert!(
            assembled.contains("load_attachment(\"doc-test\")"),
            "load_attachment usage hint missing:\n{assembled}"
        );
        assert!(
            assembled.contains("\"$attachment:doc-test\""),
            "$attachment placeholder hint missing:\n{assembled}"
        );
        assert!(
            assembled.contains("origin: uploaded by user"),
            "user_upload origin not humanized:\n{assembled}"
        );
    }

    #[tokio::test]
    async fn catalog_block_is_none_when_session_has_no_attachments() {
        let registry: Arc<dyn AttachmentRegistry> = Arc::new(
            SqliteAttachmentRegistry::new("sqlite::memory:")
                .await
                .unwrap(),
        );
        let listed = registry.list_for_session("empty_session").await.unwrap();
        assert!(render_catalog(&listed).is_none());
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
        assert!(
            out.contains("Timezone: America/Bogota (UTC-5)"),
            "tz line: {}",
            out
        );
        assert!(
            out.contains("Location: Bogotá, Colombia"),
            "loc line: {}",
            out
        );
        assert!(out.contains("Locale: es-CO"), "locale line: {}", out);
    }

    #[test]
    fn half_hour_offset_renders_correctly() {
        let out = format_temporal_context_block("Asia/Kolkata", "Mumbai, India", "hi-IN");
        assert!(
            out.contains("Timezone: Asia/Kolkata (UTC+5:30)"),
            "expected UTC+5:30 in: {}",
            out
        );
        assert!(out.contains("Locale: hi-IN"));
    }

    #[test]
    fn invalid_iana_falls_back_coherently() {
        let out = format_temporal_context_block("Mars/Olympus", "Mars Base", "en-US");
        assert!(
            out.contains("Timezone: America/Bogota (UTC-5)"),
            "fallback tz: {}",
            out
        );
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

#[cfg(test)]
mod filter_enabled_tools_tests {
    //! Coverage for `filter_enabled_tools`, the pure helper that decides which
    //! tools from `tool_executor.available_tools()` are exposed to the LLM.
    //!
    //! Key behaviors verified:
    //!   - Wildcard `"*"` keeps every tool.
    //!   - Exact-name match works (back-compat — listing
    //!     `api_explorer__load_spec` still enables that single sub-tool).
    //!   - Toolkit prefix match works (listing the alias `api_explorer`
    //!     enables every `api_explorer__*` sub-tool — flag-only ergonomic
    //!     parity with `tool_configurations`).
    //!   - `configured_aliases` are auto-enabled (no need to also list them
    //!     under `enabled_tools`).
    use super::filter_enabled_tools;
    use crate::llm::domain::{ToolDefinition, ToolParameters};
    use serde_json::json;
    use std::collections::{HashMap, HashSet};

    fn td(name: &str) -> ToolDefinition {
        ToolDefinition::new(
            name.to_string(),
            format!("desc for {}", name),
            ToolParameters {
                schema_type: "object".to_string(),
                properties: HashMap::new(),
                required: Vec::new(),
            },
        )
    }

    fn api_explorer_catalog() -> Vec<ToolDefinition> {
        vec![
            td("api_explorer__load_spec"),
            td("api_explorer__search_endpoint"),
            td("api_explorer__get_endpoint_details"),
            td("api_explorer__build_http_request"),
            td("api_explorer__list_endpoints"),
            td("current_time"),
        ]
    }

    #[test]
    fn enabled_tools_api_explorer_alias_enables_all_subtools() {
        // Flag-only path: NO tool_configurations (so configured_aliases is
        // empty), and the user writes `enabled_tools: ["api_explorer"]`.
        // The catalog returned by `available_tools()` already contains the
        // five expanded sub-tools (commit 131c540 made that change). The
        // filter must accept the alias and yield all five sub-tools.
        let enabled = json!(["api_explorer"]);
        let configured: HashSet<String> = HashSet::new();

        let out = filter_enabled_tools(api_explorer_catalog(), Some(&enabled), &configured);
        let names: Vec<&str> = out.iter().map(|t| t.name.as_str()).collect();

        assert!(
            names.contains(&"api_explorer__load_spec"),
            "expected api_explorer__load_spec in {:?}",
            names
        );
        assert!(
            names.contains(&"api_explorer__search_endpoint"),
            "expected api_explorer__search_endpoint in {:?}",
            names
        );
        assert!(
            names.contains(&"api_explorer__get_endpoint_details"),
            "expected api_explorer__get_endpoint_details in {:?}",
            names
        );
        assert!(
            names.contains(&"api_explorer__build_http_request"),
            "expected api_explorer__build_http_request in {:?}",
            names
        );
        assert!(
            names.contains(&"api_explorer__list_endpoints"),
            "expected api_explorer__list_endpoints in {:?}",
            names
        );
        assert!(
            !names.contains(&"current_time"),
            "current_time was not requested but slipped through: {:?}",
            names
        );
        assert_eq!(
            out.len(),
            5,
            "expected exactly the 5 api_explorer sub-tools, got {:?}",
            names
        );
    }

    #[test]
    fn enabled_tools_exact_subtool_name_still_works() {
        // Back-compat: listing the fully-qualified sub-tool name keeps
        // working via the exact-equality branch and enables ONLY that one.
        let enabled = json!(["api_explorer__load_spec"]);
        let configured: HashSet<String> = HashSet::new();

        let out = filter_enabled_tools(api_explorer_catalog(), Some(&enabled), &configured);
        let names: Vec<&str> = out.iter().map(|t| t.name.as_str()).collect();

        assert_eq!(names, vec!["api_explorer__load_spec"], "names: {:?}", names);
    }

    #[test]
    fn enabled_tools_wildcard_exposes_everything() {
        let enabled = json!("*");
        let configured: HashSet<String> = HashSet::new();

        let out = filter_enabled_tools(api_explorer_catalog(), Some(&enabled), &configured);

        assert_eq!(out.len(), 6, "wildcard must keep all tools");
    }

    #[test]
    fn configured_aliases_auto_enable_subtools_without_enabled_tools() {
        // When `tool_configurations` declares `api_explorer`, the alias is
        // auto-enabled — the user does NOT need to also list it under
        // `enabled_tools`. This is the legacy path and must keep working.
        let mut configured: HashSet<String> = HashSet::new();
        configured.insert("api_explorer".to_string());

        let out = filter_enabled_tools(api_explorer_catalog(), None, &configured);
        let names: Vec<&str> = out.iter().map(|t| t.name.as_str()).collect();

        assert_eq!(
            out.len(),
            5,
            "expected 5 api_explorer sub-tools, got {:?}",
            names
        );
        assert!(!names.contains(&"current_time"));
    }

    #[test]
    fn unknown_name_in_enabled_tools_is_silently_dropped() {
        let enabled = json!(["definitely_not_a_tool"]);
        let configured: HashSet<String> = HashSet::new();

        let out = filter_enabled_tools(api_explorer_catalog(), Some(&enabled), &configured);

        assert!(out.is_empty(), "expected empty result, got {:?}", out);
    }

    fn build_fake_catalog(names: &[&str]) -> Vec<crate::llm::domain::ToolDefinition> {
        use crate::llm::domain::tools::{ToolDefinition, ToolParameters};
        names
            .iter()
            .map(|n| {
                ToolDefinition::new(
                    n.to_string(),
                    format!("description of {}", n),
                    ToolParameters::new(),
                )
            })
            .collect()
    }

    #[test]
    fn package_alias_expands_to_all_tools() {
        let all_tools = build_fake_catalog(&[
            "gsheets_create_spreadsheet",
            "gsheets_create_from_xlsx",
            "gsheets_export_xlsx",
            "gsheets_list_sheets",
            "gsheets_add_sheet",
            "gsheets_delete_sheet",
            "gsheets_read",
            "gsheets_set_cell",
            "gsheets_set_range",
            "gsheets_run_python",
            "tavily_web",
        ]);
        let enabled = json!(["gsheets"]);
        let configured = std::collections::HashSet::new();
        let filtered = super::filter_enabled_tools(all_tools, Some(&enabled), &configured);
        assert_eq!(filtered.len(), 10, "gsheets alias must expand to 10 tools");
        assert!(filtered.iter().all(|t| t.name.starts_with("gsheets_")));
    }

    #[test]
    fn package_plus_individual_tool_works() {
        let all_tools = build_fake_catalog(&[
            "gsheets_read",
            "gsheets_set_cell",
            "gsheets_create_spreadsheet",
            "gsheets_create_from_xlsx",
            "gsheets_export_xlsx",
            "gsheets_list_sheets",
            "gsheets_add_sheet",
            "gsheets_delete_sheet",
            "gsheets_set_range",
            "gsheets_run_python",
            "tavily_web",
        ]);
        let enabled = json!(["gsheets", "tavily_web"]);
        let filtered = super::filter_enabled_tools(
            all_tools,
            Some(&enabled),
            &std::collections::HashSet::new(),
        );
        assert_eq!(filtered.len(), 11);
    }

    #[test]
    fn exclusion_removes_tool_from_package() {
        let all_tools = build_fake_catalog(&[
            "gsheets_read",
            "gsheets_delete_sheet",
            "gsheets_list_sheets",
            "gsheets_add_sheet",
            "gsheets_set_cell",
            "gsheets_set_range",
            "gsheets_create_spreadsheet",
            "gsheets_create_from_xlsx",
            "gsheets_export_xlsx",
            "gsheets_run_python",
        ]);
        let enabled = json!(["gsheets", "!gsheets_delete_sheet"]);
        let filtered = super::filter_enabled_tools(
            all_tools,
            Some(&enabled),
            &std::collections::HashSet::new(),
        );
        assert_eq!(filtered.len(), 9);
        assert!(!filtered.iter().any(|t| t.name == "gsheets_delete_sheet"));
    }

    #[test]
    fn exclusion_order_independent() {
        let all_tools = build_fake_catalog(&[
            "gsheets_read",
            "gsheets_delete_sheet",
            "gsheets_list_sheets",
            "gsheets_add_sheet",
            "gsheets_set_cell",
            "gsheets_set_range",
            "gsheets_create_spreadsheet",
            "gsheets_create_from_xlsx",
            "gsheets_export_xlsx",
            "gsheets_run_python",
        ]);
        let order_a = json!(["gsheets", "!gsheets_read"]);
        let order_b = json!(["!gsheets_read", "gsheets"]);
        let configured = std::collections::HashSet::new();
        let names_a: std::collections::HashSet<String> =
            super::filter_enabled_tools(all_tools.clone(), Some(&order_a), &configured)
                .into_iter()
                .map(|t| t.name)
                .collect();
        let names_b: std::collections::HashSet<String> =
            super::filter_enabled_tools(all_tools, Some(&order_b), &configured)
                .into_iter()
                .map(|t| t.name)
                .collect();
        assert_eq!(names_a, names_b, "exclusion order must not matter");
    }

    #[test]
    fn exclusion_of_package_removes_all_its_tools() {
        let all_tools = build_fake_catalog(&[
            "gsheets_read",
            "gsheets_set_cell",
            "tavily_web",
            "current_time",
            "gsheets_create_spreadsheet",
            "gsheets_create_from_xlsx",
            "gsheets_export_xlsx",
            "gsheets_list_sheets",
            "gsheets_add_sheet",
            "gsheets_delete_sheet",
            "gsheets_set_range",
            "gsheets_run_python",
        ]);
        let enabled = json!(["*", "!gsheets"]);
        let filtered = super::filter_enabled_tools(
            all_tools,
            Some(&enabled),
            &std::collections::HashSet::new(),
        );
        let names: std::collections::HashSet<String> =
            filtered.into_iter().map(|t| t.name).collect();
        assert!(!names.iter().any(|n| n.starts_with("gsheets_")));
        assert!(names.contains("tavily_web"));
        assert!(names.contains("current_time"));
    }

    #[test]
    fn unknown_alias_silently_ignored() {
        let all_tools = build_fake_catalog(&["gsheets_read", "tavily_web"]);
        let enabled = json!(["gsheetz"]);
        let filtered = super::filter_enabled_tools(
            all_tools,
            Some(&enabled),
            &std::collections::HashSet::new(),
        );
        assert_eq!(
            filtered.len(),
            0,
            "unknown alias produces empty result, no panic"
        );
    }

    #[test]
    fn exact_tool_name_match_still_works_unchanged() {
        let all_tools = build_fake_catalog(&["gsheets_read", "tavily_web"]);
        let enabled = json!(["gsheets_read"]);
        let filtered = super::filter_enabled_tools(
            all_tools,
            Some(&enabled),
            &std::collections::HashSet::new(),
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "gsheets_read");
    }

    #[test]
    fn empty_exclusion_logged_and_ignored() {
        let all_tools = build_fake_catalog(&["gsheets_read", "tavily_web"]);
        let enabled = json!(["gsheets_read", "!"]);
        let filtered = super::filter_enabled_tools(
            all_tools,
            Some(&enabled),
            &std::collections::HashSet::new(),
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "gsheets_read");
    }
}

#[cfg(test)]
mod resolve_synthetic_enabled_tools_tests {
    //! Coverage for `resolve_synthetic_enabled_tools`. Regression suite for the
    //! bug that left `enabled_tools: ["gsheets"]` exposing 0 tools because the
    //! synthetic-tools block didn't expand the toolkit-package alias and didn't
    //! honor `!entry` exclusions. The helper centralizes that logic now;
    //! both the gsheets and gdocs synthetic blocks call it.
    use super::resolve_synthetic_enabled_tools;
    use serde_json::json;

    const GSHEETS_ALL: [&str; 10] = [
        "gsheets_create_spreadsheet",
        "gsheets_create_from_xlsx",
        "gsheets_export_xlsx",
        "gsheets_list_sheets",
        "gsheets_add_sheet",
        "gsheets_delete_sheet",
        "gsheets_read",
        "gsheets_set_cell",
        "gsheets_set_range",
        "gsheets_run_python",
    ];

    #[test]
    fn alias_expands_to_full_package() {
        let cfg = json!(["gsheets"]);
        let (wants, excludes) = resolve_synthetic_enabled_tools(Some(&cfg), &GSHEETS_ALL);
        assert_eq!(wants.len(), 10);
        assert!(excludes.is_empty());
    }

    #[test]
    fn alias_with_exclusions_yields_partial_package() {
        // Reproduces the exact ADP payload that surfaced the original bug:
        // gsheets alias + two negated sub-tools must net 8 wants and 2 excludes.
        let cfg = json!([
            "gsheets",
            "!gsheets_create_from_xlsx",
            "!gsheets_export_xlsx"
        ]);
        let (wants, excludes) = resolve_synthetic_enabled_tools(Some(&cfg), &GSHEETS_ALL);
        assert_eq!(wants.len(), 10);
        assert_eq!(excludes.len(), 2);
        assert!(excludes.contains("gsheets_create_from_xlsx"));
        assert!(excludes.contains("gsheets_export_xlsx"));
        // Final set (wants - excludes) used by the synthetic block.
        let final_set: std::collections::HashSet<&&str> = wants.difference(&excludes).collect();
        assert_eq!(final_set.len(), 8);
    }

    #[test]
    fn wildcard_string_enables_all() {
        let cfg = json!("*");
        let (wants, excludes) = resolve_synthetic_enabled_tools(Some(&cfg), &GSHEETS_ALL);
        assert_eq!(wants.len(), 10);
        assert!(excludes.is_empty());
    }

    #[test]
    fn wildcard_in_array_with_exclusion() {
        let cfg = json!(["*", "!gsheets_run_python"]);
        let (wants, excludes) = resolve_synthetic_enabled_tools(Some(&cfg), &GSHEETS_ALL);
        assert_eq!(wants.len(), 10);
        assert_eq!(excludes.len(), 1);
        assert!(excludes.contains("gsheets_run_python"));
    }

    #[test]
    fn exact_tool_name_works_without_alias() {
        let cfg = json!(["gsheets_read", "gsheets_set_cell"]);
        let (wants, excludes) = resolve_synthetic_enabled_tools(Some(&cfg), &GSHEETS_ALL);
        assert_eq!(wants.len(), 2);
        assert!(wants.contains("gsheets_read"));
        assert!(wants.contains("gsheets_set_cell"));
        assert!(excludes.is_empty());
    }

    #[test]
    fn alias_only_in_string_form() {
        let cfg = json!("gsheets");
        let (wants, _) = resolve_synthetic_enabled_tools(Some(&cfg), &GSHEETS_ALL);
        assert_eq!(wants.len(), 10);
    }

    #[test]
    fn entries_unrelated_to_known_set_are_ignored() {
        // `gdocs_create` is a real package tool but not part of `GSHEETS_ALL`.
        // The gsheets synthetic block must silently ignore it (the gdocs
        // block will pick it up separately).
        let cfg = json!(["gsheets", "gdocs_create", "unknown_tool"]);
        let (wants, excludes) = resolve_synthetic_enabled_tools(Some(&cfg), &GSHEETS_ALL);
        assert_eq!(wants.len(), 10);
        assert!(excludes.is_empty());
    }

    #[test]
    fn cross_toolkit_exclusion_is_silently_ignored() {
        // Excluding a gdocs tool while scoped to gsheets is a no-op (the
        // exclusion would apply at the gdocs synthetic block instead).
        let cfg = json!(["gsheets", "!gdocs_create"]);
        let (wants, excludes) = resolve_synthetic_enabled_tools(Some(&cfg), &GSHEETS_ALL);
        assert_eq!(wants.len(), 10);
        assert!(excludes.is_empty());
    }

    #[test]
    fn empty_exclusion_marker_is_dropped() {
        let cfg = json!(["gsheets", "!"]);
        let (wants, excludes) = resolve_synthetic_enabled_tools(Some(&cfg), &GSHEETS_ALL);
        assert_eq!(wants.len(), 10);
        assert!(excludes.is_empty());
    }

    #[test]
    fn no_enabled_tools_yields_empty_sets() {
        let (wants, excludes) = resolve_synthetic_enabled_tools(None, &GSHEETS_ALL);
        assert!(wants.is_empty());
        assert!(excludes.is_empty());
    }

    #[test]
    fn gdocs_alias_expands_correctly() {
        // The gdocs synthetic block uses the same helper. Confirm `gdocs`
        // package alias resolves against a gdocs-only universe.
        let gdocs_all: [&str; 3] = ["gdocs_create", "gdocs_share", "gdocs_export"];
        let cfg = json!(["gdocs"]);
        let (wants, _) = resolve_synthetic_enabled_tools(Some(&cfg), &gdocs_all);
        // Helper filters package tools through `all_known`, so wants ⊆ gdocs_all.
        assert_eq!(wants.len(), 3);
    }
}

#[cfg(test)]
mod skills_path_tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn resolves_skills_path_single_directory() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("hello")).unwrap();
        std::fs::write(
            tmp.path().join("hello/SKILL.md"),
            "---\nname: hello\ndescription: hi\n---\nbody",
        )
        .unwrap();

        let cfg = json!({
            "provider": "openai",
            "model": "gpt-4o-mini",
            "api_key": "test",
            "skills_path": tmp.path().to_str().unwrap(),
        });
        let resolved = LlmNode::resolve_skill_names(&cfg).await.unwrap();
        assert!(resolved.iter().any(|n| n == "hello"), "got: {:?}", resolved);
    }

    #[tokio::test]
    async fn resolves_skills_paths_plural() {
        let tmp1 = tempfile::tempdir().unwrap();
        let tmp2 = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp1.path().join("a")).unwrap();
        std::fs::write(
            tmp1.path().join("a/SKILL.md"),
            "---\nname: a\ndescription: x\n---\n",
        )
        .unwrap();
        std::fs::create_dir_all(tmp2.path().join("b")).unwrap();
        std::fs::write(
            tmp2.path().join("b/SKILL.md"),
            "---\nname: b\ndescription: x\n---\n",
        )
        .unwrap();

        let cfg = json!({
            "provider": "openai",
            "model": "gpt-4o-mini",
            "api_key": "test",
            "skills_paths": [tmp1.path().to_str().unwrap(), tmp2.path().to_str().unwrap()],
        });
        let resolved = LlmNode::resolve_skill_names(&cfg).await.unwrap();
        assert!(
            resolved.iter().any(|n| n == "a"),
            "missing 'a' in {:?}",
            resolved
        );
        assert!(
            resolved.iter().any(|n| n == "b"),
            "missing 'b' in {:?}",
            resolved
        );
    }

    #[tokio::test]
    async fn unions_skills_array_with_skills_path_dedup() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("from-path")).unwrap();
        std::fs::write(
            tmp.path().join("from-path/SKILL.md"),
            "---\nname: from-path\ndescription: x\n---\n",
        )
        .unwrap();
        // also create a duplicate that's also in `skills:`
        std::fs::create_dir_all(tmp.path().join("dup")).unwrap();
        std::fs::write(
            tmp.path().join("dup/SKILL.md"),
            "---\nname: dup\ndescription: x\n---\n",
        )
        .unwrap();

        let cfg = json!({
            "provider": "openai",
            "model": "gpt-4o-mini",
            "api_key": "test",
            "skills": ["builtin-name", "dup"],
            "skills_path": tmp.path().to_str().unwrap(),
        });
        let resolved = LlmNode::resolve_skill_names(&cfg).await.unwrap();
        assert!(
            resolved.contains(&"builtin-name".to_string()),
            "missing builtin-name in {:?}",
            resolved
        );
        assert!(
            resolved.contains(&"from-path".to_string()),
            "missing from-path in {:?}",
            resolved
        );
        assert!(
            resolved.contains(&"dup".to_string()),
            "missing dup in {:?}",
            resolved
        );
        // dedup: dup should appear exactly once
        let count = resolved.iter().filter(|n| n.as_str() == "dup").count();
        assert_eq!(count, 1, "dup should appear once; got: {:?}", resolved);
    }

    #[tokio::test]
    async fn skills_path_missing_returns_error() {
        let cfg = json!({
            "provider": "openai",
            "model": "gpt-4o-mini",
            "api_key": "test",
            "skills_path": "/nonexistent/path/abc123xyz",
        });
        let err = LlmNode::resolve_skill_names(&cfg).await.unwrap_err();
        assert!(
            err.contains("not readable") || err.contains("nonexistent") || err.contains("No such"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn skills_path_empty_directory_returns_empty_list() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = json!({
            "provider": "openai",
            "model": "gpt-4o-mini",
            "api_key": "test",
            "skills_path": tmp.path().to_str().unwrap(),
        });
        let resolved = LlmNode::resolve_skill_names(&cfg).await.unwrap();
        assert!(
            resolved.is_empty(),
            "expected empty list, got: {:?}",
            resolved
        );
    }
}
