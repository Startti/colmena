//! Tool executor that bridges LLM tool calls to DAG node execution.
//!
//! [`DagToolExecutor`] implements [`ToolExecutor`]. When the LLM invokes a tool:
//! 1. The tool configuration is looked up by name.
//! 2. LLM arguments are merged with fixed values using one of three strategies (see below).
//! 3. `inject_secrets()` replaces `<value_N>` placeholders with real secret values.
//! 4. The DAG node is executed.
//! 5. If `secure: true` is set in `fixed_config`, `hash_output()` is called — the LLM
//!    receives opaque placeholders (`<value_1>`, `<value_2>`, …) and never sees real secrets.
//!
//! ## Merge strategies (in priority order)
//!
//! 1. **`node_schema`** — Full declarative control. Fixed values are seeded first; LLM args
//!    are placed into their target containers based on `param_to_container` from
//!    [`parse_node_schema`]. Use this for all non-trivial tools.
//!
//! 2. **`$DYNAMIC` placeholders** — Simpler alternative. The executor scans `fixed_config` for
//!    [`DYNAMIC_PLACEHOLDER`] string values and replaces each one with the LLM-provided value.
//!    Works one level deep inside container objects (e.g. `body.title`), but NOT for deeper
//!    nesting (e.g. `body.metadata.author.name` is NOT detected). Use only for simple cases.
//!
//! 3. **Deprecated fallback** — `field_mapping` + `mergeable_fields` + `exposed_inputs`.
//!    Executed for backward compatibility only. Not used when `node_schema` or `$DYNAMIC` is present.

use crate::colmena_log;
use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::node::ExecutableNode;
use crate::dag_engine::domain::tool_configuration::{ToolConfiguration, DYNAMIC_PLACEHOLDER};
use crate::llm::domain::{LlmError, ToolCall, ToolExecutor, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Callback fired when a `load_skill` tool call succeeds, carrying the dispatched
/// skill payload so the enclosing LLM node can emit observability events.
pub type SkillObserver = Arc<
    dyn Fn(&crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::LoadSkillDispatchResult)
        + Send
        + Sync,
>;

/// Callback fired when a `describe_tool` call succeeds. The enclosing LLM node
/// uses this to add the tool name to its discovered set and emit SSE events.
pub type ToolDescribeObserver = Arc<
    dyn Fn(
            &crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::DescribeToolDispatchResult,
        ) + Send
        + Sync,
>;

/// Executes DAG nodes on behalf of LLM tool calls.
///
/// Constructed via [`DagToolExecutor::new`] and optionally configured with
/// [`DagToolExecutor::with_secure_values`] for encrypted secret injection.
/// See module-level docs for the three merge strategies and the secure values flow.
pub struct DagToolExecutor {
    registry: Arc<dyn NodeRegistryPort>,
    tool_configurations: HashMap<String, ToolConfiguration>,
    /// Optional SecureValueService for decrypting <value_N> placeholders during tool calls.
    secure_value_service: Option<Arc<SecureValueService>>,
    /// Session ID used to scope secret lookup.
    session_id: Option<String>,
    /// Agent session id (chat handle). When Some, secret lookup is agent-first
    /// with session fallback — letting tool calls in a fresh ephemeral session
    /// resolve secrets persisted under the same chat in a previous run.
    agent_session_id: Option<String>,
    /// Optional skill repository. When present, the executor intercepts `load_skill`
    /// tool calls and dispatches them to this repository instead of the normal
    /// tool-configuration path. An optional observer callback receives SkillLoaded
    /// metadata so the enclosing LlmNode can emit SSE events.
    skill_repository: Option<Arc<dyn crate::skills::domain::SkillRepository>>,
    skill_observer: Option<SkillObserver>,
    /// Optional documents context. When present, the executor intercepts the
    /// seven `document_*` synthetic tool calls and dispatches them to the
    /// underlying `DocumentRuntime` use cases instead of the normal
    /// tool-configuration path.
    documents_context: Option<
        Arc<crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::DocumentToolsContext>,
    >,
    /// Per-call context for the v1 CRDT documents synthetic tools. Populated
    /// via `with_crdt_documents()` from the llm_call node.
    crdt_docs_context: Option<
        Arc<crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::CrdtDocsContext>,
    >,
    /// Snapshot of `ToolConfiguration` entries available for `describe_tool`
    /// to look up. When `Some(...)`, the executor intercepts `describe_tool`
    /// calls and dispatches against this slice; absent → describe_tool falls
    /// through and is treated as an unknown tool by the rest of the executor.
    describe_tool_lookup: Option<Vec<ToolConfiguration>>,
    describe_tool_observer: Option<ToolDescribeObserver>,
    /// Catalog snapshot for `load_attachment` interception. When present
    /// (`Some(...)`), the executor handles `load_attachment` calls by validating
    /// against this slice and returning a LOAD_ATTACHMENT sentinel. The actual
    /// registry handle stays in the llm_call node; we only need the catalog
    /// here so dispatch can succeed without an extra dependency.
    attachment_catalog: Option<Vec<crate::llm::domain::ConversationAttachment>>,
    /// Per-string size cap applied to tool results before they are returned
    /// to the LLM. Strings whose byte length exceeds this value are replaced
    /// with `[truncated: original_size=N bytes]`. Defaults to
    /// [`DEFAULT_MAX_TOOL_RESULT_STRING_BYTES`].
    ///
    /// Independent from the data-URI elision, which is always applied because
    /// binary base64 in the LLM context never makes sense — it just burns
    /// tokens and risks tripping the model's TPM rate limit.
    max_tool_result_bytes: usize,
}

/// Default per-string cap for tool results (50 KB). Above this, the string is
/// replaced with a truncation marker. Set explicitly via
/// [`DagToolExecutor::with_max_tool_result_bytes`] (or per-llm_call via the
/// `max_tool_result_bytes` config field).
pub const DEFAULT_MAX_TOOL_RESULT_STRING_BYTES: usize = 50 * 1024;

impl DagToolExecutor {
    /// Resolve `${var}` and `${context.var}` placeholders in a string value
    /// using values from the inputs map. Only resolves keys present in `inputs`;
    /// unrecognized placeholders are left as-is.
    /// Note: this is a shallow template resolution for `fixed_config` string fields.
    /// Full node-output path resolution (e.g. `${node_name.field.path}`) happens
    /// upstream in the DAG engine before the tool executor is called.
    fn resolve_template_string(template: &str, inputs: &HashMap<String, Value>) -> String {
        use regex::Regex;

        // Pattern: ${context.key} or ${key}
        let re = Regex::new(r"\$\{(?:context\.)?(\w+)\}").unwrap();

        re.replace_all(template, |caps: &regex::Captures| {
            let key = &caps[1];
            match inputs.get(key).and_then(|v| v.as_str()) {
                Some(resolved) => resolved.to_string(),
                None => caps[0].to_string(),
            }
        })
        .to_string()
    }

    /// Recursively resolve template strings in a Value
    fn resolve_value_templates(value: &Value, inputs: &HashMap<String, Value>) -> Value {
        match value {
            Value::String(s) => Value::String(Self::resolve_template_string(s, inputs)),
            Value::Object(obj) => {
                let mut resolved = serde_json::Map::new();
                for (k, v) in obj {
                    resolved.insert(k.clone(), Self::resolve_value_templates(v, inputs));
                }
                Value::Object(resolved)
            }
            Value::Array(arr) => {
                let resolved: Vec<Value> = arr
                    .iter()
                    .map(|v| Self::resolve_value_templates(v, inputs))
                    .collect();
                Value::Array(resolved)
            }
            _ => value.clone(),
        }
    }
    /// Create a new executor with the given node registry and tool configurations.
    ///
    /// Call [`with_secure_values`](Self::with_secure_values) afterward if any tool uses
    /// `"secure": true` in its `fixed_config` (OAuth tokens, API keys, etc.).
    pub fn new(
        registry: Arc<dyn NodeRegistryPort>,
        tool_configurations: HashMap<String, ToolConfiguration>,
    ) -> Self {
        Self {
            registry,
            tool_configurations,
            secure_value_service: None,
            session_id: None,
            agent_session_id: None,
            skill_repository: None,
            skill_observer: None,
            documents_context: None,
            crdt_docs_context: None,
            describe_tool_lookup: None,
            describe_tool_observer: None,
            attachment_catalog: None,
            max_tool_result_bytes: DEFAULT_MAX_TOOL_RESULT_STRING_BYTES,
        }
    }

    /// Builder: override the per-string cap applied to tool results.
    /// Increase for tools that legitimately return large text payloads.
    pub fn with_max_tool_result_bytes(mut self, max: usize) -> Self {
        self.max_tool_result_bytes = max;
        self
    }

    /// Builder: attach a SecureValueService + session_id for secret injection.
    pub fn with_secure_values(
        mut self,
        secure_value_service: Arc<SecureValueService>,
        session_id: String,
    ) -> Self {
        self.secure_value_service = Some(secure_value_service);
        self.session_id = Some(session_id);
        self
    }

    /// Builder: set the session_id without attaching a SecureValueService.
    ///
    /// This is useful when the engine has a session context but no secrets
    /// store configured. `__colmena_session_id` will still be injected into
    /// every tool's `inputs` map so that nodes like `secure_suspend` can
    /// find it on the resume path.
    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Builder: attach the agent_session_id (chat handle). When set, every
    /// tool dispatch will inject `__colmena_agent_session_id` into the node's
    /// inputs and secret lookups become agent-first with session fallback.
    pub fn with_agent_session_id(mut self, agent_session_id: Option<String>) -> Self {
        self.agent_session_id = agent_session_id;
        self
    }

    /// Attach a SkillRepository so `load_skill` tool calls are handled.
    pub fn with_skills(
        mut self,
        repository: Arc<dyn crate::skills::domain::SkillRepository>,
    ) -> Self {
        self.skill_repository = Some(repository);
        self
    }

    /// Attach an observer callback that fires after a successful `load_skill` dispatch.
    pub fn with_skill_observer(mut self, cb: SkillObserver) -> Self {
        self.skill_observer = Some(cb);
        self
    }

    /// Attach a `DocumentToolsContext` so the seven `document_*` synthetic
    /// tool calls dispatch to the document runtime.
    pub fn with_documents(
        mut self,
        ctx: Arc<
            crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::DocumentToolsContext,
        >,
    ) -> Self {
        self.documents_context = Some(ctx);
        self
    }

    /// Attach a `CrdtDocsContext` so the five `crdt_doc_*` synthetic tool
    /// calls dispatch to the v1 crdt_documents runtime.
    pub fn with_crdt_documents(
        mut self,
        ctx: Arc<
            crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::CrdtDocsContext,
        >,
    ) -> Self {
        self.crdt_docs_context = Some(ctx);
        self
    }

    /// Attach a snapshot of `ToolConfiguration` entries so `describe_tool`
    /// calls can be intercepted and resolved against this lookup.
    pub fn with_describe_tool_lookup(mut self, lookup: Vec<ToolConfiguration>) -> Self {
        self.describe_tool_lookup = Some(lookup);
        self
    }

    /// Attach an observer callback that fires after a successful
    /// `describe_tool` dispatch.
    pub fn with_describe_tool_observer(mut self, cb: ToolDescribeObserver) -> Self {
        self.describe_tool_observer = Some(cb);
        self
    }

    /// Attach a snapshot of available attachments for `load_attachment`
    /// interception. Passing an empty slice has the same effect as not
    /// calling this method (the tool dispatch will report no rows).
    pub fn with_attachments(
        mut self,
        catalog: Vec<crate::llm::domain::ConversationAttachment>,
    ) -> Self {
        self.attachment_catalog = Some(catalog);
        self
    }

    /// Recursively scan fixed_config for all "$DYNAMIC" placeholders.
    /// Returns Vec of (param_name, container_field) tuples.
    /// - For nested: (field_key, Some(container_key)) e.g. ("title", Some("body"))
    /// - For top-level: (container_key, None) e.g. ("endpoint", None)
    fn collect_dynamic_fields(
        fixed_config: &HashMap<String, Value>,
    ) -> Vec<(String, Option<String>)> {
        let mut dynamic_fields = Vec::new();

        for (container_key, container_val) in fixed_config {
            match container_val {
                // Top-level $DYNAMIC string
                Value::String(s) if s == DYNAMIC_PLACEHOLDER => {
                    dynamic_fields.push((container_key.clone(), None));
                }
                // Object container (e.g., body, headers, query_params)
                Value::Object(obj) => {
                    for (field_key, field_val) in obj {
                        if field_val.as_str() == Some(DYNAMIC_PLACEHOLDER) {
                            dynamic_fields.push((field_key.clone(), Some(container_key.clone())));
                        }
                    }
                }
                // Other fixed values are ignored (not dynamic)
                _ => {}
            }
        }

        dynamic_fields
    }

    async fn execute_toolkit(
        &self,
        alias: &str,
        sub_tool: &str,
        cfg: &ToolConfiguration,
        tool_call: &crate::llm::domain::ToolCall,
    ) -> Result<crate::llm::domain::ToolResult, crate::llm::domain::LlmError> {
        use crate::dag_engine::domain::toolkit_node::SUB_TOOL_INPUT_KEY;
        use crate::llm::domain::{LlmError, ToolResult};

        // Resolve the toolkit node.
        let toolkit = self
            .registry
            .get_toolkit_node(&cfg.node_type)
            .ok_or_else(|| LlmError::ToolNotFound {
                name: cfg.node_type.clone(),
            })?;

        // Confirm this sub-tool is actually in the filter / catalogue.
        let node_cfg = cfg
            .node_config
            .clone()
            .unwrap_or_else(|| Value::Object(Default::default()));
        let catalog = toolkit.sub_tool_catalog(&node_cfg);
        let known = catalog.iter().any(|d| d.name.as_ref() == sub_tool);
        let exposed = cfg
            .expose_sub_tools
            .as_ref()
            .map(|f| f.includes(sub_tool))
            .unwrap_or(false);
        if !known || !exposed {
            return Ok(ToolResult {
                tool_call_id: tool_call.id.clone(),
                success: false,
                output: format!("unknown sub-tool '{}' for toolkit '{}'", sub_tool, alias),
                error: Some("unknown sub-tool".to_string()),
            });
        }

        // Parse LLM arguments.
        let mut inputs: HashMap<String, Value> =
            serde_json::from_str(&tool_call.function.arguments).map_err(|e| {
                LlmError::InvalidToolCall {
                    reason: format!(
                        "Failed to parse arguments for {}: {}",
                        tool_call.function.name, e
                    ),
                }
            })?;

        // Inject the reserved sub-tool discriminator.
        inputs.insert(
            SUB_TOOL_INPUT_KEY.to_string(),
            Value::String(sub_tool.to_string()),
        );

        // Execute the underlying toolkit node as a plain ExecutableNode.
        // node_exec_config is the per-toolkit static node_config from the entry
        // (e.g. { "api_key": "..." }).
        let exec_node =
            self.registry
                .get_node(&cfg.node_type)
                .ok_or_else(|| LlmError::ToolNotFound {
                    name: cfg.node_type.clone(),
                })?;

        let mut state = serde_json::json!({});
        let result = exec_node
            .execute(&inputs, &node_cfg, &mut state, None)
            .await;

        match result {
            Ok(value) => Ok(ToolResult {
                tool_call_id: tool_call.id.clone(),
                success: true,
                output: value.to_string(),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                tool_call_id: tool_call.id.clone(),
                success: false,
                output: format!("Error executing toolkit {}__{}: {}", alias, sub_tool, e),
                error: Some(e.to_string()),
            }),
        }
    }

    /// Generate ToolDefinition from node with partial configuration
    #[allow(deprecated)]
    fn generate_tool_definition(
        &self,
        tool_name: &str,
        tool_config: &ToolConfiguration,
        node: &Arc<dyn ExecutableNode>,
    ) -> crate::llm::domain::ToolDefinition {
        use crate::dag_engine::domain::tool_configuration::parse_node_schema;
        use crate::llm::domain::{ParameterProperty, ToolDefinition, ToolParameters};

        // Use tool_config.name if non-empty (e.g. when the map key is a UUID from the frontend),
        // otherwise fall back to the map key so existing graphs are unaffected.
        let effective_name = if !tool_config.name.is_empty() {
            tool_config.name.as_str()
        } else {
            tool_name
        };

        // BRANCH 0 (HIGHEST PRIORITY): node_schema
        if let Some(schema) = &tool_config.node_schema {
            let parsed = parse_node_schema(schema).unwrap_or_else(|e| {
                panic!(
                    "Invalid node_schema for tool '{}': {}\nFix the graph configuration and re-run.",
                    effective_name, e
                )
            });
            return ToolDefinition {
                name: effective_name.to_string(),
                description: tool_config.description.clone(),
                parameters: ToolParameters {
                    schema_type: "object".to_string(),
                    properties: parsed.llm_properties,
                    required: parsed.required_params,
                },
                input_schema_override: None,
            };
        }

        // If parameters are explicitly defined in config, use them
        if let Some(params_value) = &tool_config.parameters {
            if let Ok(params) = serde_json::from_value::<ToolParameters>(params_value.clone()) {
                return ToolDefinition {
                    name: effective_name.to_string(),
                    description: tool_config.description.clone(),
                    parameters: params,
                    input_schema_override: None,
                };
            } else {
                colmena_log!(
                    "WARN: Failed to parse custom parameters for tool {}",
                    effective_name
                );
                // Fallback to default generation? or error?
                // Let's fallback but maybe log.
            }
        }

        // Check for $DYNAMIC placeholders in fixed_config
        // If any are found, derive parameters from them (new $DYNAMIC system)
        let dynamic_fields = Self::collect_dynamic_fields(&tool_config.fixed_config);
        if !dynamic_fields.is_empty() {
            let mut properties = HashMap::new();
            let mut required = Vec::new();

            for (param_name, container) in &dynamic_fields {
                let description = match container {
                    Some(c) => format!("Value for {}.{}", c, param_name),
                    None => format!("Value for {}", param_name),
                };
                properties.insert(
                    param_name.clone(),
                    ParameterProperty::new("string".to_string(), description),
                );
                required.push(param_name.clone());
            }

            return ToolDefinition {
                name: effective_name.to_string(),
                description: if !tool_config.description.is_empty() {
                    tool_config.description.clone()
                } else {
                    node.description()
                        .unwrap_or("No description available")
                        .to_string()
                },
                parameters: ToolParameters {
                    schema_type: "object".to_string(),
                    properties,
                    required,
                },
                input_schema_override: None,
            };
        }

        let node_schema = node.schema();
        let inputs_schema = node_schema
            .get("inputs")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        // Filter out inputs that are in fixed_config
        let mut exposed_properties = HashMap::new();
        let mut required = Vec::new(); // We need to determine required fields dynamically

        for (key, value) in inputs_schema {
            // Skip if in fixed_config
            if tool_config.fixed_config.contains_key(&key) {
                continue;
            }

            // Skip if not in exposed_inputs (when specified)
            if let Some(ref exposed) = tool_config.exposed_inputs {
                if !exposed.contains(&key) {
                    continue;
                }
            }

            // Parse the schema value into ParameterProperty
            // We reuse the logic from available_tools but adapted
            let desc = value.as_str().unwrap_or("");
            let (prop_type, is_optional) = if desc.contains("number") {
                ("number", desc.contains("optional"))
            } else if desc.contains("integer") {
                ("integer", desc.contains("optional"))
            } else if desc.contains("boolean") {
                ("boolean", desc.contains("optional"))
            } else {
                ("string", desc.contains("optional"))
            };

            exposed_properties.insert(
                key.clone(),
                ParameterProperty::new(prop_type.to_string(), desc.to_string()),
            );

            if !is_optional {
                required.push(key.clone());
            }
        }

        // Use custom description or fall back to node description
        let description = if !tool_config.description.is_empty() {
            tool_config.description.clone()
        } else {
            node.description()
                .unwrap_or("No description available")
                .to_string()
        };

        ToolDefinition {
            name: effective_name.to_string(),
            description,
            parameters: ToolParameters {
                schema_type: "object".to_string(),
                properties: exposed_properties,
                required,
            },
            input_schema_override: None,
        }
    }
}

impl DagToolExecutor {
    /// Execute a tool call and inject `__colmena_resume_answer` into the node's
    /// inputs so a previously-suspended tool receives the user's answer.
    ///
    /// Behaves identically to [`ToolExecutor::execute`] in every other respect.
    pub async fn execute_with_resume_answer(
        &self,
        tool_call: &ToolCall,
        resume_answer: &str,
    ) -> Result<ToolResult, LlmError> {
        self.execute_inner(tool_call, Some(resume_answer)).await
    }

    /// Shared body for [`ToolExecutor::execute`] and [`execute_with_resume_answer`].
    ///
    /// `resume_answer` is inserted into the final `inputs` map under the key
    /// `__colmena_resume_answer` **after** all fixed/dynamic merging and **before**
    /// `inject_secrets` runs, so secrets resolution still applies uniformly.
    #[allow(deprecated)]
    async fn execute_inner(
        &self,
        tool_call: &ToolCall,
        resume_answer: Option<&str>,
    ) -> Result<ToolResult, LlmError> {
        use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
            describe_tool_into_tool_result, dispatch_describe_tool, dispatch_load_skill,
            into_tool_result, DESCRIBE_TOOL_NAME, LOAD_SKILL_TOOL_NAME,
        };

        if tool_call.function.name == LOAD_SKILL_TOOL_NAME {
            let repo = self
                .skill_repository
                .as_ref()
                .ok_or_else(|| LlmError::ToolNotFound {
                    name: LOAD_SKILL_TOOL_NAME.to_string(),
                })?;
            let result = dispatch_load_skill(tool_call, repo).await?;
            if let Some(obs) = &self.skill_observer {
                obs(&result);
            }
            return Ok(into_tool_result(&tool_call.id, &result));
        }

        if tool_call.function.name == DESCRIBE_TOOL_NAME {
            let lookup =
                self.describe_tool_lookup
                    .as_ref()
                    .ok_or_else(|| LlmError::ToolNotFound {
                        name: DESCRIBE_TOOL_NAME.to_string(),
                    })?;
            let result = dispatch_describe_tool(tool_call, lookup).await?;
            if let Some(obs) = &self.describe_tool_observer {
                obs(&result);
            }
            return Ok(describe_tool_into_tool_result(&tool_call.id, &result));
        }

        // --- Synthetic load_attachment ---
        {
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
                dispatch_load_attachment, LOAD_ATTACHMENT_TOOL_NAME,
            };
            if tool_call.function.name == LOAD_ATTACHMENT_TOOL_NAME {
                let empty: Vec<crate::llm::domain::ConversationAttachment> = Vec::new();
                let catalog = self.attachment_catalog.as_ref().unwrap_or(&empty);
                return dispatch_load_attachment(tool_call, catalog);
            }
        }

        // --- Synthetic document tools (document_create, document_edit, etc.) ---
        if let Some(ctx) = self.documents_context.as_ref() {
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
                dispatch_document_apply_patch, dispatch_document_create,
                dispatch_document_get_head, dispatch_document_list_my_artifacts,
                dispatch_document_list_versions, dispatch_document_read,
                dispatch_document_rollback, DOCUMENT_APPLY_PATCH_TOOL, DOCUMENT_CREATE_TOOL,
                DOCUMENT_GET_HEAD_TOOL, DOCUMENT_LIST_MY_ARTIFACTS_TOOL,
                DOCUMENT_LIST_VERSIONS_TOOL, DOCUMENT_READ_TOOL, DOCUMENT_ROLLBACK_TOOL,
            };

            let name = tool_call.function.name.as_str();
            let is_doc_tool = matches!(
                name,
                n if n == DOCUMENT_CREATE_TOOL
                    || n == DOCUMENT_APPLY_PATCH_TOOL
                    || n == DOCUMENT_READ_TOOL
                    || n == DOCUMENT_GET_HEAD_TOOL
                    || n == DOCUMENT_LIST_VERSIONS_TOOL
                    || n == DOCUMENT_ROLLBACK_TOOL
                    || n == DOCUMENT_LIST_MY_ARTIFACTS_TOOL
            );

            if is_doc_tool {
                let args: serde_json::Value = if tool_call.function.arguments.trim().is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(&tool_call.function.arguments).map_err(|e| {
                        LlmError::InvalidToolCall {
                            reason: format!("Failed to parse arguments for tool {}: {}", name, e),
                        }
                    })?
                };

                let result = match name {
                    n if n == DOCUMENT_CREATE_TOOL => dispatch_document_create(ctx, args).await,
                    n if n == DOCUMENT_APPLY_PATCH_TOOL => {
                        dispatch_document_apply_patch(ctx, args).await
                    }
                    n if n == DOCUMENT_READ_TOOL => dispatch_document_read(ctx, args).await,
                    n if n == DOCUMENT_GET_HEAD_TOOL => dispatch_document_get_head(ctx, args).await,
                    n if n == DOCUMENT_LIST_VERSIONS_TOOL => {
                        dispatch_document_list_versions(ctx, args).await
                    }
                    n if n == DOCUMENT_ROLLBACK_TOOL => dispatch_document_rollback(ctx, args).await,
                    _ => dispatch_document_list_my_artifacts(ctx, args).await,
                };

                let success =
                    !matches!(&result, serde_json::Value::Object(m) if m.contains_key("error"));
                return Ok(crate::llm::domain::ToolResult {
                    tool_call_id: tool_call.id.clone(),
                    output: result.to_string(),
                    success,
                    error: None,
                });
            }
        }

        // --- Synthetic CRDT documents tools (crdt_doc_*) ---
        if let Some(ctx) = self.crdt_docs_context.as_ref() {
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
                dispatch_crdt_doc_add_sheet, dispatch_crdt_doc_list_sheets,
                dispatch_crdt_doc_read, dispatch_crdt_doc_set_cell, dispatch_crdt_doc_set_range,
                CRDT_DOC_ADD_SHEET_TOOL, CRDT_DOC_LIST_SHEETS_TOOL, CRDT_DOC_READ_TOOL,
                CRDT_DOC_SET_CELL_TOOL, CRDT_DOC_SET_RANGE_TOOL,
            };

            let name = tool_call.function.name.as_str();
            let is_crdt_tool = matches!(
                name,
                n if n == CRDT_DOC_LIST_SHEETS_TOOL
                    || n == CRDT_DOC_READ_TOOL
                    || n == CRDT_DOC_SET_CELL_TOOL
                    || n == CRDT_DOC_SET_RANGE_TOOL
                    || n == CRDT_DOC_ADD_SHEET_TOOL
            );

            if is_crdt_tool {
                let args: serde_json::Value = if tool_call.function.arguments.trim().is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(&tool_call.function.arguments).map_err(|e| {
                        LlmError::InvalidToolCall {
                            reason: format!("Failed to parse arguments for tool {}: {}", name, e),
                        }
                    })?
                };

                let result = match name {
                    n if n == CRDT_DOC_LIST_SHEETS_TOOL => {
                        dispatch_crdt_doc_list_sheets(ctx, args).await
                    }
                    n if n == CRDT_DOC_READ_TOOL => dispatch_crdt_doc_read(ctx, args).await,
                    n if n == CRDT_DOC_SET_CELL_TOOL => {
                        dispatch_crdt_doc_set_cell(ctx, args).await
                    }
                    n if n == CRDT_DOC_SET_RANGE_TOOL => {
                        dispatch_crdt_doc_set_range(ctx, args).await
                    }
                    _ => dispatch_crdt_doc_add_sheet(ctx, args).await,
                };

                let success =
                    !matches!(&result, serde_json::Value::Object(m) if m.contains_key("error"));
                return Ok(crate::llm::domain::ToolResult {
                    tool_call_id: tool_call.id.clone(),
                    output: result.to_string(),
                    success,
                    error: None,
                });
            }
        }

        // --- Toolkit dispatch: names of the form "<alias>__<sub_tool>" ---
        if let Some((alias, sub_tool)) = tool_call.function.name.split_once("__") {
            if let Some(cfg) = self.tool_configurations.get(alias) {
                if cfg.is_toolkit() {
                    return self.execute_toolkit(alias, sub_tool, cfg, tool_call).await;
                }
            }
            // Flag-only fallback for `api_explorer`: when no `tool_configurations`
            // entry exists, `available_tools()` still auto-exposes the
            // `api_explorer__*` sub-tools (see loop 2 in `available_tools`).
            // For dispatch to match, synthesise a default ToolConfiguration here
            // so `execute_toolkit` can route the call. Scoped strictly to
            // `api_explorer` — other toolkits (tavily_client, browser, etc.)
            // still require an explicit `tool_configurations` entry.
            if alias == "api_explorer" && self.registry.get_toolkit_node(alias).is_some() {
                let synth_cfg = synthesise_default_toolkit_config(alias);
                return self
                    .execute_toolkit(alias, sub_tool, &synth_cfg, tool_call)
                    .await;
            }
        }

        let node_type = &tool_call.function.name;

        // 1. Check if it's a configured tool or a raw node.
        //    First try by map key (fast path), then by config.name (handles UUID keys from frontend).
        let (node, fixed_config, tool_cfg) =
            if let Some(config) = self.tool_configurations.get(node_type) {
                let node = self.registry.get_node(&config.node_type).ok_or_else(|| {
                    LlmError::ToolNotFound {
                        name: config.node_type.clone(),
                    }
                })?;
                (node, Some(config.fixed_config.clone()), Some(config))
            } else if let Some(config) = self
                .tool_configurations
                .values()
                .find(|c| c.name == *node_type)
            {
                // Fallback: LLM used the semantic name but the map key is a UUID
                let node = self.registry.get_node(&config.node_type).ok_or_else(|| {
                    LlmError::ToolNotFound {
                        name: config.node_type.clone(),
                    }
                })?;
                (node, Some(config.fixed_config.clone()), Some(config))
            } else {
                let node =
                    self.registry
                        .get_node(node_type)
                        .ok_or_else(|| LlmError::ToolNotFound {
                            name: node_type.clone(),
                        })?;
                (node, None, None)
            };

        // 2. Parse arguments
        let args: HashMap<String, Value> = serde_json::from_str(&tool_call.function.arguments)
            .map_err(|e| LlmError::InvalidToolCall {
                reason: format!("Failed to parse arguments for tool {}: {}", node_type, e),
            })?;

        // 3. Build final_args with node_schema, $DYNAMIC substitution, or legacy field_mapping
        use crate::dag_engine::domain::tool_configuration::parse_node_schema;

        let inputs = if let Some(schema) = tool_cfg.and_then(|c| c.node_schema.as_ref()) {
            // PATH 0 (HIGHEST PRIORITY): node_schema
            let parsed = parse_node_schema(schema).map_err(|e| LlmError::InvalidToolCall {
                reason: format!("Invalid node_schema for tool {}: {}", node_type, e),
            })?;
            let mut result: HashMap<String, Value> = HashMap::new();

            // Seed with all fixed values (will be resolved later)
            for (k, v) in &parsed.fixed_values {
                result.insert(k.clone(), v.clone());
            }

            // Place each LLM arg in the correct location
            for (param_name, param_value) in &args {
                if let Some(container) = parsed.param_to_container.get(param_name) {
                    // Merge into container
                    let entry = result
                        .entry(container.clone())
                        .or_insert_with(|| Value::Object(serde_json::Map::new()));
                    if let Value::Object(map) = entry {
                        // Strip dot-prefix if present (collision-prefixed keys use
                        // "container.child" format, but the real key inside the container
                        // is just "child").
                        let real_key = if let Some(dot_pos) = param_name.find('.') {
                            &param_name[dot_pos + 1..]
                        } else {
                            param_name.as_str()
                        };

                        // Deep-merge: if the container already has a fixed object for this key
                        // (e.g., edge with {type, animated, environmentId}), merge the LLM-provided
                        // object into it rather than overwriting.
                        if let (Some(Value::Object(existing)), Value::Object(incoming)) =
                            (map.get(real_key), param_value)
                        {
                            let mut merged = existing.clone();
                            for (k, v) in incoming {
                                merged.insert(k.clone(), v.clone());
                            }
                            map.insert(real_key.to_string(), Value::Object(merged));
                        } else {
                            map.insert(real_key.to_string(), param_value.clone());
                        }
                    }
                } else {
                    // Top-level placement
                    result.insert(param_name.clone(), param_value.clone());
                }
            }

            // Resolve template variables in fixed values using the final inputs
            // We need to clone to avoid borrow checker issues
            let resolved_result = result
                .iter()
                .map(|(k, v)| (k.clone(), Self::resolve_value_templates(v, &result)))
                .collect::<HashMap<String, Value>>();

            resolved_result
        } else if let Some(fixed) = fixed_config.as_ref() {
            // Check if using new $DYNAMIC system
            let dynamic_fields = Self::collect_dynamic_fields(fixed);
            if !dynamic_fields.is_empty() {
                // New path: walk fixed_config, substitute $DYNAMIC with LLM values
                let mut result: HashMap<String, Value> = HashMap::new();

                for (container_key, container_val) in fixed {
                    match container_val {
                        // Top-level $DYNAMIC → substitute directly
                        Value::String(s) if s == DYNAMIC_PLACEHOLDER => {
                            if let Some(v) = args.get(container_key) {
                                result.insert(container_key.clone(), v.clone());
                            }
                            // if LLM didn't provide it, omit (will likely cause node error)
                        }
                        // Object container → rebuild with substitutions
                        Value::Object(obj) => {
                            let mut rebuilt = serde_json::Map::new();
                            for (field_key, field_val) in obj {
                                if field_val.as_str() == Some(DYNAMIC_PLACEHOLDER) {
                                    // Replace with LLM value (use field_key as param name)
                                    if let Some(v) = args.get(field_key) {
                                        rebuilt.insert(field_key.clone(), v.clone());
                                    }
                                    // if not provided, skip (field absent from request)
                                } else {
                                    // Fixed value: keep as-is
                                    rebuilt.insert(field_key.clone(), field_val.clone());
                                }
                            }
                            result.insert(container_key.clone(), Value::Object(rebuilt));
                        }
                        // Any other fixed value (string, number, bool) → keep as-is
                        _ => {
                            result.insert(container_key.clone(), container_val.clone());
                        }
                    }
                }

                result
            } else {
                // Old path: field_mapping + mergeable_fields (backward compatibility)
                let mut final_args: HashMap<String, Value> = HashMap::new();
                let mut remaining_args = args.clone();

                // Step A: Apply field_mapping
                if let Some(mapping) = tool_cfg.and_then(|c| c.field_mapping.as_ref()) {
                    for (param_name, dest_field) in mapping {
                        if let Some(value) = remaining_args.remove(param_name) {
                            let container = final_args
                                .entry(dest_field.clone())
                                .or_insert_with(|| Value::Object(serde_json::Map::new()));
                            if let Value::Object(map) = container {
                                map.insert(param_name.clone(), value);
                            }
                        }
                    }
                }

                // Remaining unmapped args go to top level
                for (k, v) in remaining_args {
                    final_args.insert(k, v);
                }

                // Step B: Merge/apply fixed_config
                let mergeable: &[String] = tool_cfg
                    .and_then(|c| c.mergeable_fields.as_deref())
                    .unwrap_or(&[]);

                for (k, fixed_val) in fixed {
                    if mergeable.contains(k) {
                        // Merge: fixed is the base, dynamic is the overlay
                        match (fixed_val, final_args.get(k)) {
                            (Value::Object(fixed_obj), Some(Value::Object(dyn_obj))) => {
                                let mut merged = fixed_obj.clone();
                                for (dk, dv) in dyn_obj {
                                    merged.insert(dk.clone(), dv.clone());
                                }
                                final_args.insert(k.clone(), Value::Object(merged));
                            }
                            // fixed is object but no dynamic counterpart → use fixed as-is
                            (_, None) => {
                                final_args.insert(k.clone(), fixed_val.clone());
                            }
                            // non-object types: dynamic already in final_args, fixed ignored
                            _ => {}
                        }
                    } else {
                        // Non-mergeable: always apply fixed
                        final_args.insert(k.clone(), fixed_val.clone());
                    }
                }

                final_args
            }
        } else {
            // No fixed_config: just use args as-is
            args
        };

        // Inject the resume answer AFTER all merging but BEFORE inject_secrets so that
        // secret resolution still applies uniformly and the key cannot be overridden by
        // anything in fixed_config or the LLM arguments.
        let mut inputs = inputs;
        if let Some(ans) = resume_answer {
            inputs.insert(
                "__colmena_resume_answer".to_string(),
                Value::String(ans.to_string()),
            );
        }

        // Always inject __colmena_session_id so every tool dispatch — including
        // secure_suspend on its resume path — can find the session without relying
        // on the caller to pass it through fixed_config or LLM arguments.
        // The engine's value is authoritative and overwrites any caller-supplied one.
        if let Some(sid) = &self.session_id {
            inputs.insert(
                "__colmena_session_id".to_string(),
                Value::String(sid.clone()),
            );
        }
        if let Some(asid) = &self.agent_session_id {
            inputs.insert(
                "__colmena_agent_session_id".to_string(),
                Value::String(asid.clone()),
            );
        }

        // Convert HashMap to NodeInputs (which is just HashMap<String, Value>)
        // SECURE VALUES: decrypt <value_N> placeholders before sending to the node.
        // The applied map `(decrypted_value → handle)` will be used by the outbound
        // masker (Task 11) to rewrite real values back to handles in tool responses.
        let mut applied_secrets: HashMap<String, String> = HashMap::new();
        let inputs = if let (Some(svc), Some(sid)) = (&self.secure_value_service, &self.session_id)
        {
            let mut inputs_val =
                serde_json::to_value(&inputs).unwrap_or(Value::Object(Default::default()));
            applied_secrets = match svc
                .inject_secrets(&mut inputs_val, sid, self.agent_session_id.as_deref())
                .await
            {
                Ok(map) => map,
                Err(e) => {
                    eprintln!("⚠️ [DagToolExecutor] Failed to inject secrets: {}", e);
                    Default::default()
                }
            };
            serde_json::from_value::<HashMap<String, Value>>(inputs_val).unwrap_or(inputs)
        } else {
            inputs
        };

        // fixed_config values are already merged into `inputs` by the logic above.
        // Do NOT pass fixed_config as node config: HttpNode would double-process headers/body
        // causing conflicts (e.g., duplicate Content-Type → Amadeus 400).
        let node_exec_config = serde_json::json!({});

        // Read the secure flag directly from tool_cfg — no need to pass it via config.
        let is_secure = tool_cfg
            .and_then(|c| c.fixed_config.get("secure"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut state = serde_json::json!({});

        let result = node
            .execute(&inputs, &node_exec_config, &mut state, None)
            .await;

        // SECURE VALUES (Task 11): mask decrypted secrets back to their handles
        // before any downstream handling. This runs unconditionally (independent of
        // `is_secure`) and on BOTH Ok and Err paths so error messages cannot leak
        // a secret either. Must precede `hash_output` so the masker sees raw values.
        let result = if let Some(svc) = &self.secure_value_service {
            match result {
                Ok(mut value) => {
                    svc.mask_outbound(&mut value, &applied_secrets);
                    Ok(value)
                }
                Err(e) => {
                    let mut err_value = Value::String(e.to_string());
                    svc.mask_outbound(&mut err_value, &applied_secrets);
                    let masked_msg = err_value.as_str().unwrap_or("").to_string();
                    Err(Box::<dyn std::error::Error + Send + Sync>::from(masked_msg))
                }
            }
        } else {
            result
        };

        // 4. Apply Secure Value hashing BEFORE returning to LLM
        // This is the critical step: if the tool has `secure: true`, all sensitive
        // values in the response are replaced with <value_N> placeholders so the
        // LLM never sees the real secret. Real values are encrypted in the DB.
        match result {
            Ok(value) => {
                let safe_output = if is_secure {
                    if let (Some(svc), Some(sid)) = (&self.secure_value_service, &self.session_id) {
                        let secure_config = serde_json::json!({ "secure": true });
                        match svc
                            .hash_output(
                                &value,
                                &secure_config,
                                sid,
                                self.agent_session_id.as_deref(),
                                node_type,
                            )
                            .await
                        {
                            Ok(hashed) => {
                                colmena_log!("🔒 [DagToolExecutor] Secure tool '{}': output hashed, real values encrypted in DB", node_type);
                                hashed
                            }
                            Err(e) => {
                                eprintln!(
                                    "⚠️ [DagToolExecutor] hash_output failed for '{}': {}",
                                    node_type, e
                                );
                                value // fallback: return as-is (still better than crashing)
                            }
                        }
                    } else {
                        eprintln!("⚠️ [DagToolExecutor] Tool '{}' has secure:true but no SecureValueService attached. Token WILL be visible to LLM.", node_type);
                        value
                    }
                } else {
                    value
                };

                Ok(ToolResult {
                    tool_call_id: tool_call.id.clone(),
                    success: true,
                    output: safe_output.to_string(),
                    error: None,
                })
            }
            Err(e) => Ok(ToolResult {
                tool_call_id: tool_call.id.clone(),
                success: false,
                output: format!("Error executing node {}: {}", node_type, e),
                error: Some(e.to_string()),
            }),
        }
    }
}

impl DagToolExecutor {
    /// Walk a JSON value and scrub anything that would bloat the LLM context:
    /// - Strings starting with `data:<mime>;base64,...` → replaced with a
    ///   compact marker. Binary base64 in the LLM context is always a footgun
    ///   (megabytes of useless tokens, TPM rate-limit risk), so this is
    ///   always-on regardless of `max_string_bytes`.
    /// - Strings whose byte length exceeds `max_string_bytes` → replaced with
    ///   `[truncated: original_size=N bytes]`.
    ///
    /// Returns the cleaned value. Other types pass through unchanged.
    fn scrub_value_for_llm(value: Value, max_string_bytes: usize) -> Value {
        match value {
            Value::String(s) => {
                // Catch data: URIs (any mime, any encoding) — these only ever
                // make sense when the consumer is a renderer, never an LLM.
                if let Some(rest) = s.strip_prefix("data:") {
                    if let Some(semi) = rest.find(";base64,") {
                        let mime = &rest[..semi];
                        let payload_len = rest.len() - semi - ";base64,".len();
                        return Value::String(format!(
                            "[binary elided: mime={mime}, encoded_size={payload_len} bytes]"
                        ));
                    }
                }
                if s.len() > max_string_bytes {
                    Value::String(format!(
                        "[truncated: original_size={} bytes (cap={} bytes); request via load_attachment if needed]",
                        s.len(),
                        max_string_bytes
                    ))
                } else {
                    Value::String(s)
                }
            }
            Value::Object(obj) => Value::Object(
                obj.into_iter()
                    .map(|(k, v)| (k, Self::scrub_value_for_llm(v, max_string_bytes)))
                    .collect(),
            ),
            Value::Array(arr) => Value::Array(
                arr.into_iter()
                    .map(|v| Self::scrub_value_for_llm(v, max_string_bytes))
                    .collect(),
            ),
            other => other,
        }
    }

    /// Apply [`scrub_value_for_llm`](Self::scrub_value_for_llm) to a JSON-or-text
    /// `output` string. Falls back to length-only truncation when the output
    /// is not valid JSON.
    fn scrub_tool_result_output(output: String, max_string_bytes: usize) -> String {
        match serde_json::from_str::<Value>(&output) {
            Ok(value) => {
                let scrubbed = Self::scrub_value_for_llm(value, max_string_bytes);
                serde_json::to_string(&scrubbed).unwrap_or(output)
            }
            Err(_) => {
                if output.len() > max_string_bytes {
                    format!(
                        "[truncated: original_size={} bytes (cap={} bytes)]",
                        output.len(),
                        max_string_bytes
                    )
                } else {
                    output
                }
            }
        }
    }
}

#[async_trait]
impl ToolExecutor for DagToolExecutor {
    async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, LlmError> {
        let mut result = self.execute_inner(tool_call, None).await?;
        // Scrub binary / oversized strings from the tool result before it
        // reaches the LLM. Keeps the LLM context free of raw bytes by design.
        result.output = Self::scrub_tool_result_output(result.output, self.max_tool_result_bytes);
        Ok(result)
    }

    async fn available_tools(&self) -> Vec<crate::llm::domain::ToolDefinition> {
        use crate::llm::domain::{ParameterProperty, ToolDefinition, ToolParameters};

        let nodes = self.registry.get_all_nodes();
        let mut tools = Vec::new();

        // 1. Add configured tools first
        for (name, config) in &self.tool_configurations {
            if config.is_toolkit() {
                // Toolkit: expand one ToolDefinition per declared sub-tool.
                // Unlike the non-toolkit branch (which silently skips on miss), an
                // unknown toolkit node_type is almost always a user misconfiguration
                // worth surfacing — the alias exists but no handler is registered.
                let Some(toolkit) = self.registry.get_toolkit_node(&config.node_type) else {
                    colmena_log!(
                        "WARN: toolkit config '{}' references unknown toolkit node_type '{}'",
                        name,
                        config.node_type
                    );
                    continue;
                };
                let node_cfg = config
                    .node_config
                    .clone()
                    .unwrap_or_else(|| Value::Object(Default::default()));
                let catalog = toolkit.sub_tool_catalog(&node_cfg);
                let filter = config
                    .expose_sub_tools
                    .as_ref()
                    .expect("is_toolkit → filter present");
                for sub in catalog {
                    if !filter.includes(&sub.name) {
                        continue;
                    }
                    tools.push(crate::llm::domain::ToolDefinition {
                        name: format!("{}__{}", name, sub.name),
                        description: sub.description,
                        parameters: crate::llm::domain::ToolParameters {
                            schema_type: "object".to_string(),
                            properties: sub.properties,
                            required: sub.required,
                        },
                        input_schema_override: None,
                    });
                }
            } else if let Some(node) = self.registry.get_node(&config.node_type) {
                tools.push(self.generate_tool_definition(name, config, &node));
            }
        }

        // 2. Add raw nodes (if not already added as configured tool with same name)
        // Note: If a configured tool has same name as a node, the configured tool takes precedence in the list above.
        // But here we are iterating over all nodes.
        // If we want to expose raw nodes ONLY if they are not configured, we should check.
        // However, usually configured tools have different names (e.g. "fetch_users" vs "http_call").

        for (name, node) in nodes {
            // Skip internal nodes or nodes that shouldn't be tools
            if name == "llm_call" || name == "mock_input" || name == "log" {
                continue;
            }

            // Document nodes are exposed via richer synthetic tools. Skipping
            // the raw-node auto-discovery here avoids name collisions with the
            // schemars-derived definitions injected at the LLM node level.
            if name.starts_with("document_") {
                continue;
            }

            // Skip if this node name is already used by a configured tool?
            // Or maybe we allow both "http_call" (raw) and "fetch_users" (configured)?
            // Let's allow both for now, unless the configured tool explicitly uses the node name.
            if self.tool_configurations.contains_key(&name) {
                continue;
            }

            // Special case: auto-expand `api_explorer` into its sub-tool catalog
            // even without an explicit `tool_configurations` entry, so the LLM
            // sees `api_explorer__load_spec`, `api_explorer__search_endpoint`,
            // etc. rather than a single opaque `api_explorer` raw tool. Any
            // explicit `tool_configurations` entry above takes precedence
            // (handled by the `contains_key` guard immediately above).
            if name == "api_explorer" {
                if let Some(toolkit) = self.registry.get_toolkit_node(&name) {
                    let node_cfg = Value::Object(Default::default());
                    let catalog = toolkit.sub_tool_catalog(&node_cfg);
                    for sub in catalog {
                        tools.push(crate::llm::domain::ToolDefinition {
                            name: format!("{}__{}", name, sub.name),
                            description: sub.description,
                            parameters: crate::llm::domain::ToolParameters {
                                schema_type: "object".to_string(),
                                properties: sub.properties,
                                required: sub.required,
                            },
                            input_schema_override: None,
                        });
                    }
                    continue;
                }
            }

            let schema = node.schema();

            // Convert node schema to ToolDefinition
            // Node schema: { "type": "...", "config": {...}, "inputs": {...}, "outputs": {...} }
            // ToolDefinition needs: name, description, parameters (JSON Schema)

            // We use "inputs" as parameters.
            let inputs_schema = schema
                .get("inputs")
                .cloned()
                .unwrap_or(serde_json::json!({}));

            // Convert inputs schema to ToolParameters
            // Simple conversion: treat all inputs as string/optional for now,
            // or try to infer type from description string in schema?
            // The schema in ExecutableNode returns "type description" strings like "string (optional)".

            let mut properties = HashMap::new();
            let mut required = Vec::new();

            if let Some(inputs_obj) = inputs_schema.as_object() {
                for (key, desc_val) in inputs_obj {
                    let desc = desc_val.as_str().unwrap_or("");
                    let (prop_type, is_optional) = if desc.contains("number") {
                        ("number", desc.contains("optional"))
                    } else if desc.contains("integer") {
                        ("integer", desc.contains("optional"))
                    } else if desc.contains("boolean") {
                        ("boolean", desc.contains("optional"))
                    } else {
                        ("string", desc.contains("optional"))
                    };

                    properties.insert(
                        key.clone(),
                        ParameterProperty::new(prop_type.to_string(), desc.to_string()),
                    );

                    if !is_optional {
                        required.push(key.clone());
                    }
                }
            }

            tools.push(ToolDefinition {
                name: name.clone(),
                description: node
                    .description()
                    .unwrap_or(&format!("Execute node: {}", name))
                    .to_string(),
                parameters: ToolParameters {
                    schema_type: "object".to_string(),
                    properties,
                    required,
                },
                input_schema_override: None,
            });
        }

        tools
    }
}

/// Build a default `ToolConfiguration` for a toolkit alias that was advertised
/// to the LLM via flag-only auto-exposure (no explicit `tool_configurations`
/// entry). Used by the dispatch fallback in `execute_inner` so
/// `execute_toolkit` has the shape it expects.
///
/// The synthesised config sets:
/// - `node_type == alias` (toolkit nodes are registered under the alias name).
/// - `expose_sub_tools = SubToolFilter::All` so the filter inside
///   `execute_toolkit` does not reject the call.
/// - Empty `fixed_config` and absent `node_config` — the toolkit runs with its
///   own defaults, just as it does in the auto-exposed catalog path.
#[allow(deprecated)] // Synthesises legacy fields with defaults for backward compatibility.
fn synthesise_default_toolkit_config(alias: &str) -> ToolConfiguration {
    use crate::dag_engine::domain::tool_configuration::SubToolFilter;
    ToolConfiguration {
        name: alias.to_string(),
        description: String::new(),
        node_type: alias.to_string(),
        fixed_config: HashMap::new(),
        exposed_inputs: None,
        parameters: None,
        mergeable_fields: None,
        field_mapping: None,
        node_schema: None,
        node_config: None,
        expose_sub_tools: Some(SubToolFilter::all()),
        summary: None,
        eager: false,
    }
}

#[cfg(test)]
#[allow(deprecated)] // Tests intentionally exercise legacy ToolConfiguration fields for backward-compat coverage.
mod tests {
    use super::*;
    use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
    use crate::llm::domain::{FunctionCall, LlmError, ToolCall};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::Arc;

    // Mock Node
    struct MockNode {
        name: String,
    }

    #[async_trait]
    impl ExecutableNode for MockNode {
        async fn execute(
            &self,
            inputs: &NodeInputs,
            _config: &Value,
            _state: &mut Value,
            _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
        ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
            // Echo inputs
            Ok(serde_json::to_value(inputs)?)
        }

        fn schema(&self) -> Value {
            serde_json::json!({
                "type": self.name,
                "inputs": {
                    "a": "string (optional)"
                }
            })
        }
    }

    // Mock Registry
    struct MockRegistry {
        nodes: HashMap<String, Arc<dyn ExecutableNode>>,
    }

    impl MockRegistry {
        fn new() -> Self {
            let mut nodes = HashMap::new();
            nodes.insert(
                "mock_tool".to_string(),
                Arc::new(MockNode {
                    name: "mock_tool".to_string(),
                }),
            );
            Self {
                nodes: nodes
                    .into_iter()
                    .map(|(k, v)| (k, v as Arc<dyn ExecutableNode>))
                    .collect(),
            }
        }
    }

    impl NodeRegistryPort for MockRegistry {
        fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
            self.nodes.get(node_type).cloned()
        }

        fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
            self.nodes.clone()
        }
    }

    #[tokio::test]
    async fn test_execute_success() {
        let registry = Arc::new(MockRegistry::new());
        let executor = DagToolExecutor::new(registry, HashMap::new());

        let tool_call = ToolCall::new(
            "call_1".to_string(),
            FunctionCall::new("mock_tool".to_string(), r#"{"a": "hello"}"#.to_string()),
        );

        let result = executor.execute(&tool_call).await.unwrap();

        assert!(result.success);
        assert_eq!(result.tool_call_id, "call_1");

        // Output should be the inputs echoed back
        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["a"], "hello");
    }

    #[tokio::test]
    async fn test_execute_tool_not_found() {
        let registry = Arc::new(MockRegistry::new());
        let executor = DagToolExecutor::new(registry, HashMap::new());

        let tool_call = ToolCall::new(
            "call_2".to_string(),
            FunctionCall::new("unknown_tool".to_string(), "{}".to_string()),
        );

        let result = executor.execute(&tool_call).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::ToolNotFound { name } => assert_eq!(name, "unknown_tool"),
            _ => panic!("Expected ToolNotFound"),
        }
    }

    #[tokio::test]
    async fn test_available_tools() {
        let registry = Arc::new(MockRegistry::new());
        let executor = DagToolExecutor::new(registry, HashMap::new());

        let tools = executor.available_tools().await;

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "mock_tool");
        assert_eq!(tools[0].parameters.properties.len(), 1);
        assert!(tools[0].parameters.properties.contains_key("a"));
    }

    #[tokio::test]
    async fn test_generate_tool_definition_with_config() {
        let registry = Arc::new(MockRegistry::new());
        let mut tool_configs = HashMap::new();

        let mut fixed_config = HashMap::new();
        fixed_config.insert("a".to_string(), serde_json::json!("fixed_value"));

        tool_configs.insert(
            "configured_tool".to_string(),
            ToolConfiguration {
                name: "configured_tool".to_string(),
                description: "A configured tool".to_string(),
                node_type: "mock_tool".to_string(),
                fixed_config,
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                field_mapping: None,
                node_schema: None,
                node_config: None,
                expose_sub_tools: None,
                summary: None,
                eager: false,
            },
        );

        let executor = DagToolExecutor::new(registry, tool_configs);
        let tools = executor.available_tools().await;

        let configured_tool = tools
            .iter()
            .find(|t| t.name == "configured_tool")
            .expect("configured_tool not found");

        // Check description
        assert_eq!(configured_tool.description, "A configured tool");

        // Check parameters: "a" should be hidden because it's in fixed_config
        assert!(!configured_tool.parameters.properties.contains_key("a"));

        // MockNode schema has "a". We fixed it. So properties should be empty.
        assert!(configured_tool.parameters.properties.is_empty());
    }

    #[tokio::test]
    async fn test_tool_name_from_config_name_when_key_is_uuid() {
        // When the map key is a UUID but config.name is a semantic name,
        // generate_tool_definition should use config.name so the LLM sees a meaningful name.
        let registry = Arc::new(MockRegistry::new());
        let mut tool_configs = HashMap::new();

        tool_configs.insert(
            "0618e7a1-2d50-4c7d-9244-52f2b504a3ca".to_string(),
            ToolConfiguration {
                name: "list_products".to_string(),
                description: "List products from the catalog".to_string(),
                node_type: "mock_tool".to_string(),
                fixed_config: HashMap::new(),
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                field_mapping: None,
                node_schema: None,
                node_config: None,
                expose_sub_tools: None,
                summary: None,
                eager: false,
            },
        );

        let executor = DagToolExecutor::new(registry, tool_configs);
        let tools = executor.available_tools().await;

        // Should use config.name, not the UUID key
        let tool = tools
            .iter()
            .find(|t| t.name == "list_products")
            .expect("tool named 'list_products' not found — UUID key leaked as name");
        assert_eq!(tool.description, "List products from the catalog");

        // UUID should NOT appear as a tool name
        assert!(
            !tools
                .iter()
                .any(|t| t.name == "0618e7a1-2d50-4c7d-9244-52f2b504a3ca"),
            "UUID key leaked as tool name"
        );
    }

    #[tokio::test]
    async fn test_execute_tool_by_config_name_when_key_is_uuid() {
        // When the map key is a UUID but config.name is semantic,
        // execute() should resolve the tool correctly when the LLM calls it by semantic name.
        let registry = Arc::new(MockRegistry::new());
        let mut tool_configs = HashMap::new();

        tool_configs.insert(
            "0618e7a1-2d50-4c7d-9244-52f2b504a3ca".to_string(),
            ToolConfiguration {
                name: "list_products".to_string(),
                description: "List products from the catalog".to_string(),
                node_type: "mock_tool".to_string(),
                fixed_config: HashMap::new(),
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                field_mapping: None,
                node_schema: None,
                node_config: None,
                expose_sub_tools: None,
                summary: None,
                eager: false,
            },
        );

        let executor = DagToolExecutor::new(registry, tool_configs);

        // LLM calls the tool using the semantic name (not the UUID key)
        let tool_call = ToolCall::new(
            "call_1".to_string(),
            FunctionCall::new("list_products".to_string(), r#"{"a": "test"}"#.to_string()),
        );

        let result = executor.execute(&tool_call).await;
        assert!(
            result.is_ok(),
            "execute should resolve tool by config.name: {:?}",
            result.err()
        );
        let result = result.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_field_mapping_to_body() {
        // field_mapping: title → body, message → body
        // LLM args: {title: "T", message: "M"}
        // Expected: inputs["body"] == {title: "T", message: "M"}
        let registry = Arc::new(MockRegistry::new());
        let mut tool_configs = HashMap::new();

        let mut field_mapping = HashMap::new();
        field_mapping.insert("title".to_string(), "body".to_string());
        field_mapping.insert("message".to_string(), "body".to_string());

        tool_configs.insert(
            "test_mapping".to_string(),
            ToolConfiguration {
                name: "test_mapping".to_string(),
                description: "Test field mapping".to_string(),
                node_type: "mock_tool".to_string(),
                fixed_config: HashMap::new(),
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                node_schema: None,
                field_mapping: Some(field_mapping),
                node_config: None,
                expose_sub_tools: None,
                summary: None,
                eager: false,
            },
        );

        let executor = DagToolExecutor::new(registry, tool_configs);

        let tool_call = ToolCall::new(
            "call_1".to_string(),
            FunctionCall::new(
                "test_mapping".to_string(),
                r#"{"title": "T", "message": "M"}"#.to_string(),
            ),
        );

        let result = executor.execute(&tool_call).await.unwrap();
        assert!(result.success);

        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["body"]["title"], "T");
        assert_eq!(output["body"]["message"], "M");
    }

    #[tokio::test]
    async fn test_field_mapping_merge_with_fixed_body() {
        // fixed_config: {body: {name: "Fulanito"}}
        // mergeable_fields: ["body"]
        // field_mapping: {message → body}
        // LLM args: {message: "Hi"}
        // Expected: inputs["body"] == {name: "Fulanito", message: "Hi"}
        let registry = Arc::new(MockRegistry::new());
        let mut tool_configs = HashMap::new();

        let mut fixed_config = HashMap::new();
        let mut body_fixed = serde_json::Map::new();
        body_fixed.insert("name".to_string(), serde_json::json!("Fulanito"));
        fixed_config.insert("body".to_string(), Value::Object(body_fixed));

        let mut field_mapping = HashMap::new();
        field_mapping.insert("message".to_string(), "body".to_string());

        tool_configs.insert(
            "test_merge".to_string(),
            ToolConfiguration {
                name: "test_merge".to_string(),
                description: "Test field mapping with merge".to_string(),
                node_type: "mock_tool".to_string(),
                fixed_config,
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: Some(vec!["body".to_string()]),
                node_schema: None,
                field_mapping: Some(field_mapping),
                node_config: None,
                expose_sub_tools: None,
                summary: None,
                eager: false,
            },
        );

        let executor = DagToolExecutor::new(registry, tool_configs);

        let tool_call = ToolCall::new(
            "call_2".to_string(),
            FunctionCall::new("test_merge".to_string(), r#"{"message": "Hi"}"#.to_string()),
        );

        let result = executor.execute(&tool_call).await.unwrap();
        assert!(result.success);

        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["body"]["name"], "Fulanito");
        assert_eq!(output["body"]["message"], "Hi");
    }

    #[tokio::test]
    async fn test_mergeable_headers() {
        // fixed_config: {headers: {Authorization: "Bearer x"}}
        // mergeable_fields: ["headers"]
        // field_mapping: {x_request_id → headers}
        // LLM args: {x_request_id: "abc"}
        // Expected: inputs["headers"] == {Authorization: "Bearer x", x_request_id: "abc"}
        let registry = Arc::new(MockRegistry::new());
        let mut tool_configs = HashMap::new();

        let mut fixed_config = HashMap::new();
        let mut headers_fixed = serde_json::Map::new();
        headers_fixed.insert("Authorization".to_string(), serde_json::json!("Bearer x"));
        fixed_config.insert("headers".to_string(), Value::Object(headers_fixed));

        let mut field_mapping = HashMap::new();
        field_mapping.insert("x_request_id".to_string(), "headers".to_string());

        tool_configs.insert(
            "test_headers".to_string(),
            ToolConfiguration {
                name: "test_headers".to_string(),
                description: "Test headers merge".to_string(),
                node_type: "mock_tool".to_string(),
                fixed_config,
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: Some(vec!["headers".to_string()]),
                node_schema: None,
                field_mapping: Some(field_mapping),
                node_config: None,
                expose_sub_tools: None,
                summary: None,
                eager: false,
            },
        );

        let executor = DagToolExecutor::new(registry, tool_configs);

        let tool_call = ToolCall::new(
            "call_3".to_string(),
            FunctionCall::new(
                "test_headers".to_string(),
                r#"{"x_request_id": "abc"}"#.to_string(),
            ),
        );

        let result = executor.execute(&tool_call).await.unwrap();
        assert!(result.success);

        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["headers"]["Authorization"], "Bearer x");
        assert_eq!(output["headers"]["x_request_id"], "abc");
    }

    #[tokio::test]
    async fn test_backward_compat_no_mapping() {
        // No field_mapping, no mergeable_fields
        // fixed_config: {a: "fixed"}
        // LLM args: {b: "dynamic"}
        // Expected: inputs == {a: "fixed", b: "dynamic"} (same as before)
        let registry = Arc::new(MockRegistry::new());
        let mut tool_configs = HashMap::new();

        let mut fixed_config = HashMap::new();
        fixed_config.insert("a".to_string(), serde_json::json!("fixed"));

        tool_configs.insert(
            "test_compat".to_string(),
            ToolConfiguration {
                name: "test_compat".to_string(),
                description: "Test backward compatibility".to_string(),
                node_type: "mock_tool".to_string(),
                fixed_config,
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                field_mapping: None,
                node_schema: None,
                node_config: None,
                expose_sub_tools: None,
                summary: None,
                eager: false,
            },
        );

        let executor = DagToolExecutor::new(registry, tool_configs);

        let tool_call = ToolCall::new(
            "call_4".to_string(),
            FunctionCall::new("test_compat".to_string(), r#"{"b": "dynamic"}"#.to_string()),
        );

        let result = executor.execute(&tool_call).await.unwrap();
        assert!(result.success);

        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["a"], "fixed");
        assert_eq!(output["b"], "dynamic");
    }

    // ──────────────────────────────────────────────────────────────────────
    // Tests for $DYNAMIC placeholder system
    // ──────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_dynamic_placeholder_body() {
        // fixed_config: {body: {userId: 1, author: "Fulanito", title: "$DYNAMIC", content: "$DYNAMIC"}}
        // LLM args: {title: "Test Title", content: "Test Content"}
        // Expected: inputs["body"] == {userId: 1, author: "Fulanito", title: "Test Title", content: "Test Content"}
        let registry = Arc::new(MockRegistry::new());
        let mut tool_configs = HashMap::new();

        let mut body_fixed = serde_json::Map::new();
        body_fixed.insert("userId".to_string(), serde_json::json!(1));
        body_fixed.insert("author".to_string(), serde_json::json!("Fulanito"));
        body_fixed.insert("title".to_string(), serde_json::json!(DYNAMIC_PLACEHOLDER));
        body_fixed.insert(
            "content".to_string(),
            serde_json::json!(DYNAMIC_PLACEHOLDER),
        );

        let mut fixed_config = HashMap::new();
        fixed_config.insert("body".to_string(), Value::Object(body_fixed));

        tool_configs.insert(
            "test_dynamic_body".to_string(),
            ToolConfiguration {
                name: "test_dynamic_body".to_string(),
                description: "Test $DYNAMIC in body".to_string(),
                node_type: "mock_tool".to_string(),
                fixed_config,
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                field_mapping: None,
                node_schema: None,
                node_config: None,
                expose_sub_tools: None,
                summary: None,
                eager: false,
            },
        );

        let executor = DagToolExecutor::new(registry, tool_configs);

        let tool_call = ToolCall::new(
            "call_dyn_1".to_string(),
            FunctionCall::new(
                "test_dynamic_body".to_string(),
                r#"{"title": "Test Title", "content": "Test Content"}"#.to_string(),
            ),
        );

        let result = executor.execute(&tool_call).await.unwrap();
        assert!(result.success);

        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["body"]["userId"], 1);
        assert_eq!(output["body"]["author"], "Fulanito");
        assert_eq!(output["body"]["title"], "Test Title");
        assert_eq!(output["body"]["content"], "Test Content");
    }

    #[tokio::test]
    async fn test_dynamic_placeholder_headers_and_body() {
        // fixed_config: {
        //   headers: {Authorization: "Bearer secret", X-Request-ID: "$DYNAMIC"},
        //   body: {userId: 1, name: "$DYNAMIC"}
        // }
        // LLM args: {X-Request-ID: "req_123", name: "Alice"}
        // Expected:
        //   inputs["headers"] == {Authorization: "Bearer secret", X-Request-ID: "req_123"}
        //   inputs["body"] == {userId: 1, name: "Alice"}
        let registry = Arc::new(MockRegistry::new());
        let mut tool_configs = HashMap::new();

        let mut headers_fixed = serde_json::Map::new();
        headers_fixed.insert(
            "Authorization".to_string(),
            serde_json::json!("Bearer secret"),
        );
        headers_fixed.insert(
            "X-Request-ID".to_string(),
            serde_json::json!(DYNAMIC_PLACEHOLDER),
        );

        let mut body_fixed = serde_json::Map::new();
        body_fixed.insert("userId".to_string(), serde_json::json!(1));
        body_fixed.insert("name".to_string(), serde_json::json!(DYNAMIC_PLACEHOLDER));

        let mut fixed_config = HashMap::new();
        fixed_config.insert("headers".to_string(), Value::Object(headers_fixed));
        fixed_config.insert("body".to_string(), Value::Object(body_fixed));

        tool_configs.insert(
            "test_dynamic_multi".to_string(),
            ToolConfiguration {
                name: "test_dynamic_multi".to_string(),
                description: "Test $DYNAMIC across multiple fields".to_string(),
                node_type: "mock_tool".to_string(),
                fixed_config,
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                field_mapping: None,
                node_schema: None,
                node_config: None,
                expose_sub_tools: None,
                summary: None,
                eager: false,
            },
        );

        let executor = DagToolExecutor::new(registry, tool_configs);

        let tool_call = ToolCall::new(
            "call_dyn_2".to_string(),
            FunctionCall::new(
                "test_dynamic_multi".to_string(),
                r#"{"X-Request-ID": "req_123", "name": "Alice"}"#.to_string(),
            ),
        );

        let result = executor.execute(&tool_call).await.unwrap();
        assert!(result.success);

        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["headers"]["Authorization"], "Bearer secret");
        assert_eq!(output["headers"]["X-Request-ID"], "req_123");
        assert_eq!(output["body"]["userId"], 1);
        assert_eq!(output["body"]["name"], "Alice");
    }

    #[tokio::test]
    async fn test_dynamic_placeholder_top_level() {
        // fixed_config: {base_url: "https://example.com", endpoint: "$DYNAMIC", method: "POST"}
        // LLM args: {endpoint: "/users"}
        // Expected: inputs == {base_url: "https://example.com", endpoint: "/users", method: "POST"}
        let registry = Arc::new(MockRegistry::new());
        let mut tool_configs = HashMap::new();

        let mut fixed_config = HashMap::new();
        fixed_config.insert(
            "base_url".to_string(),
            serde_json::json!("https://example.com"),
        );
        fixed_config.insert(
            "endpoint".to_string(),
            serde_json::json!(DYNAMIC_PLACEHOLDER),
        );
        fixed_config.insert("method".to_string(), serde_json::json!("POST"));

        tool_configs.insert(
            "test_dynamic_toplevel".to_string(),
            ToolConfiguration {
                name: "test_dynamic_toplevel".to_string(),
                description: "Test $DYNAMIC at top level".to_string(),
                node_type: "mock_tool".to_string(),
                fixed_config,
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                field_mapping: None,
                node_schema: None,
                node_config: None,
                expose_sub_tools: None,
                summary: None,
                eager: false,
            },
        );

        let executor = DagToolExecutor::new(registry, tool_configs);

        let tool_call = ToolCall::new(
            "call_dyn_3".to_string(),
            FunctionCall::new(
                "test_dynamic_toplevel".to_string(),
                r#"{"endpoint": "/users"}"#.to_string(),
            ),
        );

        let result = executor.execute(&tool_call).await.unwrap();
        assert!(result.success);

        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(output["base_url"], "https://example.com");
        assert_eq!(output["endpoint"], "/users");
        assert_eq!(output["method"], "POST");
    }

    #[tokio::test]
    async fn test_dynamic_generates_correct_tool_definition() {
        // Verify that $DYNAMIC placeholders generate correct ToolDefinition
        let registry = Arc::new(MockRegistry::new());
        let mut tool_configs = HashMap::new();

        let mut body_fixed = serde_json::Map::new();
        body_fixed.insert("userId".to_string(), serde_json::json!(1));
        body_fixed.insert("title".to_string(), serde_json::json!(DYNAMIC_PLACEHOLDER));
        body_fixed.insert(
            "content".to_string(),
            serde_json::json!(DYNAMIC_PLACEHOLDER),
        );

        let mut headers_fixed = serde_json::Map::new();
        headers_fixed.insert(
            "Authorization".to_string(),
            serde_json::json!("Bearer secret"),
        );
        headers_fixed.insert(
            "X-Request-ID".to_string(),
            serde_json::json!(DYNAMIC_PLACEHOLDER),
        );

        let mut fixed_config = HashMap::new();
        fixed_config.insert("body".to_string(), Value::Object(body_fixed));
        fixed_config.insert("headers".to_string(), Value::Object(headers_fixed));

        tool_configs.insert(
            "test_dynamic_definition".to_string(),
            ToolConfiguration {
                name: "test_dynamic_definition".to_string(),
                description: "Test dynamic tool definition".to_string(),
                node_type: "mock_tool".to_string(),
                fixed_config,
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                field_mapping: None,
                node_schema: None,
                node_config: None,
                expose_sub_tools: None,
                summary: None,
                eager: false,
            },
        );

        let executor = DagToolExecutor::new(registry, tool_configs);
        let tools = executor.available_tools().await;

        let tool_def = tools
            .iter()
            .find(|t| t.name == "test_dynamic_definition")
            .expect("test_dynamic_definition not found");

        // Should have exactly 3 required parameters: title, content, X-Request-ID
        assert_eq!(tool_def.parameters.properties.len(), 3);
        assert!(tool_def.parameters.properties.contains_key("title"));
        assert!(tool_def.parameters.properties.contains_key("content"));
        assert!(tool_def.parameters.properties.contains_key("X-Request-ID"));
        assert_eq!(tool_def.parameters.required.len(), 3);

        // Check descriptions include container context
        let title_prop = &tool_def.parameters.properties["title"];
        assert!(title_prop.description.contains("body"));

        let x_request_prop = &tool_def.parameters.properties["X-Request-ID"];
        assert!(x_request_prop.description.contains("headers"));
    }

    #[tokio::test]
    async fn intercepts_load_skill_when_repository_attached() {
        use crate::skills::domain::{
            Skill, SkillCatalogEntry, SkillError, SkillReference, SkillRepository, SkillSource,
        };
        use async_trait::async_trait;

        struct TinyRepo;
        #[async_trait]
        impl SkillRepository for TinyRepo {
            fn list_available(&self) -> Vec<SkillCatalogEntry> {
                vec![SkillCatalogEntry {
                    name: "x".into(),
                    description: "d".into(),
                    source: SkillSource::Builtin,
                }]
            }
            async fn load_skill(&self, name: &str) -> Result<Skill, SkillError> {
                Ok(Skill {
                    name: name.into(),
                    description: "d".into(),
                    body: "BODY".into(),
                    references: vec![],
                    source: SkillSource::Builtin,
                })
            }
            async fn load_reference(&self, _: &str, _: &str) -> Result<SkillReference, SkillError> {
                Err(SkillError::SkillNotFound("x".into()))
            }
        }

        let registry = Arc::new(MockRegistry::new());
        let executor =
            DagToolExecutor::new(registry, HashMap::new()).with_skills(Arc::new(TinyRepo));

        let call = ToolCall::new(
            "c1".to_string(),
            FunctionCall::new("load_skill".to_string(), r#"{"name":"x"}"#.to_string()),
        );

        let result = executor.execute(&call).await.unwrap();
        assert!(result.output.contains("BODY"));
        assert!(result.success);
    }

    #[tokio::test]
    async fn intercepts_describe_tool_when_lookup_attached() {
        let registry = Arc::new(MockRegistry::new());
        let cfg = ToolConfiguration {
            name: "search_orders".to_string(),
            description: "Search the orders table".to_string(),
            node_type: "noop".to_string(),
            fixed_config: HashMap::new(),
            #[allow(deprecated)]
            exposed_inputs: None,
            #[allow(deprecated)]
            parameters: None,
            #[allow(deprecated)]
            mergeable_fields: None,
            #[allow(deprecated)]
            field_mapping: None,
            node_schema: None,
            node_config: None,
            expose_sub_tools: None,
            summary: None,
            eager: false,
        };
        let executor =
            DagToolExecutor::new(registry, HashMap::new()).with_describe_tool_lookup(vec![cfg]);

        let call = ToolCall::new(
            "c_describe".to_string(),
            FunctionCall::new(
                "describe_tool".to_string(),
                serde_json::json!({"name":"search_orders"}).to_string(),
            ),
        );

        let result = executor.execute(&call).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("# search_orders"));
        assert!(result.output.contains("now available"));
    }

    #[tokio::test]
    async fn describe_tool_observer_fires_with_dispatched_payload() {
        let registry = Arc::new(MockRegistry::new());
        let cfg = ToolConfiguration {
            name: "search_orders".to_string(),
            description: "Search".to_string(),
            node_type: "noop".to_string(),
            fixed_config: HashMap::new(),
            #[allow(deprecated)]
            exposed_inputs: None,
            #[allow(deprecated)]
            parameters: None,
            #[allow(deprecated)]
            mergeable_fields: None,
            #[allow(deprecated)]
            field_mapping: None,
            node_schema: None,
            node_config: None,
            expose_sub_tools: None,
            summary: None,
            eager: false,
        };

        let observed: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_clone = observed.clone();

        let executor = DagToolExecutor::new(registry, HashMap::new())
            .with_describe_tool_lookup(vec![cfg])
            .with_describe_tool_observer(Arc::new(move |result| {
                observed_clone
                    .lock()
                    .unwrap()
                    .push(result.tool_name.clone());
            }));

        let call = ToolCall::new(
            "c1".to_string(),
            FunctionCall::new(
                "describe_tool".to_string(),
                serde_json::json!({"name":"search_orders"}).to_string(),
            ),
        );
        executor.execute(&call).await.unwrap();
        assert_eq!(observed.lock().unwrap().as_slice(), &["search_orders"]);
    }

    #[tokio::test]
    async fn test_dynamic_priority_over_field_mapping() {
        // Verify that when $DYNAMIC is present, it takes priority over field_mapping
        let registry = Arc::new(MockRegistry::new());
        let mut tool_configs = HashMap::new();

        let mut body_fixed = serde_json::Map::new();
        body_fixed.insert("title".to_string(), serde_json::json!(DYNAMIC_PLACEHOLDER));

        let mut fixed_config = HashMap::new();
        fixed_config.insert("body".to_string(), Value::Object(body_fixed));

        let mut field_mapping = HashMap::new();
        field_mapping.insert("title".to_string(), "headers".to_string()); // This should be ignored

        tool_configs.insert(
            "test_dynamic_priority".to_string(),
            ToolConfiguration {
                name: "test_dynamic_priority".to_string(),
                description: "Test $DYNAMIC priority".to_string(),
                node_type: "mock_tool".to_string(),
                fixed_config,
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                node_schema: None,
                field_mapping: Some(field_mapping),
                node_config: None,
                expose_sub_tools: None,
                summary: None,
                eager: false,
            },
        );

        let executor = DagToolExecutor::new(registry, tool_configs);

        let tool_call = ToolCall::new(
            "call_dyn_4".to_string(),
            FunctionCall::new(
                "test_dynamic_priority".to_string(),
                r#"{"title": "Test"}"#.to_string(),
            ),
        );

        let result = executor.execute(&tool_call).await.unwrap();
        assert!(result.success);

        let output: Value = serde_json::from_str(&result.output).unwrap();
        // title should be in body (from $DYNAMIC), not in headers (from field_mapping)
        assert_eq!(output["body"]["title"], "Test");
        assert!(output.get("headers").is_none() || output["headers"].is_null());
    }

    #[tokio::test]
    async fn execute_with_resume_answer_threads_value_into_node_inputs() {
        // MockNode echoes inputs back as JSON — we can assert the resume key was injected.
        let registry = Arc::new(MockRegistry::new());
        let executor = DagToolExecutor::new(registry, HashMap::new());

        let tool_call = ToolCall::new(
            "call_resume".to_string(),
            FunctionCall::new("mock_tool".to_string(), r#"{"a": "original"}"#.to_string()),
        );

        let result = executor
            .execute_with_resume_answer(&tool_call, "USER_ANSWER_42")
            .await
            .unwrap();

        assert!(result.success);

        let output: Value = serde_json::from_str(&result.output).unwrap();
        // The original LLM arg must still be present.
        assert_eq!(output["a"], "original");
        // The resume answer must have been injected under the reserved key.
        assert_eq!(output["__colmena_resume_answer"], "USER_ANSWER_42");
    }

    #[tokio::test]
    async fn execute_without_resume_answer_does_not_inject_key() {
        // Calling the plain execute path must NOT inject __colmena_resume_answer.
        let registry = Arc::new(MockRegistry::new());
        let executor = DagToolExecutor::new(registry, HashMap::new());

        let tool_call = ToolCall::new(
            "call_plain".to_string(),
            FunctionCall::new("mock_tool".to_string(), r#"{"a": "plain"}"#.to_string()),
        );

        let result = executor.execute(&tool_call).await.unwrap();
        assert!(result.success);

        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert!(
            output.get("__colmena_resume_answer").is_none(),
            "plain execute must not inject __colmena_resume_answer"
        );
    }

    #[tokio::test]
    async fn execute_inner_injects_session_id_into_node_inputs() {
        // Build an executor with a known session_id and verify that every tool
        // dispatch — including plain `execute` — receives __colmena_session_id
        // in its inputs map.  This is the uniform-contract guarantee that allows
        // secure_suspend (and similar nodes) to find the session on the resume path.
        let registry = Arc::new(MockRegistry::new());
        let executor = DagToolExecutor::new(registry, HashMap::new())
            .with_session_id("session_xyz".to_string());

        let tool_call = ToolCall::new(
            "call_sid".to_string(),
            FunctionCall::new("mock_tool".to_string(), r#"{"a": "value"}"#.to_string()),
        );

        let result = executor.execute(&tool_call).await.unwrap();
        assert!(result.success);

        let output: Value = serde_json::from_str(&result.output).unwrap();
        // The original arg must still be present.
        assert_eq!(output["a"], "value");
        // The session_id must have been injected under the reserved key.
        assert_eq!(
            output["__colmena_session_id"], "session_xyz",
            "execute_inner must inject __colmena_session_id from self.session_id"
        );
    }

    #[tokio::test]
    async fn execute_inner_does_not_inject_session_id_when_none() {
        // When no session_id is configured (executor built without with_session_id or
        // with_secure_values), __colmena_session_id must NOT appear in the inputs.
        let registry = Arc::new(MockRegistry::new());
        let executor = DagToolExecutor::new(registry, HashMap::new());

        let tool_call = ToolCall::new(
            "call_no_sid".to_string(),
            FunctionCall::new(
                "mock_tool".to_string(),
                r#"{"b": "no_session"}"#.to_string(),
            ),
        );

        let result = executor.execute(&tool_call).await.unwrap();
        assert!(result.success);

        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert!(
            output.get("__colmena_session_id").is_none(),
            "execute_inner must not inject __colmena_session_id when session_id is None"
        );
    }
}

#[cfg(test)]
#[allow(deprecated)] // Tests intentionally exercise legacy ToolConfiguration fields for backward-compat coverage.
mod toolkit_runtime_tests {
    use super::*;
    use crate::dag_engine::domain::node::ExecutableNode;
    use crate::dag_engine::domain::tool_configuration::{SubToolFilter, ToolConfiguration};
    use crate::dag_engine::domain::toolkit_node::ToolkitNode;
    use crate::dag_engine::infrastructure::nodes::echo_toolkit::EchoToolkitNode;
    use serde_json::json;
    use std::sync::Arc;

    /// Test registry that returns the same `Arc<EchoToolkitNode>` for both
    /// `get_node()` and `get_toolkit_node()`.
    struct EchoRegistry {
        node: Arc<EchoToolkitNode>,
    }

    impl crate::dag_engine::application::ports::NodeRegistryPort for EchoRegistry {
        fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
            if node_type == "echo_toolkit" {
                Some(self.node.clone() as Arc<dyn ExecutableNode>)
            } else {
                None
            }
        }

        fn get_all_nodes(&self) -> std::collections::HashMap<String, Arc<dyn ExecutableNode>> {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "echo_toolkit".to_string(),
                self.node.clone() as Arc<dyn ExecutableNode>,
            );
            m
        }

        fn get_toolkit_node(&self, node_type: &str) -> Option<Arc<dyn ToolkitNode>> {
            if node_type == "echo_toolkit" {
                Some(self.node.clone() as Arc<dyn ToolkitNode>)
            } else {
                None
            }
        }
    }

    #[allow(deprecated)]
    fn build_executor_with_toolkit_all() -> DagToolExecutor {
        let registry = Arc::new(EchoRegistry {
            node: Arc::new(EchoToolkitNode),
        });
        let mut configs = HashMap::new();
        configs.insert(
            "web".to_string(),
            ToolConfiguration {
                name: "web".to_string(),
                description: "echo toolkit".to_string(),
                node_type: "echo_toolkit".to_string(),
                fixed_config: HashMap::new(),
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                field_mapping: None,
                node_schema: None,
                node_config: Some(json!({})),
                expose_sub_tools: Some(SubToolFilter::all()),
                summary: None,
                eager: false,
            },
        );
        DagToolExecutor::new(registry, configs)
    }

    #[tokio::test]
    async fn toolkit_expands_to_one_tooldef_per_sub_tool() {
        let exec = build_executor_with_toolkit_all();
        let tools = exec.available_tools().await;
        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        // Prefixed by alias "web__"
        assert!(names.contains(&"web__echo".to_string()));
        assert!(names.contains(&"web__double".to_string()));
    }

    #[tokio::test]
    async fn toolkit_dispatch_echo_returns_message() {
        use crate::llm::domain::{FunctionCall, ToolCall};

        let exec = build_executor_with_toolkit_all();

        let call = ToolCall {
            id: "call-1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "web__echo".to_string(),
                arguments: r#"{"message":"hola"}"#.to_string(),
            },
            response: None,
        };
        let result = exec.execute(&call).await.expect("execute ok");
        assert!(result.success, "got error: {:?}", result.error);
        // Output is a JSON-stringified value.
        let parsed: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(parsed.get("output").unwrap().as_str(), Some("hola"));
    }

    #[tokio::test]
    async fn toolkit_dispatch_unknown_sub_tool_errors_cleanly() {
        use crate::llm::domain::{FunctionCall, ToolCall};

        let exec = build_executor_with_toolkit_all();

        let call = ToolCall {
            id: "call-2".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "web__does_not_exist".to_string(),
                arguments: "{}".to_string(),
            },
            response: None,
        };
        let result = exec
            .execute(&call)
            .await
            .expect("execute returns ToolResult");
        assert!(!result.success);
        assert!(result.output.to_lowercase().contains("unknown sub-tool"));
    }

    #[tokio::test]
    #[allow(deprecated)]
    async fn toolkit_filter_list_only_exposes_listed_sub_tools() {
        let registry = Arc::new(EchoRegistry {
            node: Arc::new(EchoToolkitNode),
        });
        let mut configs = HashMap::new();
        configs.insert(
            "web".to_string(),
            ToolConfiguration {
                name: "web".to_string(),
                description: "".to_string(),
                node_type: "echo_toolkit".to_string(),
                fixed_config: HashMap::new(),
                exposed_inputs: None,
                parameters: None,
                mergeable_fields: None,
                field_mapping: None,
                node_schema: None,
                node_config: None,
                expose_sub_tools: Some(SubToolFilter::List(vec!["echo".to_string()])),
                summary: None,
                eager: false,
            },
        );
        let exec = DagToolExecutor::new(registry, configs);
        let tools = exec.available_tools().await;
        let names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        assert!(names.contains(&"web__echo".to_string()));
        assert!(!names.contains(&"web__double".to_string()));
    }

    /// Test registry that exposes `api_explorer` as both an `ExecutableNode`
    /// and a `ToolkitNode`, plus a stand-in raw `tavily_client` ExecutableNode.
    /// Used to verify that `available_tools()` auto-expands `api_explorer`
    /// sub-tools when no `tool_configurations` entry exists, while leaving
    /// other raw nodes (`tavily_client`) untouched.
    struct ApiExplorerOnlyRegistry {
        api_explorer: Arc<crate::dag_engine::infrastructure::nodes::api_explorer::ApiExplorerNode>,
        tavily_stub: Arc<EchoToolkitNode>,
    }

    impl crate::dag_engine::application::ports::NodeRegistryPort for ApiExplorerOnlyRegistry {
        fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
            match node_type {
                "api_explorer" => Some(self.api_explorer.clone() as Arc<dyn ExecutableNode>),
                "tavily_client" => Some(self.tavily_stub.clone() as Arc<dyn ExecutableNode>),
                _ => None,
            }
        }

        fn get_all_nodes(&self) -> std::collections::HashMap<String, Arc<dyn ExecutableNode>> {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "api_explorer".to_string(),
                self.api_explorer.clone() as Arc<dyn ExecutableNode>,
            );
            m.insert(
                "tavily_client".to_string(),
                self.tavily_stub.clone() as Arc<dyn ExecutableNode>,
            );
            m
        }

        fn get_toolkit_node(&self, node_type: &str) -> Option<Arc<dyn ToolkitNode>> {
            match node_type {
                "api_explorer" => Some(self.api_explorer.clone() as Arc<dyn ToolkitNode>),
                _ => None,
            }
        }
    }

    #[tokio::test]
    async fn api_explorer_auto_expands_subtools_without_tool_configurations() {
        use crate::dag_engine::infrastructure::nodes::api_explorer::ApiExplorerNode;

        let registry = Arc::new(ApiExplorerOnlyRegistry {
            api_explorer: Arc::new(ApiExplorerNode::new()),
            tavily_stub: Arc::new(EchoToolkitNode),
        });
        // EMPTY tool_configurations — auto-expansion must happen in loop 2.
        let executor = DagToolExecutor::new(registry, HashMap::new());

        let tools = executor.available_tools().await;
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();

        // The 4 mandated sub-tools must all be auto-exposed.
        for required in [
            "api_explorer__load_spec",
            "api_explorer__search_endpoint",
            "api_explorer__get_endpoint_details",
            "api_explorer__build_http_request",
        ] {
            assert!(
                names.contains(&required),
                "expected `{required}` in auto-exposed tools, got {names:?}"
            );
        }

        // No raw-node fallthrough: bare `api_explorer` must NOT appear.
        assert!(
            !names.contains(&"api_explorer"),
            "raw `api_explorer` leaked into tool list — expansion fell through to raw branch: {names:?}"
        );

        // Other raw nodes (e.g. tavily_client) are unaffected: this special
        // case must target ONLY api_explorer.
        assert!(
            names.contains(&"tavily_client"),
            "tavily_client should still appear as a raw tool — only api_explorer is special-cased: {names:?}"
        );
    }

    /// Companion to `api_explorer_auto_expands_subtools_without_tool_configurations`:
    /// once the catalog exposes `api_explorer__<sub>` tools, the LLM must also be
    /// able to *call* them. Before the dispatch fallback existed, the executor
    /// would fall through to `registry.get_node("api_explorer__load_spec")` (no
    /// such node) and return `ToolNotFound` — even though `available_tools()`
    /// had advertised the sub-tool.
    ///
    /// This test pins the dispatch path: empty `tool_configurations`, but the
    /// registry has `api_explorer` as a toolkit. Calling
    /// `api_explorer__load_spec` must reach the underlying node (we do not care
    /// whether the call itself succeeds — only that dispatch routes correctly).
    #[tokio::test]
    async fn flag_only_dispatch_api_explorer_subtool_succeeds() {
        use crate::dag_engine::infrastructure::nodes::api_explorer::ApiExplorerNode;
        use crate::llm::domain::{FunctionCall, ToolCall};

        let registry = Arc::new(ApiExplorerOnlyRegistry {
            api_explorer: Arc::new(ApiExplorerNode::new()),
            tavily_stub: Arc::new(EchoToolkitNode),
        });
        // EMPTY tool_configurations — the dispatch fallback must synthesise one.
        let executor = DagToolExecutor::new(registry, HashMap::new());

        let call = ToolCall {
            id: "test1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "api_explorer__load_spec".to_string(),
                // An obviously-invalid URL: the toolkit will return a domain
                // error, but dispatch will have reached the node. That's what
                // we're asserting — NOT that the load itself succeeds.
                arguments: r#"{"url":"not-a-real-url-xyz"}"#.to_string(),
            },
            response: None,
        };

        let result = executor
            .execute(&call)
            .await
            .expect("dispatch must reach the toolkit, not return ToolNotFound");

        // Whatever the underlying node returned, the dispatch path produced a
        // ToolResult — that proves the synthesised config was used. The
        // toolkit returns a structured payload (success or domain error JSON);
        // both are encoded as a non-empty JSON-stringified output.
        assert!(
            !result.output.is_empty(),
            "expected a tool result payload (success or domain error), got empty output"
        );
        // Sanity: the result must NOT be a "tool not found" surface. The
        // `execute_toolkit` path always sets tool_call_id to the original id.
        assert_eq!(result.tool_call_id, "test1");
    }

    #[tokio::test]
    async fn intercepts_load_attachment_when_catalog_attached() {
        use crate::llm::domain::attachments::AttachmentSource;
        use crate::llm::domain::tools::FunctionCall;
        use crate::llm::domain::ProviderKind;
        use crate::llm::domain::{ConversationAttachment, ToolCall};
        use chrono::Utc;

        struct DummyRegistry;
        impl NodeRegistryPort for DummyRegistry {
            fn get_node(&self, _: &str) -> Option<Arc<dyn ExecutableNode>> {
                None
            }

            fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
                HashMap::new()
            }
        }

        let attach = ConversationAttachment {
            agent_session_id: "s1".to_string(),
            document_id: "doc-x".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_bytes: Some(1024),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            registered_at: Utc::now(),
            refreshed_at: Utc::now(),
            storage_key: None,
            origin: None,
            last_used_at: None,
        };

        let executor = DagToolExecutor::new(Arc::new(DummyRegistry), Default::default())
            .with_attachments(vec![attach]);

        let call = ToolCall {
            id: "c1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall::new(
                "load_attachment".to_string(),
                r#"{"document_id":"doc-x"}"#.to_string(),
            ),
            response: None,
        };

        let res = executor.execute(&call).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&res.output).unwrap();
        assert_eq!(parsed["__colmena_status"], "LOAD_ATTACHMENT");
        assert_eq!(parsed["document_id"], "doc-x");
    }
}

#[cfg(test)]
mod scrubber_tests {
    //! Unit tests for [`DagToolExecutor::scrub_value_for_llm`] +
    //! [`DagToolExecutor::scrub_tool_result_output`]. The invariant we
    //! enforce is: **the LLM never sees raw binary base64 in tool results,
    //! and no single string blows past the configured byte cap**.
    use super::*;
    use serde_json::json;

    #[test]
    fn data_uri_with_base64_is_replaced_with_marker() {
        let v = json!("data:image/png;base64,AAAABBBBCCCCDDDD");
        let scrubbed = DagToolExecutor::scrub_value_for_llm(v, 1_000_000);
        let s = scrubbed.as_str().unwrap();
        assert!(s.starts_with("[binary elided"));
        assert!(s.contains("mime=image/png"));
    }

    #[test]
    fn nested_data_uri_inside_object_is_replaced() {
        let v = json!({
            "image": "data:image/png;base64,XX",
            "name": "small.png",
            "info": { "thumb": "data:image/jpeg;base64,YY" }
        });
        let out = DagToolExecutor::scrub_value_for_llm(v, 1_000_000);
        assert!(out["image"].as_str().unwrap().starts_with("[binary elided"));
        assert!(out["info"]["thumb"]
            .as_str()
            .unwrap()
            .starts_with("[binary elided"));
        assert_eq!(out["name"], "small.png");
    }

    #[test]
    fn long_plain_string_above_cap_is_truncated() {
        let s = "x".repeat(60_000);
        let scrubbed = DagToolExecutor::scrub_value_for_llm(json!(s), 50_000);
        let out = scrubbed.as_str().unwrap();
        assert!(out.starts_with("[truncated"));
        assert!(out.contains("original_size=60000"));
    }

    #[test]
    fn short_strings_pass_through() {
        let v = json!({ "hello": "world", "n": 42 });
        let out = DagToolExecutor::scrub_value_for_llm(v.clone(), 1_000);
        assert_eq!(out, v);
    }

    #[test]
    fn scrub_tool_result_output_handles_json() {
        // The exact failure mode we hit in the smoke test:
        // httpbin echoes the image bytes back in its response body,
        // which becomes part of the tool result JSON.
        let echoed = json!({
            "status": 200,
            "body": { "image": "data:image/png;base64,AAAABBBBCCCC" }
        })
        .to_string();
        let out = DagToolExecutor::scrub_tool_result_output(echoed, 50_000);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["status"], 200);
        assert!(parsed["body"]["image"]
            .as_str()
            .unwrap()
            .starts_with("[binary elided"));
    }

    #[test]
    fn scrub_tool_result_output_handles_non_json() {
        let big = "x".repeat(100_000);
        let out = DagToolExecutor::scrub_tool_result_output(big, 50_000);
        assert!(out.starts_with("[truncated"));
        assert!(out.contains("original_size=100000"));
    }

    #[test]
    fn data_uri_without_base64_is_left_alone() {
        // Conservative — only data:*;base64,* gets the binary-elision treatment.
        // Other data: URIs (e.g. data:text/plain,hello) stay intact.
        let v = json!("data:text/plain,hello world");
        let out = DagToolExecutor::scrub_value_for_llm(v.clone(), 1_000);
        assert_eq!(out, v);
    }
}
