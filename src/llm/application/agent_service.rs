use crate::llm::domain::{
    ConversationRepository, LlmConfig, LlmError, LlmMessage, LlmRepository, LlmRequest,
    LlmResponse, ThreadId, ToolDefinition, ToolExecutor, ToolResult,
};
use std::sync::Arc;

/// Agent service implementing the ReAct (Reasoning + Acting) pattern
///
/// This service orchestrates the LLM reasoning loop:
/// 1. LLM thinks and may request tool execution
/// 2. Tools are executed via ToolExecutor
/// 3. Results are fed back to LLM
/// 4. Loop continues until LLM provides final answer
pub struct AgentService {
    llm_repository: Arc<dyn LlmRepository>,
    conversation_repository: Arc<dyn ConversationRepository>,
}

impl AgentService {
    pub fn new(
        llm_repository: Arc<dyn LlmRepository>,
        conversation_repository: Arc<dyn ConversationRepository>,
    ) -> Self {
        Self {
            llm_repository,
            conversation_repository,
        }
    }

    /// Run the agent with tool execution capabilities
    ///
    /// # Arguments
    /// * `thread_id` - Conversation thread for memory
    /// * `prompt` - User's prompt/request
    /// * `config` - LLM configuration
    /// * `tools` - List of tools available to the agent (from enabled_tools config)
    /// * `tool_executor` - Implementation that executes tools
    /// * `max_iterations` - Safety limit for ReAct loop (default: 10)
    ///
    /// # Returns
    /// Final response from the LLM after tool execution
    pub async fn run(
        &self,
        thread_id: &ThreadId,
        prompt: String,
        config: LlmConfig,
        tools: Vec<ToolDefinition>,
        tool_executor: &dyn ToolExecutor,
        max_iterations: Option<usize>,
    ) -> Result<LlmResponse, LlmError> {
        let max_iter = max_iterations.unwrap_or(10);

        // 1. Load conversation history
        let conversation = self.conversation_repository.get_by_id(thread_id).await?;
        let mut messages = conversation.messages;

        // 2. Add user prompt
        let user_message = LlmMessage::user(prompt)?;
        messages.push(user_message.clone());
        self.conversation_repository
            .add_message(thread_id, user_message)
            .await?;

        // 3. ReAct Loop
        for _iteration in 0..max_iter {
            // A. Call LLM with tools (only if tools are provided)
            let mut request = LlmRequest::new(messages.clone(), config.clone(), false)?;
            if !tools.is_empty() {
                request = request.with_tools(tools.clone());
            }

            let response = self.llm_repository.call(request).await?;

            // B. Save assistant response to memory
            self.conversation_repository
                .add_message(thread_id, response.message().clone())
                .await?;
            messages.push(response.message().clone());

            // C. Check if LLM wants to use tools
            if let Some(tool_calls) = response.tool_calls() {
                if tool_calls.is_empty() {
                    // No tool calls, return response
                    return Ok(response);
                }
                // D. Execute each tool call
                for tool_call in tool_calls {
                    // Execute tool
                    let result = match tool_executor.execute(tool_call).await {
                        Ok(res) => res,
                        Err(e) => {
                            // If execution fails, we still need to report it to the LLM
                            // so it can try again or apologize
                            ToolResult {
                                tool_call_id: tool_call.id.clone(),
                                success: false,
                                output: format!("Error executing tool: {}", e),
                                error: Some(e.to_string()),
                            }
                        }
                    };

                    // E. Create tool result message
                    let tool_message = LlmMessage::tool(
                        result.tool_call_id.clone(),
                        result.output.clone(),
                    )?;

                    // F. Add to conversation
                    messages.push(tool_message.clone());
                    self.conversation_repository
                        .add_message(thread_id, tool_message)
                        .await?;
                }

                // Continue loop - LLM will see tool results in next iteration
                continue;
            } else {
                // No tool calls - final response
                return Ok(response);
            }
        }

        // Safety: Max iterations reached
        Err(LlmError::MaxIterationsReached { max: max_iter })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use mockall::mock;
    use mockall::predicate::*;
    use std::sync::Arc;

    // Mock LlmRepository
    mock! {
        pub LlmRepo {}
        #[async_trait]
        impl LlmRepository for LlmRepo {
            async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;
            async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError>;
            async fn health_check(&self) -> Result<(), LlmError>;
            fn provider_name(&self) -> &'static str;
        }
    }

    // Mock ConversationRepository
    mock! {
        pub ConversationRepo {}
        #[async_trait]
        impl ConversationRepository for ConversationRepo {
            async fn get_by_id(&self, thread_id: &ThreadId) -> Result<Conversation, LlmError>;
            async fn add_message(&self, thread_id: &ThreadId, message: LlmMessage) -> Result<(), LlmError>;
            async fn delete(&self, thread_id: &ThreadId) -> Result<(), LlmError>;
        }
    }

    // Mock ToolExecutor
    mock! {
        pub ToolExec {}
        #[async_trait]
        impl ToolExecutor for ToolExec {
            async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, LlmError>;
            async fn available_tools(&self) -> Vec<ToolDefinition>;
        }
    }

    fn create_config() -> LlmConfig {
        LlmConfig::new(LlmProvider::new(ProviderKind::OpenAi, "key".to_string(), Some("gpt-4".to_string())).unwrap())
    }

    #[tokio::test]
    async fn test_agent_service_simple_response_no_tools() {
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_conv = MockConversationRepo::new();
        let mock_tool_exec = MockToolExec::new();

        let thread_id = ThreadId("test-thread".to_string());
        let prompt = "Hello".to_string();

        // Setup Conversation Repo
        mock_conv
            .expect_get_by_id()
            .with(eq(thread_id.clone()))
            .times(1)
            .returning(|_| Ok(Conversation {
                thread_id: ThreadId("test-thread".to_string()),
                messages: vec![],
            }));

        mock_conv
            .expect_add_message()
            .times(2) // 1 user message, 1 assistant message
            .returning(|_, _| Ok(()));

        // Setup LLM Repo
        mock_llm
            .expect_call()
            .times(1)
            .returning(|_| {
                Ok(LlmResponse::new(
                    LlmRequestId::from_string("req-1".to_string()).unwrap(),
                    "Hi there!".to_string(),
                    LlmProvider::new(ProviderKind::OpenAi, "key".to_string(), Some("gpt-4".to_string())).unwrap(),
                ).unwrap())
            });

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
        
        let result = service.run(
            &thread_id,
            prompt,
            create_config(),
            vec![],
            &mock_tool_exec,
            None
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content(), "Hi there!");
    }

    #[tokio::test]
    async fn test_agent_service_with_tool_call() {
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_conv = MockConversationRepo::new();
        let mut mock_tool_exec = MockToolExec::new();

        let thread_id = ThreadId("test-thread".to_string());
        let prompt = "Add 2+2".to_string();

        // Setup Conversation Repo
        mock_conv
            .expect_get_by_id()
            .returning(|_| Ok(Conversation {
                thread_id: ThreadId("test-thread".to_string()),
                messages: vec![],
            }));
        
        mock_conv.expect_add_message().returning(|_, _| Ok(()));

        // Setup Tool Executor
        mock_tool_exec
            .expect_execute()
            .times(1)
            .returning(|call| {
                Ok(ToolResult {
                    tool_call_id: call.id.clone(),
                    success: true,
                    output: "4".to_string(),
                    error: None,
                })
            });

        // Setup LLM Repo - Sequence of responses
        let mut seq = mockall::Sequence::new();

        // 1. First call returns tool call
        mock_llm
            .expect_call()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_| {
                let tool_call = ToolCall {
                    id: "call_1".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "add".to_string(),
                        arguments: "{\"a\": 2, \"b\": 2}".to_string(),
                    },
                };
                
                Ok(LlmResponse::new(
                    LlmRequestId::from_string("req-1".to_string()).unwrap(),
                    "".to_string(),
                    LlmProvider::new(ProviderKind::OpenAi, "key".to_string(), Some("gpt-4".to_string())).unwrap(),
                ).unwrap().with_tool_calls(vec![tool_call]))
            });

        // 2. Second call returns final answer
        mock_llm
            .expect_call()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_| {
                Ok(LlmResponse::new(
                    LlmRequestId::from_string("req-2".to_string()).unwrap(),
                    "The answer is 4".to_string(),
                    LlmProvider::new(ProviderKind::OpenAi, "key".to_string(), Some("gpt-4".to_string())).unwrap(),
                ).unwrap())
            });

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
        
        let result = service.run(
            &thread_id,
            prompt,
            create_config(),
            vec![], // Tools list doesn't matter for mock
            &mock_tool_exec,
            None
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content(), "The answer is 4");
    }

    #[tokio::test]
    async fn test_agent_service_max_iterations() {
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_conv = MockConversationRepo::new();
        let mut mock_tool_exec = MockToolExec::new();

        let thread_id = ThreadId("test-thread".to_string());

        mock_conv.expect_get_by_id().returning(|_| Ok(Conversation {
            thread_id: ThreadId("test-thread".to_string()),
            messages: vec![],
        }));
        mock_conv.expect_add_message().returning(|_, _| Ok(()));

        mock_tool_exec.expect_execute().returning(|call| {
            Ok(ToolResult {
                tool_call_id: call.id.clone(),
                success: true,
                output: "loop".to_string(),
                error: None,
            })
        });

        // Always return tool call
        mock_llm.expect_call().returning(|_| {
            let tool_call = ToolCall {
                id: "call_loop".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "loop".to_string(),
                    arguments: "{}".to_string(),
                },
            };
            
            Ok(LlmResponse::new(
                LlmRequestId::from_string("req-loop".to_string()).unwrap(),
                "".to_string(),
                LlmProvider::new(ProviderKind::OpenAi, "key".to_string(), Some("gpt-4".to_string())).unwrap(),
            ).unwrap().with_tool_calls(vec![tool_call]))
        });

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
        
        let result = service.run(
            &thread_id,
            "Loop me".to_string(),
            create_config(),
            vec![],
            &mock_tool_exec,
            Some(3) // Max 3 iterations
        ).await;

        assert!(matches!(result, Err(LlmError::MaxIterationsReached { max: 3 })));
    }
}
