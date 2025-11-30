use crate::application::ports::NodeRegistryPort;
use colmena::llm::domain::{LlmError, ToolCall, ToolExecutor, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

pub struct DagToolExecutor {
    registry: Arc<dyn NodeRegistryPort>,
}

impl DagToolExecutor {
    pub fn new(registry: Arc<dyn NodeRegistryPort>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl ToolExecutor for DagToolExecutor {
    async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, LlmError> {
        let node_type = &tool_call.function.name;
        
        // 1. Find the node in the registry
        let node = self.registry.get_node(node_type).ok_or_else(|| {
            LlmError::ToolNotFound {
                name: node_type.clone(),
            }
        })?;

        // 2. Parse arguments
        let args: HashMap<String, Value> = serde_json::from_str(&tool_call.function.arguments)
            .map_err(|e| LlmError::InvalidToolCall {
                reason: format!("Failed to parse arguments for tool {}: {}", node_type, e),
            })?;

        // 3. Execute the node
        let config = serde_json::json!({});
        let mut state = serde_json::json!({});

        let result = node.execute(&args, &config, &mut state).await;

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

    async fn available_tools(&self) -> Vec<colmena::llm::domain::ToolDefinition> {
        use colmena::llm::domain::{ToolDefinition, ToolParameters, ParameterProperty};

        let nodes = self.registry.get_all_nodes();
        let mut tools = Vec::new();

        for (name, node) in nodes {
            // Skip internal nodes or nodes that shouldn't be tools
            if name == "llm_call" || name == "mock_input" || name == "log" {
                continue;
            }

            let schema = node.schema();
            
            // Convert node schema to ToolDefinition
            // Node schema: { "type": "...", "config": {...}, "inputs": {...}, "outputs": {...} }
            // ToolDefinition needs: name, description, parameters (JSON Schema)
            
            // We use "inputs" as parameters.
            let inputs_schema = schema.get("inputs").cloned().unwrap_or(serde_json::json!({}));
            
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

                    properties.insert(key.clone(), ParameterProperty {
                        property_type: prop_type.to_string(),
                        description: desc.to_string(),
                        enum_values: None, // TODO: Parse enum values if available
                    });

                    if !is_optional {
                        required.push(key.clone());
                    }
                }
            }

            tools.push(ToolDefinition {
                name: name.clone(),
                description: format!("Execute node: {}", name), // TODO: Add description to Node schema
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
    use crate::domain::node::{ExecutableNode, NodeInputs};
    use async_trait::async_trait;
    use colmena::llm::domain::{FunctionCall, ToolCall, LlmError};
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
        ) -> Result<Value, Box<dyn std::error::Error>> {
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
            nodes.insert("mock_tool".to_string(), Arc::new(MockNode { name: "mock_tool".to_string() }));
            Self {
                nodes: nodes.into_iter().map(|(k, v)| (k, v as Arc<dyn ExecutableNode>)).collect(),
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
        let executor = DagToolExecutor::new(registry);

        let tool_call = ToolCall::new(
            "call_1".to_string(),
            FunctionCall::new(
                "mock_tool".to_string(),
                r#"{"a": "hello"}"#.to_string(),
            ),
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
        let executor = DagToolExecutor::new(registry);

        let tool_call = ToolCall::new(
            "call_2".to_string(),
            FunctionCall::new(
                "unknown_tool".to_string(),
                "{}".to_string(),
            ),
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
        let executor = DagToolExecutor::new(registry);

        let tools = executor.available_tools().await;
        
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "mock_tool");
        assert_eq!(tools[0].parameters.properties.len(), 1);
        assert!(tools[0].parameters.properties.contains_key("a"));
    }
}
