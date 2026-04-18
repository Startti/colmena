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
}

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
        }
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
            let parsed = parse_node_schema(schema);
            return ToolDefinition {
                name: effective_name.to_string(),
                description: tool_config.description.clone(),
                parameters: ToolParameters {
                    schema_type: "object".to_string(),
                    properties: parsed.llm_properties,
                    required: parsed.required_params,
                },
            };
        }

        // If parameters are explicitly defined in config, use them
        if let Some(params_value) = &tool_config.parameters {
            if let Ok(params) = serde_json::from_value::<ToolParameters>(params_value.clone()) {
                return ToolDefinition {
                    name: effective_name.to_string(),
                    description: tool_config.description.clone(),
                    parameters: params,
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
                    ParameterProperty {
                        property_type: "string".to_string(),
                        description,
                        enum_values: None,
                        pattern: None,
                    },
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
                ParameterProperty {
                    property_type: prop_type.to_string(),
                    description: desc.to_string(),
                    enum_values: None,
                    pattern: None,
                },
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
        }
    }
}

#[async_trait]
impl ToolExecutor for DagToolExecutor {
    #[allow(deprecated)]
    async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, LlmError> {
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
            let parsed = parse_node_schema(schema);
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

        // Convert HashMap to NodeInputs (which is just HashMap<String, Value>)
        // SECURE VALUES: decrypt <value_N> placeholders before sending to the node.
        let inputs = if let (Some(svc), Some(sid)) = (&self.secure_value_service, &self.session_id)
        {
            let mut inputs_val =
                serde_json::to_value(&inputs).unwrap_or(Value::Object(Default::default()));
            if let Err(e) = svc.inject_secrets(&mut inputs_val, sid).await {
                eprintln!("⚠️ [DagToolExecutor] Failed to inject secrets: {}", e);
            }
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
                            .hash_output(&value, &secure_config, sid, node_type)
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

    async fn available_tools(&self) -> Vec<crate::llm::domain::ToolDefinition> {
        use crate::llm::domain::{ParameterProperty, ToolDefinition, ToolParameters};

        let nodes = self.registry.get_all_nodes();
        let mut tools = Vec::new();

        // 1. Add configured tools first
        for (name, config) in &self.tool_configurations {
            if let Some(node) = self.registry.get_node(&config.node_type) {
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

            // Skip if this node name is already used by a configured tool?
            // Or maybe we allow both "http_call" (raw) and "fetch_users" (configured)?
            // Let's allow both for now, unless the configured tool explicitly uses the node name.
            if self.tool_configurations.contains_key(&name) {
                continue;
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
                        ParameterProperty {
                            property_type: prop_type.to_string(),
                            description: desc.to_string(),
                            enum_values: None, // TODO: Parse enum values if available
                            pattern: None,
                        },
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
            });
        }

        tools
    }
}

#[cfg(test)]
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
}
