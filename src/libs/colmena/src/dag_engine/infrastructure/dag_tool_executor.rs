use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::domain::node::ExecutableNode;
use crate::dag_engine::domain::tool_configuration::ToolConfiguration;
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
    pub fn new(
        registry: Arc<dyn NodeRegistryPort>,
        tool_configurations: HashMap<String, ToolConfiguration>,
    ) -> Self {
        Self {
            registry,
            tool_configurations,
        }
    }

    /// Generate ToolDefinition from node with partial configuration
    fn generate_tool_definition(
        &self,
        tool_name: &str,
        tool_config: &ToolConfiguration,
        node: &Arc<dyn ExecutableNode>,
    ) -> crate::llm::domain::ToolDefinition {
        use crate::llm::domain::{ParameterProperty, ToolDefinition, ToolParameters};

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
        let (node, fixed_config) = if let Some(config) = self.tool_configurations.get(node_type) {
            let node = self.registry.get_node(&config.node_type).ok_or_else(|| {
                LlmError::ToolNotFound {
                    name: config.node_type.clone(),
                }
            })?;
            (node, Some(config.fixed_config.clone()))
        } else {
            let node = self
                .registry
                .get_node(node_type)
                .ok_or_else(|| LlmError::ToolNotFound {
                    name: node_type.clone(),
                })?;
            (node, None)
        };

        // 2. Parse arguments
        let args: HashMap<String, Value> = serde_json::from_str(&tool_call.function.arguments)
            .map_err(|e| LlmError::InvalidToolCall {
                reason: format!("Failed to parse arguments for tool {}: {}", node_type, e),
            })?;

        // 3. Execute the node
        // Merge fixed_config with arguments if present
        let mut final_args = args;
        if let Some(fixed) = fixed_config {
            for (k, v) in fixed {
                final_args.insert(k, v);
            }
        }

        // Convert HashMap to NodeInputs (which is just HashMap<String, Value>)
        let inputs = final_args;
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
}
