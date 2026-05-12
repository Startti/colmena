use crate::llm::domain::{
    ConversationKey, ConversationRepository, LlmConfig, LlmError, LlmMessage, LlmRepository,
    LlmRequest, LlmResponse, LlmStreamPart, LlmUsage, ToolCall, ToolDefinition, ToolExecutor,
    ToolResult,
};
use std::sync::Arc;

/// A closure that derives the tool list to send on each ReAct iteration from
/// the current message history. Used to implement lazy tool loading without
/// teaching `AgentService` about lazy mode itself.
pub type ToolsProvider = Box<dyn Fn(&[LlmMessage]) -> Vec<ToolDefinition> + Send + Sync>;

/// Parameters for running the agent
pub struct AgentRunParams<'a> {
    pub session_id: &'a ConversationKey,
    pub prompt: Option<String>,
    pub messages: Option<Vec<LlmMessage>>,
    pub config: LlmConfig,
    pub tools: Vec<ToolDefinition>,
    pub tool_executor: &'a dyn ToolExecutor,
    pub max_iterations: Option<usize>,
    pub on_token: Option<Box<dyn Fn(LlmStreamPart) + Send + Sync>>,
    /// Optional dynamic tools provider, called fresh at each ReAct iteration.
    /// When `Some`, its return value REPLACES `tools` for that iteration.
    /// When `None`, `tools` is used unchanged each iteration (default).
    pub tools_provider: Option<ToolsProvider>,
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
        let session_id = params.session_id;
        let prompt = params.prompt;
        let config = params.config;
        let tools = params.tools;
        let tool_executor = params.tool_executor;
        let on_token = params.on_token;
        let tools_provider = params.tools_provider;

        // 1. Load conversation history
        let conversation = self.conversation_repository.get_by_id(session_id).await?;
        let mut messages = conversation.messages;

        // 2. Add user prompt (or pre-built messages)
        //    When `prompt` is `None` and `messages` is `None`, we continue from
        //    whatever is already in the conversation (resume path).
        if let Some(custom_messages) = params.messages {
            for custom_msg in custom_messages {
                messages.push(custom_msg.clone());
                self.conversation_repository
                    .add_message(session_id, custom_msg)
                    .await?;
            }
        } else if let Some(p) = prompt {
            let user_message = LlmMessage::user(p)?;
            messages.push(user_message.clone());
            self.conversation_repository
                .add_message(session_id, user_message)
                .await?;
        }
        // else: prompt is None — continue from existing history (resume path)

        let mut cumulative_usage = LlmUsage::default();
        let mut all_tool_calls_executed = Vec::new();
        let mut cumulative_content = String::new();

        // 3. ReAct Loop
        for _iteration in 0..max_iter {
            tracing::info!(
                target: "colmena::agent",
                iteration = _iteration,
                max = max_iter,
                "agent_service: iteration start"
            );
            // Signal start of a new message/iteration
            if let Some(callback) = &on_token {
                (callback)(LlmStreamPart::LlmMessageStart);
            }

            // A. Call LLM with tools
            let should_stream = on_token.is_some();
            let iteration_tools: Vec<ToolDefinition> = match &tools_provider {
                Some(p) => p(&messages),
                None => tools.clone(),
            };
            let mut request = LlmRequest::new(messages.clone(), config.clone(), should_stream)?;
            if !iteration_tools.is_empty() {
                request = request.with_tools(iteration_tools);
            }

            // Decide between call() and stream()
            let mut completion_usage = None;
            let mut response = if let Some(callback) = &on_token {
                let stream = self.llm_repository.stream(request).await?;
                use futures::StreamExt;
                // Pin stream
                let mut stream = stream;

                let mut full_content = String::new();
                let mut full_thinking = String::new();
                let mut captured_provider = config.provider().clone();
                let mut captured_req_id = crate::llm::domain::LlmRequestId::new();
                let mut accumulated_tool_calls: std::collections::HashMap<usize, ToolCall> =
                    std::collections::HashMap::new();

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
                                LlmStreamPart::ThinkingContent(c) => {
                                    full_thinking.push_str(c);
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
                                    if !tc.id.is_empty() && entry.id.is_empty() {
                                        entry.id = tc.id.clone();
                                    }
                                    if !tc.name.is_empty() && entry.function.name.is_empty() {
                                        entry.function.name = tc.name.clone();
                                    }
                                    entry.function.arguments.push_str(&tc.args_chunk);
                                }
                                LlmStreamPart::Usage(u) => {
                                    completion_usage = Some(u.clone());
                                }
                                LlmStreamPart::ThinkingStart
                                | LlmStreamPart::ThinkingEnd
                                | LlmStreamPart::LlmToolCallStart(_)
                                | LlmStreamPart::LlmToolCallFinish(_)
                                | LlmStreamPart::LlmMessageStart
                                | LlmStreamPart::LlmMessageFinish(_) => {}
                            }
                        }
                        Err(e) => return Err(e),
                    }
                }

                let mut final_response =
                    LlmResponse::new(captured_req_id, full_content, captured_provider)?;

                if !full_thinking.is_empty() {
                    final_response = final_response.with_thinking_content(full_thinking);
                }

                if !accumulated_tool_calls.is_empty() {
                    let tools: Vec<ToolCall> = accumulated_tool_calls.into_values().collect();
                    final_response = final_response.with_tool_calls(tools);
                }

                if let Some(usage) = &completion_usage {
                    final_response = final_response.with_usage(usage.clone());
                }

                final_response
            } else {
                let res = self.llm_repository.call(request).await?;
                completion_usage = res.usage().cloned();
                res
            };

            // Signal end of message/iteration
            if let Some(callback) = &on_token {
                (callback)(LlmStreamPart::LlmMessageFinish(completion_usage));
            }

            // Accumulate usage for this step
            if let Some(usage) = response.usage() {
                cumulative_usage.prompt_tokens += usage.prompt_tokens;
                cumulative_usage.completion_tokens += usage.completion_tokens;
                cumulative_usage.total_tokens += usage.total_tokens;
                if let Some(t) = usage.thinking_tokens {
                    *cumulative_usage.thinking_tokens.get_or_insert(0) += t;
                }
                if let Some(cr) = usage.cache_read_tokens {
                    *cumulative_usage.cache_read_tokens.get_or_insert(0) += cr;
                }
                if let Some(cw) = usage.cache_write_tokens {
                    *cumulative_usage.cache_write_tokens.get_or_insert(0) += cw;
                }
            }

            // B. Save assistant response to memory
            self.conversation_repository
                .add_message(session_id, response.message().clone())
                .await?;
            messages.push(response.message().clone());

            // Accumulate content
            let content = response.content();
            if !content.is_empty() {
                if !cumulative_content.is_empty() {
                    cumulative_content.push_str("\n\n");
                }
                cumulative_content.push_str(content);
            }

            // C. Check if LLM wants to use tools (Response might not have tool calls if streamed!)
            if let Some(tool_calls) = response.tool_calls() {
                if tool_calls.is_empty() {
                    response = response.with_usage(cumulative_usage);
                    response = response.with_content(cumulative_content);
                    if !all_tool_calls_executed.is_empty() {
                        response = response.with_tool_calls(all_tool_calls_executed);
                    }
                    return Ok(response);
                }

                // D. Execute each tool call
                for tool_call in tool_calls {
                    let mut executed_call = tool_call.clone();

                    // Notify start of execution
                    if let Some(callback) = &on_token {
                        (callback)(LlmStreamPart::LlmToolCallStart(tool_call.clone()));
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

                    // Detect SUSPENDED before persisting the tool message.
                    // The assistant message (with tool_calls) was already persisted above
                    // (step B), so the resume path can walk the history to find the pending
                    // tool call. We must NOT persist the tool result — we don't have one yet.
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&result.output) {
                        if parsed.get("__colmena_status").and_then(|v| v.as_str())
                            == Some("SUSPENDED")
                        {
                            tracing::info!(
                                target: "colmena::agent",
                                tool_call_id = %result.tool_call_id,
                                "agent_service: SUSPENDED detected in tool result, short-circuiting agent loop"
                            );
                            let questions = parsed
                                .get("questions")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            return Ok(LlmResponse::suspended(
                                result.tool_call_id.clone(),
                                questions,
                                result.output.clone(),
                            ));
                        }
                    }

                    // Populate tool call output tracker
                    let parsed_output = serde_json::from_str::<serde_json::Value>(&result.output)
                        .unwrap_or_else(|_| serde_json::Value::String(result.output.clone()));
                    executed_call.response = Some(parsed_output);
                    all_tool_calls_executed.push(executed_call);

                    // Notify result of execution
                    if let Some(callback) = &on_token {
                        (callback)(LlmStreamPart::LlmToolCallFinish(result.clone()));
                    }

                    let tool_message =
                        LlmMessage::tool(result.tool_call_id.clone(), result.output.clone())?;

                    messages.push(tool_message.clone());
                    self.conversation_repository
                        .add_message(session_id, tool_message)
                        .await?;
                }
                continue;
            } else {
                response = response.with_usage(cumulative_usage);
                response = response.with_content(cumulative_content);
                if !all_tool_calls_executed.is_empty() {
                    response = response.with_tool_calls(all_tool_calls_executed);
                }
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
            async fn get_by_id(&self, key: &ConversationKey) -> Result<Conversation, LlmError>;
            async fn add_message(&self, key: &ConversationKey, message: LlmMessage) -> Result<(), LlmError>;
            async fn delete(&self, key: &ConversationKey) -> Result<(), LlmError>;
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

    fn test_key() -> ConversationKey {
        ConversationKey {
            session_id: SessionId("test-thread".to_string()),
            agent_session_id: None,
            node_id: NodeIdPath("agent_service".to_string()),
        }
    }

    #[tokio::test]
    async fn test_agent_service_simple_response_no_tools() {
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_conv = MockConversationRepo::new();
        let mock_tool_exec = MockToolExec::new();

        let key = test_key();
        let prompt = "Hello".to_string();

        // Setup Conversation Repo
        mock_conv
            .expect_get_by_id()
            .with(eq(key.clone()))
            .times(1)
            .returning(|k| {
                Ok(Conversation {
                    key: k.clone(),
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
                session_id: &key,
                prompt: Some(prompt),
                messages: None,
                config: create_config(),
                tools: vec![],
                tool_executor: &mock_tool_exec,
                max_iterations: None,
                on_token: None,
                tools_provider: None,
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

        let key = test_key();
        let prompt = "Add 2+2".to_string();

        // Setup Conversation Repo
        mock_conv.expect_get_by_id().returning(|k| {
            Ok(Conversation {
                key: k.clone(),
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
                    response: None,
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
                session_id: &key,
                prompt: Some(prompt),
                messages: None,
                config: create_config(),
                tools: vec![], // Tools list doesn't matter for mock
                tool_executor: &mock_tool_exec,
                max_iterations: None,
                on_token: None,
                tools_provider: None,
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content(), "The answer is 4");
    }

    /// When `prompt` is `None` and existing messages are already in the
    /// conversation, the agent must continue from those messages WITHOUT
    /// pushing a new user message into the conversation.
    #[tokio::test]
    async fn run_with_no_prompt_continues_from_existing_messages() {
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_conv = MockConversationRepo::new();
        let mock_tool_exec = MockToolExec::new();

        let key = test_key();

        // Conversation already has a prior user message (simulating resume)
        mock_conv.expect_get_by_id().times(1).returning(|k| {
            Ok(Conversation {
                key: k.clone(),
                messages: vec![LlmMessage::user("original question".to_string()).unwrap()],
            })
        });

        // Only the assistant reply must be persisted — no new user message
        mock_conv
            .expect_add_message()
            .times(1) // exactly 1: the assistant response
            .returning(|_, _| Ok(()));

        // LLM returns a simple final answer
        mock_llm.expect_call().times(1).returning(|_| {
            Ok(LlmResponse::new(
                LlmRequestId::from_string("req-resume".to_string()).unwrap(),
                "Resumed answer".to_string(),
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
                session_id: &key,
                prompt: None,
                messages: None,
                config: create_config(),
                tools: vec![],
                tool_executor: &mock_tool_exec,
                max_iterations: None,
                on_token: None,
                tools_provider: None,
            })
            .await;

        assert!(result.is_ok(), "run with None prompt must succeed");
        assert_eq!(result.unwrap().content(), "Resumed answer");
    }

    #[tokio::test]
    async fn test_agent_service_max_iterations() {
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_conv = MockConversationRepo::new();
        let mut mock_tool_exec = MockToolExec::new();

        let key = test_key();

        mock_conv.expect_get_by_id().returning(|k| {
            Ok(Conversation {
                key: k.clone(),
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
                response: None,
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
                session_id: &key,
                prompt: Some("Loop me".to_string()),
                messages: None,
                config: create_config(),
                tools: vec![],
                tool_executor: &mock_tool_exec,
                max_iterations: Some(3), // Max 3 iterations
                on_token: None,
                tools_provider: None,
            })
            .await;

        assert!(matches!(
            result,
            Err(LlmError::MaxIterationsReached { max: 3 })
        ));
    }

    /// When a tool returns `__colmena_status: "SUSPENDED"` the agent service must
    /// stop iterating, persist only the assistant message (not a tool message), and
    /// return an `LlmResponse` whose `suspend()` is `Some`.
    #[tokio::test]
    async fn detects_suspended_tool_result_and_short_circuits() {
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_conv = MockConversationRepo::new();
        let mut mock_tool_exec = MockToolExec::new();

        let key = test_key();
        let prompt = "hello".to_string();

        // Conversation starts empty
        mock_conv.expect_get_by_id().times(1).returning(|k| {
            Ok(Conversation {
                key: k.clone(),
                messages: vec![],
            })
        });

        // Exactly 2 add_message calls:
        //   1. user message
        //   2. assistant message (with tool_calls) — already persisted before the tool loop
        // The tool result must NOT be persisted (we short-circuit before that).
        mock_conv
            .expect_add_message()
            .times(2)
            .returning(|_, _| Ok(()));

        // LLM returns a response with one tool call
        mock_llm.expect_call().times(1).returning(|_| {
            let tool_call = ToolCall {
                id: "call_xyz".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "suspend_tool".to_string(),
                    arguments: "{}".to_string(),
                },
                response: None,
            };
            Ok(LlmResponse::new(
                LlmRequestId::from_string("req-susp".to_string()).unwrap(),
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

        // Tool executor returns a SUSPENDED payload
        mock_tool_exec.expect_execute().times(1).returning(|call| {
            Ok(ToolResult {
                tool_call_id: call.id.clone(),
                success: true,
                output: r#"{"__colmena_status":"SUSPENDED","questions":[{"id":"q1","question":"x?","type":"secret"}]}"#
                    .to_string(),
                error: None,
            })
        });

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));

        let result = service
            .run(AgentRunParams {
                session_id: &key,
                prompt: Some(prompt),
                messages: None,
                config: create_config(),
                tools: vec![],
                tool_executor: &mock_tool_exec,
                max_iterations: None,
                on_token: None,
                tools_provider: None,
            })
            .await;

        assert!(result.is_ok(), "run must succeed: {:?}", result.err());
        let response = result.unwrap();

        // Must carry suspend info
        let suspend = response.suspend().expect("response must have suspend info");
        assert_eq!(suspend.tool_call_id, "call_xyz");

        let questions = suspend
            .questions
            .as_array()
            .expect("questions must be an array");
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0]["id"], "q1");
    }
}
