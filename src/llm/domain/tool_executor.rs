use super::{LlmError, ToolCall, ToolDefinition, ToolResult};
use async_trait::async_trait;

/// Trait for executing tools requested by LLMs
///
/// This abstraction allows the LLM module to request tool execution
/// without knowing the implementation details (e.g., DAG nodes).
///
/// # Example Implementation
///
/// ```rust,ignore
/// use colmena::llm::domain::{ToolExecutor, ToolCall, ToolResult, ToolDefinition, LlmError};
/// use async_trait::async_trait;
///
/// struct MyToolExecutor {
///     // ... fields
/// }
///
/// #[async_trait]
/// impl ToolExecutor for MyToolExecutor {
///     async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, LlmError> {
///         // Parse arguments
///         let args: serde_json::Value = serde_json::from_str(&tool_call.function.arguments)?;
///
///         // Execute the tool
///         let result = match tool_call.function.name.as_str() {
///             "add" => {
///                 let a = args["a"].as_f64().unwrap_or(0.0);
///                 let b = args["b"].as_f64().unwrap_or(0.0);
///                 (a + b).to_string()
///             }
///             _ => return Err(LlmError::ToolNotFound { name: tool_call.function.name.clone() })
///         };
///
///         Ok(ToolResult::success(tool_call.id.clone(), result))
///     }
///
///     async fn available_tools(&self) -> Vec<ToolDefinition> {
///         // Return list of available tools
///         vec![]
///     }
/// }
/// ```
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a tool call and return the result
    ///
    /// # Arguments
    /// * `tool_call` - The tool call to execute, containing the function name and arguments
    ///
    /// # Returns
    /// * `Ok(ToolResult)` - Successful execution with output
    /// * `Err(LlmError)` - Execution failed (tool not found, invalid arguments, etc.)
    ///
    /// # Example
    /// ```rust,ignore
    /// let tool_call = ToolCall {
    ///     id: "call_123".to_string(),
    ///     call_type: "function".to_string(),
    ///     function: FunctionCall {
    ///         name: "add".to_string(),
    ///         arguments: r#"{"a": 5, "b": 3}"#.to_string(),
    ///     }
    /// };
    ///
    /// let result = executor.execute(&tool_call).await?;
    /// assert_eq!(result.output, "8");
    /// ```
    async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, LlmError>;

    /// Get list of available tools
    ///
    /// This method is typically not used directly by the agent service.
    /// Instead, the LlmNode filters tools based on the `enabled_tools` config
    /// and passes the filtered list to the agent.
    ///
    /// # Returns
    /// Vector of tool definitions that can be passed to LLM
    ///
    /// # Note
    /// In practice, this may return an empty vector, as tool filtering
    /// is done at the LlmNode level using `get_tools()` or `get_all_available_tools()`.
    async fn available_tools(&self) -> Vec<ToolDefinition>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{FunctionCall, ToolResult};

    // Mock implementation for testing
    struct MockToolExecutor;

    #[async_trait]
    impl ToolExecutor for MockToolExecutor {
        async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, LlmError> {
            match tool_call.function.name.as_str() {
                "test_tool" => Ok(ToolResult::success(
                    tool_call.id.clone(),
                    "test_output".to_string(),
                )),
                _ => Err(LlmError::ToolNotFound {
                    name: tool_call.function.name.clone(),
                }),
            }
        }

        async fn available_tools(&self) -> Vec<ToolDefinition> {
            vec![]
        }
    }

    #[tokio::test]
    async fn test_mock_executor_success() {
        let executor = MockToolExecutor;
        let tool_call = ToolCall::new(
            "call_123".to_string(),
            FunctionCall::new("test_tool".to_string(), "{}".to_string()),
        );

        let result = executor.execute(&tool_call).await.unwrap();
        assert_eq!(result.tool_call_id, "call_123");
        assert!(result.success);
        assert_eq!(result.output, "test_output");
    }

    #[tokio::test]
    async fn test_mock_executor_tool_not_found() {
        let executor = MockToolExecutor;
        let tool_call = ToolCall::new(
            "call_456".to_string(),
            FunctionCall::new("unknown_tool".to_string(), "{}".to_string()),
        );

        let result = executor.execute(&tool_call).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            LlmError::ToolNotFound { name } => {
                assert_eq!(name, "unknown_tool");
            }
            _ => panic!("Expected ToolNotFound error"),
        }
    }
}
