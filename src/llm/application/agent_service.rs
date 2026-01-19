use crate::llm::domain::{
    ConversationRepository, LlmConfig, LlmError, LlmMessage, LlmRepository, LlmRequest,
    LlmResponse, LlmStreamPart, ThreadId, ToolCall, ToolDefinition, ToolExecutor, ToolResult,
};
use std::sync::Arc;

/// Parameters for running the agent
pub struct AgentRunParams<'a> {
    pub thread_id: &'a ThreadId,
    pub prompt: String,
    pub config: LlmConfig,
    pub tools: Vec<ToolDefinition>,
    pub tool_executor: &'a dyn ToolExecutor,
    pub max_iterations: Option<usize>,
    pub on_token: Option<Box<dyn Fn(LlmStreamPart) + Send + Sync>>,
}

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
    /// * `params` - Agent execution parameters
    ///
    /// # Returns
    /// Final response from the LLM after tool execution
    pub async fn run<'a>(&self, params: AgentRunParams<'a>) -> Result<LlmResponse, LlmError> {
        let max_iter = params.max_iterations.unwrap_or(10);
        let thread_id = params.thread_id;
        let prompt = params.prompt;
        let config = params.config;
        let tools = params.tools;
        let tool_executor = params.tool_executor;
        let on_token = params.on_token;

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
            // A. Call LLM with tools
            let should_stream = on_token.is_some();
            let mut request = LlmRequest::new(messages.clone(), config.clone(), should_stream)?;
            if !tools.is_empty() {
                request = request.with_tools(tools.clone());
            }

            // Decide between call() and stream()
            // We use stream() ONLY if on_token is present AND this is likely a generation step
            // But we don't know if it's a tool call step or generation step until we get the response.
            // So we must use stream() if on_token is provided, and handle re-construction.

            let response = if let Some(callback) = &on_token {
                let stream = self.llm_repository.stream(request).await?;
                use futures::StreamExt;
                // Pin stream
                let mut stream = stream;

                let mut full_content = String::new();
                let mut captured_provider = config.provider().clone();
                let mut captured_req_id = crate::llm::domain::LlmRequestId::new();
                let mut accumulated_tool_calls: std::collections::HashMap<usize, ToolCall> =
                    std::collections::HashMap::new();
                let mut completion_usage = None;

                while let Some(chunk_result) = stream.next().await {
                    match chunk_result {
                        Ok(chunk) => {
                            captured_req_id = chunk.request_id().clone();
                            captured_provider = chunk.provider().clone();

                            // Forward the part to the callback
                            (callback)(chunk.part().clone());

                            // Accumulate state for returning LlmResponse
                            match chunk.part() {
                                LlmStreamPart::Content(c) => {
                                    full_content.push_str(c);
                                }
                                LlmStreamPart::ToolCallChunk(tc) => {
                                    let entry = accumulated_tool_calls
                                        .entry(tc.index)
                                        .or_insert_with(|| {
                                            ToolCall::new(
                                                tc.id.clone(),
                                                crate::llm::domain::FunctionCall::new(
                                                    tc.name.clone(),
                                                    String::new(),
                                                ),
                                            )
                                        });
                                    // If ID arrives in first chunk (it should), but just in case logic updates
                                    if !tc.id.is_empty() && entry.id.is_empty() {
                                        entry.id = tc.id.clone();
                                    }
                                    // If name arrives in chunks, append it (usually name is in first chunk but ensuring)
                                    if !tc.name.is_empty() && entry.function.name.is_empty() {
                                        entry.function.name = tc.name.clone();
                                    }
                                    entry.function.arguments.push_str(&tc.args_chunk);
                                }
                                LlmStreamPart::Usage(u) => {
                                    completion_usage = Some(u.clone());
                                }
                                LlmStreamPart::ToolCallStart(_)
                                | LlmStreamPart::ToolCallFinish(_) => {}
                            }
                        }
                        Err(e) => return Err(e),
                    }
                }

                let mut final_response =
                    LlmResponse::new(captured_req_id, full_content, captured_provider)?;

                if !accumulated_tool_calls.is_empty() {
                    let tools: Vec<ToolCall> = accumulated_tool_calls.into_values().collect();
                    final_response = final_response.with_tool_calls(tools);
                }

                if let Some(usage) = completion_usage {
                    final_response = final_response.with_usage(usage);
                }

                final_response
            } else {
                self.llm_repository.call(request).await?
            };

            // B. Save assistant response to memory
            self.conversation_repository
                .add_message(thread_id, response.message().clone())
                .await?;
            messages.push(response.message().clone());

            // C. Check if LLM wants to use tools (Response might not have tool calls if streamed!)
            if let Some(tool_calls) = response.tool_calls() {
                if tool_calls.is_empty() {
                    return Ok(response);
                }
                // D. Execute each tool call
                for tool_call in tool_calls {
                    // Notify start of execution
                    if let Some(callback) = &on_token {
                        (callback)(LlmStreamPart::ToolCallStart(tool_call.clone()));
                    }

                    let result = match tool_executor.execute(tool_call).await {
                        Ok(res) => res,
                        Err(e) => ToolResult {
                            tool_call_id: tool_call.id.clone(),
                            success: false,
                            output: format!("Error executing tool: {}", e),
                            error: Some(e.to_string()),
                        },
                    };

                    // Notify result of execution
                    if let Some(callback) = &on_token {
                        (callback)(LlmStreamPart::ToolCallFinish(result.clone()));
                    }

                    let tool_message =
                        LlmMessage::tool(result.tool_call_id.clone(), result.output.clone())?;

                    messages.push(tool_message.clone());
                    self.conversation_repository
                        .add_message(thread_id, tool_message)
                        .await?;
                }
                continue;
            } else {
                return Ok(response);
            }
        }

        Err(LlmError::MaxIterationsReached { max: max_iter })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::*;
    use async_trait::async_trait;

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
        LlmConfig::new(
            LlmProvider::new(
                ProviderKind::OpenAi,
                "key".to_string(),
                Some("gpt-4".to_string()),
            )
            .unwrap(),
        )
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
            .returning(|_| {
                Ok(Conversation {
                    thread_id: ThreadId("test-thread".to_string()),
                    messages: vec![],
                })
            });

        mock_conv
            .expect_add_message()
            .times(2) // 1 user message, 1 assistant message
            .returning(|_, _| Ok(()));

        // Setup LLM Repo
        mock_llm.expect_call().times(1).returning(|_| {
            Ok(LlmResponse::new(
                LlmRequestId::from_string("req-1".to_string()).unwrap(),
                "Hi there!".to_string(),
                LlmProvider::new(
                    ProviderKind::OpenAi,
                    "key".to_string(),
                    Some("gpt-4".to_string()),
                )
                .unwrap(),
            )
            .unwrap())
        });

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));

        let result = service
            .run(AgentRunParams {
                thread_id: &thread_id,
                prompt,
                config: create_config(),
                tools: vec![],
                tool_executor: &mock_tool_exec,
                max_iterations: None,
                on_token: None,
            })
            .await;

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
        mock_conv.expect_get_by_id().returning(|_| {
            Ok(Conversation {
                thread_id: ThreadId("test-thread".to_string()),
                messages: vec![],
            })
        });

        mock_conv.expect_add_message().returning(|_, _| Ok(()));

        // Setup Tool Executor
        mock_tool_exec.expect_execute().times(1).returning(|call| {
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
                    LlmProvider::new(
                        ProviderKind::OpenAi,
                        "key".to_string(),
                        Some("gpt-4".to_string()),
                    )
                    .unwrap(),
                )
                .unwrap()
                .with_tool_calls(vec![tool_call]))
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
                    LlmProvider::new(
                        ProviderKind::OpenAi,
                        "key".to_string(),
                        Some("gpt-4".to_string()),
                    )
                    .unwrap(),
                )
                .unwrap())
            });

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));

        let result = service
            .run(AgentRunParams {
                thread_id: &thread_id,
                prompt,
                config: create_config(),
                tools: vec![], // Tools list doesn't matter for mock
                tool_executor: &mock_tool_exec,
                max_iterations: None,
                on_token: None,
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content(), "The answer is 4");
    }

    #[tokio::test]
    async fn test_agent_service_max_iterations() {
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_conv = MockConversationRepo::new();
        let mut mock_tool_exec = MockToolExec::new();

        let thread_id = ThreadId("test-thread".to_string());

        mock_conv.expect_get_by_id().returning(|_| {
            Ok(Conversation {
                thread_id: ThreadId("test-thread".to_string()),
                messages: vec![],
            })
        });
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
                LlmProvider::new(
                    ProviderKind::OpenAi,
                    "key".to_string(),
                    Some("gpt-4".to_string()),
                )
                .unwrap(),
            )
            .unwrap()
            .with_tool_calls(vec![tool_call]))
        });

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));

        let result = service
            .run(AgentRunParams {
                thread_id: &thread_id,
                prompt: "Loop me".to_string(),
                config: create_config(),
                tools: vec![],
                tool_executor: &mock_tool_exec,
                max_iterations: Some(3), // Max 3 iterations
                on_token: None,
            })
            .await;

        assert!(matches!(
            result,
            Err(LlmError::MaxIterationsReached { max: 3 })
        ));
    }
}
