use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::domain::node::ExecutableNode;
use crate::dag_engine::domain::tool_configuration::{ToolConfiguration, DYNAMIC_PLACEHOLDER};
use crate::llm::domain::{LlmError, ToolCall, ToolExecutor, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

pub struct DagToolExecutor {
    registry: Arc<dyn NodeRegistryPort>,
    tool_configurations: HashMap<String, ToolConfiguration>,
}

impl DagToolExecutor {
    /// Resolve ${var} and ${context.var} placeholders in a string value
    /// using values from the inputs HashMap
    fn resolve_template_string(template: &str, inputs: &HashMap<String, Value>) -> String {
        use regex::Regex;

        // Pattern: ${context.key} or ${key}
        let re = Regex::new(r"\$\{(?:context\.)?(\w+)\}").unwrap();

        re.replace_all(template, |caps: &regex::Captures| {
            let key = &caps[1];
            inputs
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or(&caps[0].to_string())
                .to_string()
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
    pub fn new(
        registry: Arc<dyn NodeRegistryPort>,
        tool_configurations: HashMap<String, ToolConfiguration>,
    ) -> Self {
        Self {
            registry,
            tool_configurations,
        }
    }

    /// Recursively scan fixed_config for all "$DYNAMIC" placeholders.
    /// Returns Vec of (param_name, container_field) tuples.
    /// - For nested: (field_key, Some(container_key)) e.g. ("title", Some("body"))
    /// - For top-level: (container_key, None) e.g. ("endpoint", None)
    fn collect_dynamic_fields(fixed_config: &HashMap<String, Value>) -> Vec<(String, Option<String>)> {
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
    fn generate_tool_definition(
        &self,
        tool_name: &str,
        tool_config: &ToolConfiguration,
        node: &Arc<dyn ExecutableNode>,
    ) -> crate::llm::domain::ToolDefinition {
        use crate::llm::domain::{ParameterProperty, ToolDefinition, ToolParameters};
        use crate::dag_engine::domain::tool_configuration::parse_node_schema;

        // BRANCH 0 (HIGHEST PRIORITY): node_schema
        if let Some(schema) = &tool_config.node_schema {
            let parsed = parse_node_schema(schema);
            return ToolDefinition {
                name: tool_name.to_string(),
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
                    name: tool_name.to_string(),
                    description: tool_config.description.clone(),
                    parameters: params,
                };
            } else {
                println!(
                    "WARN: Failed to parse custom parameters for tool {}",
                    tool_name
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
                name: tool_name.to_string(),
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
            name: tool_name.to_string(),
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
    async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, LlmError> {
        let node_type = &tool_call.function.name;

        // 1. Check if it's a configured tool or a raw node
        let (node, fixed_config, tool_cfg) = if let Some(config) = self.tool_configurations.get(node_type) {
            let node = self.registry.get_node(&config.node_type).ok_or_else(|| {
                LlmError::ToolNotFound {
                    name: config.node_type.clone(),
                }
            })?;
            (node, Some(config.fixed_config.clone()), Some(config))
        } else {
            let node = self
                .registry
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
                        map.insert(param_name.clone(), param_value.clone());
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
        let inputs = inputs;
        let config = serde_json::json!({});
        let mut state = serde_json::json!({});

        let result = node.execute(&inputs, &config, &mut state, None).await;

        // 4. Return result
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
        headers_fixed.insert(
            "Authorization".to_string(),
            serde_json::json!("Bearer x"),
        );
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
        body_fixed.insert("content".to_string(), serde_json::json!(DYNAMIC_PLACEHOLDER));

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
        headers_fixed.insert("X-Request-ID".to_string(), serde_json::json!(DYNAMIC_PLACEHOLDER));

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
        fixed_config.insert("base_url".to_string(), serde_json::json!("https://example.com"));
        fixed_config.insert("endpoint".to_string(), serde_json::json!(DYNAMIC_PLACEHOLDER));
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
        body_fixed.insert("content".to_string(), serde_json::json!(DYNAMIC_PLACEHOLDER));

        let mut headers_fixed = serde_json::Map::new();
        headers_fixed.insert(
            "Authorization".to_string(),
            serde_json::json!("Bearer secret"),
        );
        headers_fixed.insert("X-Request-ID".to_string(), serde_json::json!(DYNAMIC_PLACEHOLDER));

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
