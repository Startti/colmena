use crate::llm::domain::{
    ConversationKey, ConversationRepository, FileData, LlmConfig, LlmError, LlmMessage,
    LlmRepository, LlmRequest, LlmResponse, LlmStreamPart, LlmUsage, MessageRole, ToolCall,
    ToolDefinition, ToolExecutor, ToolResult,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Number of trailing messages to keep verbatim when compacting old
/// `load_skill` tool results in the request. `keep_recent_msgs = 8` covers
/// roughly the last 3 ReAct turns (assistant + 1-2 tool results per turn);
/// anything older with `load_skill` content is replaced by a short marker.
/// Set to `usize::MAX` to disable.
const COMPACT_LOAD_SKILL_KEEP_RECENT_MSGS: usize = 8;

/// F-T15 — Rolling summary parameters (append-only, line-per-message).
/// When `messages.len() > KEEP_FIRST + KEEP_RECENT + 1`, the middle slice gets
/// replaced by a single System message that lists one line per source message
/// with its persisted-index in [T<idx>] notation. The persisted history
/// (`conversation_repository`) is unchanged so `recall_history(turn=N)` can
/// always return the original msg verbatim.
///
/// `KEEP_FIRST = 2`: first user prompt + system prelude block (always kept).
/// `KEEP_RECENT = 5`: last 5 messages stay verbatim (recent context the model
///   is actively processing).
/// `SUMMARY_MAX_LINES = 100`: cap on summary length; older lines are dropped
///   (the data still lives in `conversation_repository` for recall).
const COMPACT_SUMMARY_KEEP_FIRST_MSGS: usize = 2;
const COMPACT_SUMMARY_KEEP_RECENT_MSGS: usize = 5;
const COMPACT_SUMMARY_MAX_LINES: usize = 100;

/// Per-line truncation when building summary lines (chars, not tokens).
const COMPACT_SUMMARY_LINE_MAX_CHARS: usize = 180;

/// LLM-facing text shown when a tool call with an identical `(name+args)`
/// signature is repeated (loop guard). The prior result is prepended to this.
#[allow(dead_code)] // removed in Task 4 when the loop guard uses it
const REPEAT_NUDGE_TEXT: &str =
    include_str!("../../../text/prompts/agent_loop/repeat_nudge.md");

/// LLM-facing instruction for the forced final synthesis ("rescue"). Appended
/// as a user message before the terminal, tool-less LLM call.
#[allow(dead_code)] // removed in Task 4 when the loop guard uses it
const RESCUE_SYNTHESIS_TEXT: &str =
    include_str!("../../../text/prompts/agent_loop/rescue_synthesis.md");

/// A closure that derives the tool list to send on each ReAct iteration from
/// the current message history. Used to implement lazy tool loading without
/// teaching `AgentService` about lazy mode itself.
pub type ToolsProvider = Box<dyn Fn(&[LlmMessage]) -> Vec<ToolDefinition> + Send + Sync>;

/// Resolves a `document_id` into a ready-to-use `FileData` for the agent loop.
/// Implementations are responsible for:
///   1. Looking up the entry in AttachmentRegistry.
///   2. Verifying / refreshing the provider_file_id when expired (silent re-upload).
///   3. Returning a recoverable error string when re-upload is impossible.
///
/// Returning `Ok(None)` means the document_id is not in this session — the
/// agent loop will close the tool call with a `not_found` tool result.
#[async_trait::async_trait]
pub trait LoadAttachmentResolver: Send + Sync {
    async fn resolve(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<Option<FileData>, String>;
}

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
    /// Optional resolver invoked when a tool returns the LOAD_ATTACHMENT sentinel.
    /// When `None`, sentinels surface as ordinary tool results (no special handling).
    pub attachment_resolver: Option<Arc<dyn LoadAttachmentResolver>>,
    /// Agent session id used by the resolver. Required when `attachment_resolver`
    /// is `Some`; if missing while a sentinel is detected, the loop returns an
    /// `AttachmentError::SessionMissing` mapped to a tool result string.
    pub agent_session_id: Option<String>,
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
        let params_resolver = params.attachment_resolver;
        let params_agent_session_id = params.agent_session_id;

        // 1. Load conversation history
        let conversation = self.conversation_repository.get_by_id(session_id).await?;
        let mut messages = conversation.messages;

        // 1b. Migration shim (2026-06-11): conversations persisted BEFORE the
        // cache-safe temporal fix carry the `## Temporal & Geographic Context`
        // block baked into the FRONT of their system message. The fix now
        // injects a fresh temporal block per turn as a volatile suffix, so a
        // loaded pre-fix system would produce a stale duplicate. Strip the
        // leading temporal block from any system message loaded from history.
        // New conversations never hit this (their persisted system has no
        // temporal block). Drops a system message that was ONLY temporal.
        messages = messages
            .into_iter()
            .filter_map(|msg| {
                if msg.role() == &MessageRole::System {
                    let stripped = strip_leading_temporal_block(msg.content());
                    if stripped.trim().is_empty() {
                        None
                    } else if stripped == msg.content() {
                        Some(msg)
                    } else {
                        LlmMessage::system(stripped).ok()
                    }
                } else {
                    Some(msg)
                }
            })
            .collect();

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

            // A. Call LLM with tools
            let should_stream = on_token.is_some();
            let iteration_tools: Vec<ToolDefinition> = match &tools_provider {
                Some(p) => p(&messages),
                None => tools.clone(),
            };
            // F-T14 step A2 — skill-out-of-history.
            // Compact load_skill tool results older than 8 msgs into markers.
            // Persistence in `conversation_repository` is unchanged.
            let request_messages =
                compact_old_load_skill_in_history(&messages, COMPACT_LOAD_SKILL_KEEP_RECENT_MSGS);

            // F-T15 — rolling summary. Replace the middle of history
            // (between KEEP_FIRST and KEEP_RECENT) with a single System
            // message containing one summary line per source message,
            // tagged with the original turn index for recall_history.
            // Persistence in `conversation_repository` is unchanged so
            // `recall_history(turn=N)` always returns the original verbatim.
            let request_messages = compact_history_to_summary(
                &request_messages,
                COMPACT_SUMMARY_KEEP_FIRST_MSGS,
                COMPACT_SUMMARY_KEEP_RECENT_MSGS,
                COMPACT_SUMMARY_MAX_LINES,
                COMPACT_SUMMARY_LINE_MAX_CHARS,
            );

            let mut request = LlmRequest::new(request_messages, config.clone(), should_stream)?;
            if !iteration_tools.is_empty() {
                request = request.with_tools(iteration_tools.clone());
            }

            // Per-iteration prompt-size diagnostic. Gated by env var so it has
            // ZERO runtime cost when disabled. Used during F-T13/F-T14 to measure
            // token-optimization wins between commits.
            //   COLMENA_DUMP_PROMPT_SIZES=1  → one-line summary per iteration
            //   COLMENA_DUMP_PROMPT_FULL=1   → full per-message + per-tool breakdown
            if std::env::var("COLMENA_DUMP_PROMPT_SIZES").is_ok() {
                // Dump the FULLY-COMPACTED messages (after A2 + F-T15) — these
                // are what actually gets sent over the wire. Persisted history
                // in conversation_repository keeps the originals for recall.
                let after_a2 = compact_old_load_skill_in_history(
                    &messages,
                    COMPACT_LOAD_SKILL_KEEP_RECENT_MSGS,
                );
                let msgs_to_dump = compact_history_to_summary(
                    &after_a2,
                    COMPACT_SUMMARY_KEEP_FIRST_MSGS,
                    COMPACT_SUMMARY_KEEP_RECENT_MSGS,
                    COMPACT_SUMMARY_MAX_LINES,
                    COMPACT_SUMMARY_LINE_MAX_CHARS,
                );
                let msgs_json = serde_json::to_string(&msgs_to_dump).unwrap_or_default();
                let tools_json = serde_json::to_string(&iteration_tools).unwrap_or_default();
                let n_msgs = msgs_to_dump.len();
                let per_msg_sizes: Vec<usize> = msgs_to_dump
                    .iter()
                    .map(|m| serde_json::to_string(m).map(|s| s.len()).unwrap_or(0))
                    .collect();
                eprintln!(
                    "📊 [iter] msgs={} (json_chars={} ≈ {}T)  tools={} (json_chars={} ≈ {}T)  per_msg_sizes={:?}",
                    n_msgs,
                    msgs_json.len(),
                    msgs_json.len() / 4,
                    iteration_tools.len(),
                    tools_json.len(),
                    tools_json.len() / 4,
                    per_msg_sizes
                );

                // FULL DUMP — on EVERY iter when COLMENA_DUMP_PROMPT_FULL is set.
                // The last iter dump tells us the real cumulative cost at end of run.
                if std::env::var("COLMENA_DUMP_PROMPT_FULL").is_ok() {
                    eprintln!(
                        "\n🔬 [FULL DUMP iter (n_msgs={})] ───────────────────",
                        n_msgs
                    );
                    for (i, m) in msgs_to_dump.iter().enumerate() {
                        let s = serde_json::to_string(m).unwrap_or_default();
                        // Trim each to first 400 chars for sanity
                        let preview = if s.len() > 600 {
                            format!("{}...[+{} chars]", &s[..400], s.len() - 400)
                        } else {
                            s.clone()
                        };
                        eprintln!(
                            "  msg[{}] {}ch  {}T  ::  {}",
                            i,
                            s.len(),
                            s.len() / 4,
                            preview
                        );
                    }
                    // Per-tool size breakdown
                    eprintln!(
                        "  --- TOOLS ({} total, {} chars = {} T) ---",
                        iteration_tools.len(),
                        tools_json.len(),
                        tools_json.len() / 4
                    );
                    for (i, td) in iteration_tools.iter().enumerate() {
                        let s = serde_json::to_string(td).unwrap_or_default();
                        eprintln!(
                            "    tool[{}] {} ({}ch = {}T)",
                            i,
                            td.name,
                            s.len(),
                            s.len() / 4
                        );
                    }
                    eprintln!("──────────────────────────────────────────────\n");
                }
            }

            let (mut response, _completion_usage) =
                self.invoke_llm(request, &on_token, &config).await?;

            // Accumulate usage for this step
            accumulate_usage(&mut cumulative_usage, &response);

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
                    let parsed_sentinel =
                        serde_json::from_str::<serde_json::Value>(&result.output).ok();
                    if let Some(parsed) = parsed_sentinel.as_ref() {
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
                        if parsed.get("__colmena_status").and_then(|v| v.as_str())
                            == Some("LOAD_ATTACHMENT")
                        {
                            let document_id = parsed
                                .get("document_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            tracing::info!(
                                target: "colmena::attachment",
                                event = "attachment.loaded",
                                document_id = %document_id,
                                "LOAD_ATTACHMENT sentinel received"
                            );
                            let resolver = match &params_resolver {
                                Some(r) => r.clone(),
                                None => {
                                    let tool_message = LlmMessage::tool(
                                        result.tool_call_id.clone(),
                                        r#"{"error":"load_attachment_unsupported","reason":"no AttachmentResolver wired"}"#.to_string(),
                                    )?;
                                    messages.push(tool_message.clone());
                                    self.conversation_repository
                                        .add_message(session_id, tool_message)
                                        .await?;
                                    continue;
                                }
                            };
                            let sid = match params_agent_session_id.as_ref() {
                                Some(s) => s.clone(),
                                None => {
                                    let tool_message = LlmMessage::tool(
                                        result.tool_call_id.clone(),
                                        r#"{"error":"load_attachment_session_missing"}"#
                                            .to_string(),
                                    )?;
                                    messages.push(tool_message.clone());
                                    self.conversation_repository
                                        .add_message(session_id, tool_message)
                                        .await?;
                                    continue;
                                }
                            };
                            let resolved = resolver.resolve(&sid, document_id).await;
                            let (ack_text, synthetic_user) = match resolved {
                                Ok(Some(file_data)) => {
                                    let body = format!(
                                        "[Attachment '{}' loaded; content follows in the next message]",
                                        document_id
                                    );
                                    let synth = LlmMessage::user_with_files(
                                        format!("[Attachment requested by the model: {}]", document_id),
                                        vec![file_data],
                                    )?;
                                    (body, Some(synth))
                                }
                                Ok(None) => (
                                    format!(
                                        "{{\"error\":\"unknown_document_id\",\"document_id\":\"{}\"}}",
                                        document_id
                                    ),
                                    None,
                                ),
                                Err(e) => (
                                    format!(
                                        "{{\"error\":\"attachment_expired_unrecoverable\",\"document_id\":\"{}\",\"reason\":\"{}\"}}",
                                        document_id,
                                        e.replace('"', "'")
                                    ),
                                    None,
                                ),
                            };
                            let loaded = synthetic_user.is_some();
                            let tool_message =
                                LlmMessage::tool(result.tool_call_id.clone(), ack_text)?;
                            messages.push(tool_message.clone());
                            self.conversation_repository
                                .add_message(session_id, tool_message)
                                .await?;
                            if let Some(user_msg) = synthetic_user {
                                // Plan B (D7): the synthetic user_with_files
                                // message stays in the in-memory `messages`
                                // vec so the model has the doc content for
                                // the rest of this turn's ReAct iterations.
                                // But we persist a MARKER to llm_node_history
                                // — not the doc content — so future turns
                                // don't keep paying input-token cost for it.
                                // The model can call load_attachment again
                                // to re-read.
                                // See docs/developer_guide/31_load_attachment.md.
                                messages.push(user_msg);

                                // NOTE: marker_text is deliberately a prose
                                // sentence, NOT a structured JSON/tag block.
                                // Do not write code that parses it: future UI
                                // layers should derive load_attachment events
                                // from tool_call history instead. If the
                                // wording changes, persisted history from
                                // before the change keeps the old string —
                                // that's intentional and not a bug.
                                let marker_text = format!(
                                    "[load_attachment(\"{}\") was invoked. Document \
                                     content was available for this turn only. Call \
                                     load_attachment again if you need to re-read it.]",
                                    document_id
                                );
                                let marker_msg = LlmMessage::user(marker_text)?;
                                self.conversation_repository
                                    .add_message(session_id, marker_msg)
                                    .await?;
                            }

                            // Observability: emit a tool-output-available SSE
                            // event so the frontend renders load_attachment like
                            // any other tool (the input events already fire via
                            // LlmToolCallStart). The payload carries only
                            // metadata (document_id + status) — NOT the document
                            // content, which stays ephemeral in the LLM context
                            // (Plan B / D7). Without this the UI saw an input
                            // event with no matching output event.
                            if let Some(callback) = &on_token {
                                let sse_payload = serde_json::json!({
                                    "document_id": document_id,
                                    "status": if loaded { "loaded" } else { "error" },
                                })
                                .to_string();
                                (callback)(LlmStreamPart::LlmToolCallFinish(ToolResult {
                                    tool_call_id: result.tool_call_id.clone(),
                                    output: sse_payload,
                                    success: loaded,
                                    error: None,
                                }));
                            }
                            continue;
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

    /// One LLM round-trip (stream or call) for `request`. Emits the
    /// `LlmMessageStart`/`LlmMessageFinish` bracket and forwards every stream
    /// part to `on_token` when present. Returns the assembled response and its
    /// completion usage.
    async fn invoke_llm(
        &self,
        request: LlmRequest,
        on_token: &Option<Box<dyn Fn(LlmStreamPart) + Send + Sync>>,
        config: &LlmConfig,
    ) -> Result<(LlmResponse, Option<LlmUsage>), LlmError> {
        if let Some(callback) = on_token {
            (callback)(LlmStreamPart::LlmMessageStart);
        }

        let mut completion_usage = None;
        let response = if let Some(callback) = on_token {
            let stream = self.llm_repository.stream(request).await?;
            use futures::StreamExt;
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
                        (callback)(chunk.part().clone());
                        match chunk.part() {
                            LlmStreamPart::Content(c) => full_content.push_str(c),
                            LlmStreamPart::ThinkingContent(c) => full_thinking.push_str(c),
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
                            LlmStreamPart::Usage(u) => completion_usage = Some(u.clone()),
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

        if let Some(callback) = on_token {
            (callback)(LlmStreamPart::LlmMessageFinish(completion_usage.clone()));
        }

        Ok((response, completion_usage))
    }
}

/// Fold one response's usage into the running cumulative usage.
fn accumulate_usage(cumulative: &mut LlmUsage, response: &LlmResponse) {
    if let Some(usage) = response.usage() {
        cumulative.prompt_tokens += usage.prompt_tokens;
        cumulative.completion_tokens += usage.completion_tokens;
        cumulative.total_tokens += usage.total_tokens;
        if let Some(t) = usage.thinking_tokens {
            *cumulative.thinking_tokens.get_or_insert(0) += t;
        }
        if let Some(cr) = usage.cache_read_tokens {
            *cumulative.cache_read_tokens.get_or_insert(0) += cr;
        }
        if let Some(cw) = usage.cache_write_tokens {
            *cumulative.cache_write_tokens.get_or_insert(0) += cw;
        }
    }
}

/// Canonical `(name, arguments)` signature used to detect repeated tool calls.
/// Object keys are sorted recursively so `{"a":1,"b":2}` and `{"b":2,"a":1}`
/// collapse to one key. Invalid-JSON arguments fall back to the raw string.
/// The `\u{0}` separator cannot appear in a JSON token, so name and args never
/// collide.
#[allow(dead_code)] // removed in Task 4 when the loop guard uses it
fn tool_call_signature(name: &str, arguments: &str) -> String {
    let canon = serde_json::from_str::<serde_json::Value>(arguments)
        .map(|v| canonical_json(&v))
        .unwrap_or_else(|_| arguments.to_string());
    format!("{name}\u{0}{canon}")
}

/// Deterministic, key-sorted serialization of a JSON value (for signatures only).
#[allow(dead_code)] // removed in Task 4 when the loop guard uses it
fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    let key = serde_json::to_string(k).unwrap_or_default();
                    format!("{}:{}", key, canonical_json(&map[k]))
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(arr) => {
            let inner: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}

/// Compact `load_skill` tool results that are older than `keep_recent_msgs`
/// into a short marker. Returns a new Vec — never mutates the input.
///
/// Why: skill bodies (catalog + reference content) are static — the model
/// reads them once at load time, then they live in `messages` forever and get
/// re-sent verbatim in every subsequent ReAct iteration's prompt. For a smoke
/// with 3-5 skill loads and 10 iterations, this re-sends 13-15K tokens of
/// redundant content. Marker replacement saves ~95% of that cost; if the
/// model truly needs to re-read a skill, the protocol prelude (auto-injected
/// when `crdt_documents` is configured) tells it to just call `load_skill`
/// again.
///
/// Migration shim (2026-06-11): strip a leading `## Temporal & Geographic
/// Context` block from a system message loaded from history.
///
/// Pre-fix conversations baked the temporal block into the FRONT of the
/// persisted system message (joined with the `\n\n---\n` section separator).
/// The cache-safe fix injects a fresh temporal block per turn as a volatile
/// suffix, so a loaded pre-fix system would carry a stale duplicate. This
/// removes everything from the `## Temporal & Geographic Context` header up to
/// and including the first `\n\n---\n` separator. If the block has no trailing
/// separator (it was the only section), the whole content is dropped (returns
/// empty). System messages that do not start with the header are returned
/// unchanged.
fn strip_leading_temporal_block(content: &str) -> String {
    const HEADER: &str = "## Temporal & Geographic Context";
    const SEP: &str = "\n\n---\n";
    if !content.starts_with(HEADER) {
        return content.to_string();
    }
    match content.find(SEP) {
        Some(idx) => content[idx + SEP.len()..].to_string(),
        None => String::new(),
    }
}

/// Detection: for each Tool message, find the matching Assistant message that
/// emitted the tool call by `tool_call_id` and check whether the function name
/// was `load_skill`. Only matches by exact function name — `crdt_doc_*` tool
/// results stay intact (they're either tiny or stateful and worth re-sending).
///
/// Provider-agnostic: each provider adapter serializes `LlmMessage` to its
/// own request format; the compact marker is just a Tool message with shorter
/// content, so no adapter changes needed.
fn compact_old_load_skill_in_history(
    messages: &[LlmMessage],
    keep_recent_msgs: usize,
) -> Vec<LlmMessage> {
    let mut out: Vec<LlmMessage> = messages.to_vec();
    if out.len() <= keep_recent_msgs {
        return out;
    }

    // Build: tool_call_id → load_skill arguments (only for load_skill calls).
    let mut load_skill_calls: HashMap<String, String> = HashMap::new();
    for msg in out.iter() {
        let Some(tcs) = msg.tool_calls() else {
            continue;
        };
        for tc in tcs {
            if tc.function.name == "load_skill" {
                load_skill_calls.insert(tc.id.clone(), tc.function.arguments.clone());
            }
        }
    }

    if load_skill_calls.is_empty() {
        return out;
    }

    let boundary = out.len().saturating_sub(keep_recent_msgs);

    // Two-phase scan: collect indices that need rewriting first (immutable
    // borrow), then mutate (mutable borrow). Avoids the needless-range-loop
    // lint without losing clarity.
    let mut to_compact: Vec<(usize, String, String)> = Vec::new();
    for (i, msg) in out.iter().enumerate().take(boundary) {
        if msg.role() != &MessageRole::Tool {
            continue;
        }
        let Some(tcid) = msg.tool_call_id().map(|s| s.to_string()) else {
            continue;
        };
        let Some(args) = load_skill_calls.get(&tcid) else {
            continue;
        };
        // Skip already-marked messages (idempotent).
        if msg.content().starts_with("[skill ") && msg.content().ends_with(']') {
            continue;
        }
        to_compact.push((i, tcid, args.clone()));
    }

    for (i, tcid, args) in to_compact {
        let original_size = out[i].content().len();

        // Parse args to make the marker descriptive (best-effort).
        let marker = match serde_json::from_str::<serde_json::Value>(&args) {
            Ok(v) => {
                let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                match v.get("reference").and_then(|x| x.as_str()) {
                    Some(r) => format!(
                        "[skill '{name}' (ref={r}) loaded earlier ({original_size} chars). \
                         Call load_skill again to re-read.]"
                    ),
                    None => format!(
                        "[skill '{name}' loaded earlier ({original_size} chars). \
                         Call load_skill again to re-read.]"
                    ),
                }
            }
            Err(_) => format!("[skill loaded earlier ({original_size} chars)]"),
        };

        if let Ok(new_msg) = LlmMessage::tool(tcid, marker) {
            out[i] = new_msg;
        }
    }

    out
}

/// F-T15 — Rolling-summary compaction.
///
/// When `messages.len() > keep_first + keep_recent + 1`, replace the middle
/// slice `messages[keep_first .. messages.len()-keep_recent]` with a single
/// System message containing one summary line per source message. Each line
/// is tagged with the source message's index (its persisted turn number)
/// so that the agent can call `recall_history(turn=N)` to retrieve the
/// original verbatim from the conversation_repository.
///
/// The returned Vec is a NEW allocation — the input `messages` slice is not
/// mutated. The persisted history in `conversation_repository` is not touched
/// either; this only shapes the LlmRequest sent to the provider.
///
/// Cap policy: if more than `max_lines` source messages would be summarized,
/// drop the OLDEST lines (keep the newest `max_lines` in the summary). The
/// dropped data still lives in `conversation_repository` and can be recalled
/// individually by turn number — only the eager summary view is bounded.
///
/// Composition: this runs AFTER `compact_old_load_skill_in_history` so any
/// load_skill markers in the middle slice are already small (the marker text
/// goes into the summary line directly, no double work).
fn compact_history_to_summary(
    messages: &[LlmMessage],
    keep_first: usize,
    keep_recent: usize,
    max_lines: usize,
    line_max_chars: usize,
) -> Vec<LlmMessage> {
    let total = messages.len();
    let need = keep_first.saturating_add(keep_recent).saturating_add(1);
    if total < need {
        return messages.to_vec();
    }
    let initial_middle_end = total.saturating_sub(keep_recent);
    if initial_middle_end <= keep_first {
        return messages.to_vec();
    }

    // 2026-06-07 fix: prevent splitting in the middle of an
    // `assistant(tool_calls) + tool` response sequence. The original logic
    // could push `assistant(tool_calls)` into `middle` (summarized away) and
    // leave the matching `tool` responses in `kept_recent` — producing an
    // orphaned tool sequence that OpenAI rejects with "messages with role
    // 'tool' must be a response to a preceding message with 'tool_calls'".
    //
    // Walk `middle_end` backwards while it points to a Tool message; this
    // pulls all the contiguous tool responses AND their preceding
    // assistant(tool_calls) message into `kept_recent`, preserving the pair
    // invariant required by both Chat Completions and Responses APIs.
    // See colmena BACKLOG entries "Lazy tool loading — OpenAI message-order
    // regression al cerrar el turn" and "OpenAI Responses API — input_text
    // invalid en synthetic-tool path" for the diagnosis trail.
    let mut middle_end = initial_middle_end;
    while middle_end > keep_first && matches!(messages[middle_end].role(), MessageRole::Tool) {
        middle_end -= 1;
    }
    if middle_end <= keep_first {
        return messages.to_vec();
    }
    let middle = &messages[keep_first..middle_end];
    if middle.is_empty() {
        return messages.to_vec();
    }

    // Build: tool_call_id → function.name from any Assistant messages we have
    // seen so far (including ones inside `middle`). The map is best-effort —
    // if a Tool message references a tool_call_id that's not in any visible
    // Assistant message, the tool name renders as "?".
    let mut tool_call_names: HashMap<String, String> = HashMap::new();
    for msg in messages.iter() {
        if let Some(tcs) = msg.tool_calls() {
            for tc in tcs {
                tool_call_names.insert(tc.id.clone(), tc.function.name.clone());
            }
        }
    }

    // Build one line per message, tagged with its source index.
    let mut lines: Vec<String> = Vec::with_capacity(middle.len());
    for (offset, msg) in middle.iter().enumerate() {
        let idx = keep_first + offset; // persisted turn number
        lines.push(summary_line_for_message(
            idx,
            msg,
            &tool_call_names,
            line_max_chars,
        ));
    }

    // Apply the max_lines cap by dropping the OLDEST entries first.
    let lines_dropped = lines.len().saturating_sub(max_lines);
    let kept_lines: Vec<String> = lines.into_iter().skip(lines_dropped).collect();

    // Render the summary text.
    let mut summary = String::new();
    summary.push_str("## Conversation summary (older turns compacted)\n");
    summary.push_str(
        "Each line below is one message you sent or received earlier in this conversation. \
         Numbers in [Tn] are the persisted turn index. To re-read the FULL content of any \
         turn, call recall_history(turn=N). Use it sparingly — each recall puts the \
         original message back into your context.\n\n",
    );
    if lines_dropped > 0 {
        summary.push_str(&format!(
            "(turns {}..{} omitted from summary — still recallable individually)\n",
            keep_first,
            keep_first + lines_dropped - 1,
        ));
    }
    for line in &kept_lines {
        summary.push_str(line);
        summary.push('\n');
    }

    // Assemble: kept_first + [System summary] + kept_recent.
    //
    // Edge case: if the last kept_first message is already a System, inserting
    // a NEW System right after it produces consecutive Systems — providers like
    // Gemini reject this with "Consecutive messages with the same role are not
    // supported". Merge the summary into that existing System content instead.
    let mut out: Vec<LlmMessage> = Vec::with_capacity(keep_first + 1 + keep_recent);
    let prev_is_system =
        keep_first > 0 && matches!(messages[keep_first - 1].role(), MessageRole::System);
    if prev_is_system {
        out.extend(messages[..keep_first - 1].iter().cloned());
        let combined = format!(
            "{}\n\n---\n\n{}",
            messages[keep_first - 1].content(),
            summary
        );
        if let Ok(merged) = LlmMessage::system(combined) {
            out.push(merged);
        } else {
            out.push(messages[keep_first - 1].clone());
            if let Ok(summary_msg) = LlmMessage::system(summary) {
                out.push(summary_msg);
            }
        }
    } else {
        out.extend(messages[..keep_first].iter().cloned());
        if let Ok(summary_msg) = LlmMessage::system(summary) {
            out.push(summary_msg);
        }
    }
    out.extend(messages[middle_end..].iter().cloned());

    // Safety: if the FIRST message of the kept-recent window is a Tool
    // without its matching Assistant in the kept window OR in the kept-first
    // slice, some providers reject the request. Walk backwards from the
    // window boundary to include the parent Assistant if needed.
    //
    // We don't do this expansion here for v1 — the typical case (keep_recent=5
    // covering ~1-2 turns of Assistant+Tool pairs) is well-formed. If smoke
    // hits a provider rejection, this is the place to add the guard.

    out
}

/// Render one source message as one line for the rolling summary.
/// Output shape:
///   `[T<idx>] USER: <content>`
///   `[T<idx>] AGENT said: <content>; called <tool>(<args>); ...`
///   `[T<idx>] TOOL(<name>) → <content>`
fn summary_line_for_message(
    idx: usize,
    msg: &LlmMessage,
    tool_call_names: &HashMap<String, String>,
    max_chars: usize,
) -> String {
    let truncate_inline = |s: &str, cap: usize| -> String {
        if s.chars().count() <= cap {
            s.to_string()
        } else {
            let mut end = cap;
            // walk back to a char boundary
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…", &s[..end])
        }
    };

    match msg.role() {
        MessageRole::User => format!(
            "[T{idx}] USER: {}",
            truncate_inline(msg.content(), max_chars)
        ),
        MessageRole::System => format!(
            "[T{idx}] SYSTEM: {}",
            truncate_inline(msg.content(), max_chars)
        ),
        MessageRole::Assistant => {
            let mut parts: Vec<String> = Vec::new();
            let text = msg.content().trim();
            if !text.is_empty() {
                parts.push(format!("said: {}", truncate_inline(text, max_chars / 2)));
            }
            if let Some(tcs) = msg.tool_calls() {
                for tc in tcs {
                    parts.push(format!(
                        "called {}({})",
                        tc.function.name,
                        truncate_inline(&tc.function.arguments, max_chars / 2),
                    ));
                }
            }
            if parts.is_empty() {
                format!("[T{idx}] AGENT (empty)")
            } else {
                format!("[T{idx}] AGENT {}", parts.join("; "))
            }
        }
        MessageRole::Tool => {
            let tcid = msg.tool_call_id().unwrap_or("?");
            let name = tool_call_names
                .get(tcid)
                .cloned()
                .unwrap_or_else(|| "?".to_string());
            format!(
                "[T{idx}] TOOL({name}) → {}",
                truncate_inline(msg.content(), max_chars),
            )
        }
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

    // ── Cache-safe temporal strip-on-load (2026-06-11) ──────────────────────

    #[test]
    fn strip_temporal_removes_leading_block_keeps_rest() {
        let sys = "## Temporal & Geographic Context\n\
                   Current date and time: 2026-06-11T10:00:00-05:00 (...)\n\
                   Timezone: America/Bogota (UTC-5)\n\
                   Location: Bogotá, Colombia\n\
                   Locale: es-CO\n\n---\n\
                   ## Tools\nAvailable: add.";
        let out = strip_leading_temporal_block(sys);
        assert_eq!(out, "## Tools\nAvailable: add.");
    }

    #[test]
    fn strip_temporal_drops_block_when_only_section() {
        let sys = "## Temporal & Geographic Context\n\
                   Current date and time: 2026-06-11T10:00:00-05:00\n\
                   Locale: es-CO";
        let out = strip_leading_temporal_block(sys);
        assert!(out.is_empty());
    }

    #[test]
    fn strip_temporal_leaves_non_temporal_system_untouched() {
        let sys = "## Tools\nAvailable: add.\n\n---\nmore stable content";
        let out = strip_leading_temporal_block(sys);
        assert_eq!(out, sys);
    }

    // ── Per-signature loop guard: canonical tool-call signature ─────────────

    #[test]
    fn tool_call_signature_is_key_order_independent() {
        let a = tool_call_signature("read", r#"{"a":1,"b":2}"#);
        let b = tool_call_signature("read", r#"{"b":2,"a":1}"#);
        assert_eq!(a, b, "object key order must not change the signature");
    }

    #[test]
    fn tool_call_signature_is_name_and_args_sensitive() {
        assert_ne!(
            tool_call_signature("read", r#"{"a":1}"#),
            tool_call_signature("write", r#"{"a":1}"#),
            "different tool names must differ"
        );
        assert_ne!(
            tool_call_signature("read", r#"{"range":"A1"}"#),
            tool_call_signature("read", r#"{"range":"B2"}"#),
            "different args must differ"
        );
    }

    #[test]
    fn tool_call_signature_handles_nested_and_invalid_json() {
        // nested object key order also normalized
        let a = tool_call_signature("t", r#"{"x":{"p":1,"q":2}}"#);
        let b = tool_call_signature("t", r#"{"x":{"q":2,"p":1}}"#);
        assert_eq!(a, b);
        // invalid JSON falls back to the raw string (still deterministic)
        let c = tool_call_signature("t", "not json");
        let d = tool_call_signature("t", "not json");
        assert_eq!(c, d);
    }

    // ── F-T14 step A2: skill-out-of-history compaction tests ────────────────

    fn fc(name: &str, args: &str) -> FunctionCall {
        FunctionCall {
            name: name.to_string(),
            arguments: args.to_string(),
        }
    }

    fn tool_call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall::new(id.to_string(), fc(name, args))
    }

    #[test]
    fn compact_noop_when_history_shorter_than_keep_recent() {
        let msgs = vec![
            LlmMessage::user("hi".to_string()).unwrap(),
            LlmMessage::assistant("ok".to_string()).unwrap(),
        ];
        let out = compact_old_load_skill_in_history(&msgs, 10);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].content(), "hi");
        assert_eq!(out[1].content(), "ok");
    }

    #[test]
    fn compact_noop_when_no_load_skill_in_history() {
        // 12 msgs, none from load_skill — compaction must not change anything.
        let mut msgs = Vec::new();
        msgs.push(LlmMessage::user("u".to_string()).unwrap());
        for i in 0..11 {
            msgs.push(LlmMessage::tool(format!("t{i}"), format!("payload {i}")).unwrap());
        }
        let out = compact_old_load_skill_in_history(&msgs, 3);
        assert_eq!(out.len(), msgs.len());
        for (a, b) in out.iter().zip(msgs.iter()) {
            assert_eq!(a.content(), b.content());
        }
    }

    #[test]
    #[allow(clippy::vec_init_then_push)] // tests build heterogeneous msg seqs
    fn compact_replaces_old_load_skill_results_with_markers() {
        // Build: 12 msgs total.
        // msg[0] user, msg[1] assistant load_skill(name=foo), msg[2] tool result (foo body),
        // msg[3] assistant load_skill(name=bar,reference=ref1), msg[4] tool result (bar body),
        // msg[5..11] more stuff
        // keep_recent = 4 → boundary = 8. msgs 0..7 are compactable, 8..11 stay verbatim.
        let mut msgs = Vec::new();
        msgs.push(LlmMessage::user("u".to_string()).unwrap());
        msgs.push(
            LlmMessage::assistant_with_tool_calls(
                String::new(),
                vec![tool_call("call_a", "load_skill", "{\"name\":\"foo\"}")],
            )
            .unwrap(),
        );
        msgs.push(
            LlmMessage::tool("call_a".to_string(), "FULL FOO SKILL BODY".repeat(20)).unwrap(),
        );
        msgs.push(
            LlmMessage::assistant_with_tool_calls(
                String::new(),
                vec![tool_call(
                    "call_b",
                    "load_skill",
                    "{\"name\":\"bar\",\"reference\":\"ref1\"}",
                )],
            )
            .unwrap(),
        );
        msgs.push(LlmMessage::tool("call_b".to_string(), "FULL BAR REF1 BODY".repeat(20)).unwrap());
        for i in 0..7 {
            msgs.push(LlmMessage::assistant(format!("filler {i}")).unwrap());
        }
        assert_eq!(msgs.len(), 12);

        let out = compact_old_load_skill_in_history(&msgs, 4);
        assert_eq!(out.len(), 12);

        // msg[2] (load_skill foo result) is at index 2, boundary = 12 - 4 = 8 → compacted.
        assert!(
            out[2]
                .content()
                .starts_with("[skill 'foo' loaded earlier ("),
            "expected marker; got: {}",
            out[2].content()
        );
        // msg[4] (load_skill bar ref1 result) at index 4 → compacted with reference.
        assert!(
            out[4].content().contains("(ref=ref1)"),
            "expected ref marker; got: {}",
            out[4].content()
        );
        // Marker much shorter than original
        let original_size = msgs[2].content().len();
        assert!(
            out[2].content().len() < original_size / 3,
            "marker should be a small fraction of original"
        );
    }

    #[test]
    #[allow(clippy::vec_init_then_push)] // tests build heterogeneous msg seqs
    fn compact_preserves_recent_load_skill_results() {
        // Build: 10 msgs, the LAST msg is a load_skill tool result. keep_recent=4
        // → boundary = 6. msgs[6..] should remain verbatim.
        let mut msgs = Vec::new();
        for i in 0..7 {
            msgs.push(LlmMessage::user(format!("u{i}")).unwrap());
        }
        msgs.push(
            LlmMessage::assistant_with_tool_calls(
                String::new(),
                vec![tool_call("recent", "load_skill", "{\"name\":\"recent\"}")],
            )
            .unwrap(),
        );
        msgs.push(LlmMessage::tool("recent".to_string(), "FRESH SKILL BODY".to_string()).unwrap());
        msgs.push(LlmMessage::assistant("ack".to_string()).unwrap());
        assert_eq!(msgs.len(), 10);

        let out = compact_old_load_skill_in_history(&msgs, 4);
        // msg[8] is the tool result for "recent" load — index 8 >= boundary 6 → NOT compacted.
        assert_eq!(out[8].content(), "FRESH SKILL BODY");
    }

    #[test]
    #[allow(clippy::vec_init_then_push)]
    fn compact_skips_non_load_skill_tools_even_if_old() {
        let mut msgs = Vec::new();
        msgs.push(
            LlmMessage::assistant_with_tool_calls(
                String::new(),
                vec![tool_call("call_x", "crdt_doc_run_python", "{}")],
            )
            .unwrap(),
        );
        msgs.push(
            LlmMessage::tool(
                "call_x".to_string(),
                "{\"output\": \"pandas result\"}".to_string(),
            )
            .unwrap(),
        );
        for i in 0..10 {
            msgs.push(LlmMessage::assistant(format!("filler {i}")).unwrap());
        }
        let out = compact_old_load_skill_in_history(&msgs, 4);
        // msg[1] (crdt_doc_run_python result) stays untouched.
        assert_eq!(out[1].content(), "{\"output\": \"pandas result\"}");
    }

    #[test]
    #[allow(clippy::vec_init_then_push)]
    fn compact_is_idempotent_does_not_remark_already_marked() {
        let mut msgs = Vec::new();
        msgs.push(
            LlmMessage::assistant_with_tool_calls(
                String::new(),
                vec![tool_call("c", "load_skill", "{\"name\":\"x\"}")],
            )
            .unwrap(),
        );
        msgs.push(
            LlmMessage::tool(
                "c".to_string(),
                "[skill 'x' loaded earlier (1234 chars). Call load_skill again to re-read.]"
                    .to_string(),
            )
            .unwrap(),
        );
        for i in 0..10 {
            msgs.push(LlmMessage::assistant(format!("filler {i}")).unwrap());
        }
        let out = compact_old_load_skill_in_history(&msgs, 4);
        // Already a marker → not re-marked (would show "(NaN chars)" or grow).
        assert_eq!(out[1].content(), msgs[1].content());
    }

    // ── F-T15: compact_history_to_summary tests ─────────────────────────────

    #[test]
    fn summary_noop_when_history_below_threshold() {
        let msgs = vec![
            LlmMessage::user("u".to_string()).unwrap(),
            LlmMessage::system("s".to_string()).unwrap(),
            LlmMessage::assistant("a".to_string()).unwrap(),
            LlmMessage::user("b".to_string()).unwrap(),
        ];
        // keep_first=2, keep_recent=5 → need=8; msgs.len()=4 < 8 → no compaction
        let out = compact_history_to_summary(&msgs, 2, 5, 100, 180);
        assert_eq!(out.len(), 4);
        for (a, b) in out.iter().zip(msgs.iter()) {
            assert_eq!(a.content(), b.content());
        }
    }

    #[test]
    #[allow(clippy::vec_init_then_push)]
    fn summary_replaces_middle_with_one_system_message() {
        // Build: 12 msgs total — first 2 (User, System), 5 middle that will be
        // summarized, then 5 recent that stay verbatim.
        let mut msgs = Vec::new();
        msgs.push(LlmMessage::user("original prompt".to_string()).unwrap());
        msgs.push(LlmMessage::system("prelude".to_string()).unwrap());
        // 5 middle msgs (indices 2-6)
        msgs.push(
            LlmMessage::assistant_with_tool_calls(
                String::new(),
                vec![tool_call("c1", "crdt_doc_list_sheets", "{}")],
            )
            .unwrap(),
        );
        msgs.push(LlmMessage::tool("c1".to_string(), "{\"sheets\":[\"a\"]}".to_string()).unwrap());
        msgs.push(LlmMessage::assistant("thinking text".to_string()).unwrap());
        msgs.push(
            LlmMessage::assistant_with_tool_calls(
                String::new(),
                vec![tool_call("c2", "crdt_doc_read", "{\"sheet_id\":\"a\"}")],
            )
            .unwrap(),
        );
        msgs.push(LlmMessage::tool("c2".to_string(), "{\"v\":42}".to_string()).unwrap());
        // 5 recent msgs (indices 7-11)
        for i in 0..5 {
            msgs.push(LlmMessage::assistant(format!("recent {i}")).unwrap());
        }
        assert_eq!(msgs.len(), 12);

        let out = compact_history_to_summary(&msgs, 2, 5, 100, 180);
        // Result: first 1 (User) + 1 merged System (prelude + summary) + 5 recent = 7 msgs.
        // The summary gets merged INTO the existing System (msg[1]) to avoid
        // consecutive-System rejections from providers like Gemini.
        assert_eq!(out.len(), 7);
        assert_eq!(out[0].content(), "original prompt");
        // out[1] is the merged System: prelude content + summary
        assert_eq!(out[1].role(), &MessageRole::System);
        let merged = out[1].content();
        assert!(merged.starts_with("prelude"));
        assert!(merged.contains("## Conversation summary"));
        // Contains turn tags for the summarized middle (indices 2..7)
        assert!(merged.contains("[T2]"));
        assert!(merged.contains("[T3]"));
        assert!(merged.contains("[T4]"));
        assert!(merged.contains("[T5]"));
        assert!(merged.contains("[T6]"));
        // Tool name resolved correctly
        assert!(merged.contains("crdt_doc_list_sheets"));
        assert!(merged.contains("crdt_doc_read"));
        // Mentions recall_history
        assert!(merged.contains("recall_history"));
        // Recent 5 messages verbatim (start at out[2] since merged System is at out[1])
        for i in 0..5 {
            assert_eq!(out[2 + i].content(), format!("recent {i}"));
        }
    }

    #[test]
    #[allow(clippy::vec_init_then_push)]
    fn summary_caps_at_max_lines_dropping_oldest() {
        // 50 middle msgs, max_lines=10 → only newest 10 lines stay, the rest
        // gets noted as "turns N..M omitted from summary".
        let mut msgs = Vec::new();
        msgs.push(LlmMessage::user("u".to_string()).unwrap());
        msgs.push(LlmMessage::system("s".to_string()).unwrap());
        for i in 0..50 {
            msgs.push(LlmMessage::assistant(format!("middle msg {i}")).unwrap());
        }
        for i in 0..5 {
            msgs.push(LlmMessage::assistant(format!("recent {i}")).unwrap());
        }
        let out = compact_history_to_summary(&msgs, 2, 5, 10, 180);
        // Merged into msg[1] (System): User + merged System + 5 recent = 7
        assert_eq!(out.len(), 1 + 1 + 5);
        let summary = out[1].content();
        // The dropped-range marker is present
        assert!(
            summary.contains("omitted from summary"),
            "expected omitted marker; summary was:\n{summary}"
        );
        // Latest 10 of the middle should be in the summary (indices 42..51)
        assert!(summary.contains("[T42]"));
        assert!(summary.contains("[T51]"));
        // Earliest middle msgs should NOT be in the summary
        assert!(!summary.contains("[T2]"));
        assert!(!summary.contains("[T5]"));
    }

    #[test]
    #[allow(clippy::vec_init_then_push)]
    fn summary_never_orphans_tool_message_after_compaction() {
        // REGRESSION TEST (2026-06-07): the original implementation could leave
        // orphaned `tool` messages at the start of `kept_recent` when the
        // boundary fell inside an {assistant.tool_calls, tool, tool, ...}
        // sequence. OpenAI Chat Completions rejected with "messages with role
        // 'tool' must be a response to a preceding message with 'tool_calls'";
        // OpenAI Responses API rejected with content-type mismatch. Both were
        // caused by the same orphaning behavior in compact_history_to_summary.
        //
        // Construct the exact lazy_tools scenario observed in E2E Phase 1.2:
        // 1 assistant message with 5 parallel tool_calls, followed by 5 tool
        // responses. With keep_first=2, keep_recent=5, the boundary falls
        // BETWEEN the assistant (which would be summarized) and the tools
        // (which would land in kept_recent, orphaned).
        let mut msgs = Vec::new();
        msgs.push(LlmMessage::user("the prompt".to_string()).unwrap());
        msgs.push(LlmMessage::system("system prelude".to_string()).unwrap());
        // 1 assistant with 5 parallel tool_calls (would land at index 2)
        msgs.push(
            LlmMessage::assistant_with_tool_calls(
                String::new(),
                vec![
                    tool_call("c1", "current_time", "{}"),
                    tool_call("c2", "describe_tool", "{\"name\":\"multiply\"}"),
                    tool_call("c3", "describe_tool", "{\"name\":\"add\"}"),
                    tool_call("c4", "add", "{\"a\":25,\"b\":17}"),
                    tool_call("c5", "multiply", "{\"a\":42,\"b\":3}"),
                ],
            )
            .unwrap(),
        );
        // 5 tool responses (indices 3-7)
        for (id, payload) in [
            ("c1", "{\"now\":\"2026-06-07T...\"}"),
            ("c2", "{\"description\":\"...\"}"),
            ("c3", "{\"description\":\"...\"}"),
            ("c4", "{\"output\":42}"),
            ("c5", "{\"output\":126}"),
        ] {
            msgs.push(LlmMessage::tool(id.to_string(), payload.to_string()).unwrap());
        }
        assert_eq!(msgs.len(), 8);

        let out = compact_history_to_summary(&msgs, 2, 5, 100, 180);

        // The fix should pull the assistant message INTO kept_recent (so it
        // precedes its tool responses), preserving the pair invariant.
        // Walk the output and verify: every Tool message has an Assistant
        // with tool_calls before it (possibly with intermediate tools).
        for (i, msg) in out.iter().enumerate() {
            if matches!(msg.role(), MessageRole::Tool) {
                // Find the preceding non-Tool message.
                let mut j = i;
                while j > 0 && matches!(out[j - 1].role(), MessageRole::Tool) {
                    j -= 1;
                }
                assert!(
                    j > 0,
                    "tool message at index {i} has no preceding non-Tool message; \
                     this would trigger OpenAI rejection"
                );
                let preceding = &out[j - 1];
                assert!(
                    matches!(preceding.role(), MessageRole::Assistant)
                        && preceding.tool_calls().is_some(),
                    "tool message at index {i} is preceded by a {:?} (not assistant.tool_calls); \
                     this is the orphaned-tool bug this test protects against",
                    preceding.role()
                );
            }
        }
    }

    #[test]
    fn summary_line_formats_per_role() {
        let tool_names: HashMap<String, String> = HashMap::new();
        let user_msg = LlmMessage::user("hello".to_string()).unwrap();
        let line = summary_line_for_message(0, &user_msg, &tool_names, 180);
        assert!(line.starts_with("[T0] USER: hello"));

        let asst_text = LlmMessage::assistant("thinking aloud".to_string()).unwrap();
        let line = summary_line_for_message(3, &asst_text, &tool_names, 180);
        assert!(line.starts_with("[T3] AGENT said:"));
        assert!(line.contains("thinking aloud"));

        let mut tool_names = HashMap::new();
        tool_names.insert("c1".to_string(), "crdt_doc_run_python".to_string());
        let tool_msg =
            LlmMessage::tool("c1".to_string(), "{\"output\":\"42\"}".to_string()).unwrap();
        let line = summary_line_for_message(7, &tool_msg, &tool_names, 180);
        assert!(line.starts_with("[T7] TOOL(crdt_doc_run_python) → "));
        assert!(line.contains("42"));
    }

    #[test]
    fn summary_line_truncates_long_content_safely() {
        let tool_names = HashMap::new();
        let long = "x".repeat(500);
        let user_msg = LlmMessage::user(long.clone()).unwrap();
        let line = summary_line_for_message(0, &user_msg, &tool_names, 100);
        // Expect ~ "[T0] USER: <100 chars>…"
        assert!(line.contains('…'));
        // Length is roughly bounded — definitely less than the original 500
        assert!(line.chars().count() < 200);
    }

    // ──────────────────────────────────────────────────────────────────────

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
                attachment_resolver: None,
                agent_session_id: None,
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
                attachment_resolver: None,
                agent_session_id: None,
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
                attachment_resolver: None,
                agent_session_id: None,
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
                attachment_resolver: None,
                agent_session_id: None,
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
                attachment_resolver: None,
                agent_session_id: None,
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

    #[tokio::test]
    async fn load_attachment_sentinel_injects_synthetic_user_message_and_continues() {
        use crate::llm::domain::{
            tools::FunctionCall as FC, FileData, FileSource, ProviderFileRef, ProviderKind as PK,
        };
        use std::sync::Mutex;

        let mut mock_llm = MockLlmRepo::new();
        let mut mock_conv = MockConversationRepo::new();

        let call_id = "call_la_1".to_string();

        // Turn 1: LLM emits a load_attachment tool call.
        {
            let llm_call_id = call_id.clone();
            mock_llm.expect_call().times(1).returning(move |_req| {
                let tc = ToolCall {
                    id: llm_call_id.clone(),
                    call_type: "function".to_string(),
                    function: FC::new(
                        "load_attachment".to_string(),
                        r#"{"document_id":"doc-1"}"#.to_string(),
                    ),
                    response: None,
                };
                Ok(LlmResponse::new(
                    LlmRequestId::from_string("req-la-1".to_string()).unwrap(),
                    "".to_string(),
                    LlmProvider::new(PK::OpenAi, "key".to_string(), Some("gpt-4".to_string()))
                        .unwrap(),
                )
                .unwrap()
                .with_tool_calls(vec![tc]))
            });
        }

        // Turn 2: LLM emits a final text response.
        mock_llm.expect_call().times(1).returning(|_req| {
            Ok(LlmResponse::new(
                LlmRequestId::from_string("req-la-2".to_string()).unwrap(),
                "Final answer".to_string(),
                LlmProvider::new(PK::OpenAi, "key".to_string(), Some("gpt-4".to_string())).unwrap(),
            )
            .unwrap())
        });

        // In-memory conversation repo.
        mock_conv.expect_get_by_id().returning(|key| {
            Ok(Conversation {
                key: key.clone(),
                messages: vec![],
            })
        });
        let persisted: Arc<Mutex<Vec<LlmMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let persisted_for_mock = persisted.clone();
        mock_conv.expect_add_message().returning(move |_k, m| {
            persisted_for_mock.lock().unwrap().push(m);
            Ok(())
        });

        // Tool executor: returns the LOAD_ATTACHMENT sentinel.
        struct SentinelExec;
        #[async_trait::async_trait]
        impl ToolExecutor for SentinelExec {
            async fn execute(&self, tc: &ToolCall) -> Result<ToolResult, LlmError> {
                Ok(ToolResult {
                    tool_call_id: tc.id.clone(),
                    output: r#"{"__colmena_status":"LOAD_ATTACHMENT","document_id":"doc-1"}"#
                        .to_string(),
                    success: true,
                    error: None,
                })
            }
            async fn available_tools(&self) -> Vec<ToolDefinition> {
                vec![]
            }
        }

        // Resolver: returns a fake Uploaded FileData for "doc-1".
        struct FakeResolver;
        #[async_trait::async_trait]
        impl LoadAttachmentResolver for FakeResolver {
            async fn resolve(&self, _sid: &str, doc_id: &str) -> Result<Option<FileData>, String> {
                if doc_id == "doc-1" {
                    Ok(Some(FileData {
                        document_id: Some(doc_id.to_string()),
                        mime_type: "application/pdf".to_string(),
                        filename: "x.pdf".to_string(),
                        size_hint: Some(10),
                        source: FileSource::Uploaded(ProviderFileRef {
                            provider: PK::OpenAi,
                            provider_file_id: "pf-1".to_string(),
                            mime_type: "application/pdf".to_string(),
                            filename: "x.pdf".to_string(),
                            expires_at: None,
                        }),
                        retained_inline_bytes: None,
                    }))
                } else {
                    Ok(None)
                }
            }
        }

        let svc = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
        let session = ConversationKey {
            session_id: SessionId("s1".to_string()),
            agent_session_id: Some(AgentSessionId("agent_1".to_string())),
            node_id: NodeIdPath("llm_call".to_string()),
        };
        let params = AgentRunParams {
            session_id: &session,
            prompt: Some("read the doc".to_string()),
            messages: None,
            config: create_config(),
            tools: vec![],
            tool_executor: &SentinelExec,
            max_iterations: Some(5),
            on_token: None,
            tools_provider: None,
            attachment_resolver: Some(Arc::new(FakeResolver)),
            agent_session_id: Some("agent_1".to_string()),
        };
        let resp = svc.run(params).await.unwrap();
        assert_eq!(resp.content(), "Final answer");

        // Plan B (D7): the synthetic user_with_files message must NOT be
        // persisted to the conversation history — only a short marker text
        // takes its place so future turns don't keep paying input-token cost
        // for the doc content.
        let msgs = persisted.lock().unwrap().clone();
        let has_user_with_files = msgs.iter().any(|m| {
            m.role().as_str() == "user" && m.files().map(|f| !f.is_empty()).unwrap_or(false)
        });
        assert!(
            !has_user_with_files,
            "synthetic user_with_files must NOT be persisted (ephemeral)"
        );

        // Marker user message should be present, referencing the document_id.
        let has_marker = msgs.iter().any(|m| {
            m.role().as_str() == "user"
                && m.content().contains("[load_attachment(")
                && m.content().contains("doc-1")
        });
        assert!(
            has_marker,
            "expected a load_attachment marker user message in persisted history"
        );
    }

    #[tokio::test]
    async fn load_attachment_emits_tool_output_available_for_sse() {
        // Observability: load_attachment must emit LlmToolCallFinish (mapped to
        // the `tool-output-available` SSE event) just like every other tool, so
        // the frontend can render "load_attachment completed". Before the fix,
        // the LOAD_ATTACHMENT sentinel path `continue`d before reaching the
        // LlmToolCallFinish callback, leaving the UI with an input event but no
        // matching output event. The SSE payload carries only metadata
        // (document_id + status), NOT the document content — the content stays
        // ephemeral in the LLM context.
        use crate::llm::domain::{FileData, FileSource, ProviderFileRef, ProviderKind as PK};
        use std::sync::Mutex;

        let mut mock_llm = MockLlmRepo::new();
        let mut mock_conv = MockConversationRepo::new();
        let call_id = "call_la_sse".to_string();

        // on_token=Some forces the streaming path (should_stream = on_token.is_some()),
        // so we mock stream() rather than call().
        // Turn 1: stream yields a load_attachment tool-call chunk.
        {
            let llm_call_id = call_id.clone();
            mock_llm.expect_stream().times(1).returning(move |_req| {
                let provider =
                    LlmProvider::new(PK::OpenAi, "key".to_string(), Some("gpt-4".to_string()))
                        .unwrap();
                let chunk = LlmStreamChunk::new(
                    LlmRequestId::new(),
                    LlmStreamPart::ToolCallChunk(ToolCallChunk {
                        index: 0,
                        id: llm_call_id.clone(),
                        name: "load_attachment".to_string(),
                        args_chunk: r#"{"document_id":"doc-1"}"#.to_string(),
                    }),
                    provider,
                    true,
                );
                let s = futures::stream::iter(vec![Ok(chunk)]);
                Ok(Box::pin(s) as LlmStream)
            });
        }
        // Turn 2: stream yields a final text chunk.
        mock_llm.expect_stream().times(1).returning(|_req| {
            let provider =
                LlmProvider::new(PK::OpenAi, "key".to_string(), Some("gpt-4".to_string())).unwrap();
            let chunk = LlmStreamChunk::new(
                LlmRequestId::new(),
                LlmStreamPart::Content("Done".to_string()),
                provider,
                true,
            );
            let s = futures::stream::iter(vec![Ok(chunk)]);
            Ok(Box::pin(s) as LlmStream)
        });

        mock_conv.expect_get_by_id().returning(|key| {
            Ok(Conversation {
                key: key.clone(),
                messages: vec![],
            })
        });
        mock_conv.expect_add_message().returning(|_k, _m| Ok(()));

        struct SentinelExec;
        #[async_trait::async_trait]
        impl ToolExecutor for SentinelExec {
            async fn execute(&self, tc: &ToolCall) -> Result<ToolResult, LlmError> {
                Ok(ToolResult {
                    tool_call_id: tc.id.clone(),
                    output: r#"{"__colmena_status":"LOAD_ATTACHMENT","document_id":"doc-1"}"#
                        .to_string(),
                    success: true,
                    error: None,
                })
            }
            async fn available_tools(&self) -> Vec<ToolDefinition> {
                vec![]
            }
        }

        struct FakeResolver;
        #[async_trait::async_trait]
        impl LoadAttachmentResolver for FakeResolver {
            async fn resolve(&self, _sid: &str, doc_id: &str) -> Result<Option<FileData>, String> {
                Ok(Some(FileData {
                    document_id: Some(doc_id.to_string()),
                    mime_type: "application/pdf".to_string(),
                    filename: "x.pdf".to_string(),
                    size_hint: Some(10),
                    source: FileSource::Uploaded(ProviderFileRef {
                        provider: PK::OpenAi,
                        provider_file_id: "pf-1".to_string(),
                        mime_type: "application/pdf".to_string(),
                        filename: "x.pdf".to_string(),
                        expires_at: None,
                    }),
                    retained_inline_bytes: None,
                }))
            }
        }

        // Capture every emitted stream part.
        let captured: Arc<Mutex<Vec<LlmStreamPart>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_cb = captured.clone();
        let on_token: Box<dyn Fn(LlmStreamPart) + Send + Sync> =
            Box::new(move |part| captured_cb.lock().unwrap().push(part));

        let svc = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
        let session = ConversationKey {
            session_id: SessionId("s1".to_string()),
            agent_session_id: Some(AgentSessionId("agent_1".to_string())),
            node_id: NodeIdPath("llm_call".to_string()),
        };
        let params = AgentRunParams {
            session_id: &session,
            prompt: Some("read the doc".to_string()),
            messages: None,
            config: create_config(),
            tools: vec![],
            tool_executor: &SentinelExec,
            max_iterations: Some(5),
            on_token: Some(on_token),
            tools_provider: None,
            attachment_resolver: Some(Arc::new(FakeResolver)),
            agent_session_id: Some("agent_1".to_string()),
        };
        svc.run(params).await.unwrap();

        let parts = captured.lock().unwrap();
        // A LlmToolCallFinish must have been emitted for the load_attachment call.
        let finish = parts.iter().find_map(|p| match p {
            LlmStreamPart::LlmToolCallFinish(r) if r.tool_call_id == call_id => Some(r.clone()),
            _ => None,
        });
        let finish = finish.expect(
            "load_attachment must emit LlmToolCallFinish (tool-output-available) for the SSE stream",
        );
        // The SSE payload references the document_id and a status, and must NOT
        // contain the document content (stays ephemeral).
        assert!(
            finish.output.contains("doc-1"),
            "output event should reference the document_id: {}",
            finish.output
        );
        assert!(
            finish.output.contains("loaded"),
            "output event should carry a loaded status: {}",
            finish.output
        );
    }

    #[tokio::test]
    async fn load_attachment_synthetic_message_is_not_persisted_to_history() {
        // Companion test to the above with focus on the dual behavior:
        // - in-memory ReAct stream (observed via the 2nd LLM call's request
        //   messages) sees the synthetic user_with_files;
        // - persisted conversation history sees ONLY the marker, never bytes.
        use crate::llm::domain::{
            tools::FunctionCall as FC, FileData, FileSource, ProviderFileRef, ProviderKind as PK,
        };
        use std::sync::Mutex;

        let mut mock_llm = MockLlmRepo::new();
        let mut mock_conv = MockConversationRepo::new();

        let call_id = "call_la_eph".to_string();

        // Capture the request messages on the 2nd LLM call so we can confirm
        // the in-memory stream contains the synthetic user_with_files.
        let captured_turn2: Arc<Mutex<Vec<LlmMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_for_mock = captured_turn2.clone();

        // Turn 1: LLM emits a load_attachment tool call.
        {
            let llm_call_id = call_id.clone();
            mock_llm.expect_call().times(1).returning(move |_req| {
                let tc = ToolCall {
                    id: llm_call_id.clone(),
                    call_type: "function".to_string(),
                    function: FC::new(
                        "load_attachment".to_string(),
                        r#"{"document_id":"doc-eph"}"#.to_string(),
                    ),
                    response: None,
                };
                Ok(LlmResponse::new(
                    LlmRequestId::from_string("req-eph-1".to_string()).unwrap(),
                    "".to_string(),
                    LlmProvider::new(PK::OpenAi, "key".to_string(), Some("gpt-4".to_string()))
                        .unwrap(),
                )
                .unwrap()
                .with_tool_calls(vec![tc]))
            });
        }

        // Turn 2: LLM emits final text — we capture the request messages.
        mock_llm.expect_call().times(1).returning(move |req| {
            *captured_for_mock.lock().unwrap() = req.messages().to_vec();
            Ok(LlmResponse::new(
                LlmRequestId::from_string("req-eph-2".to_string()).unwrap(),
                "ok".to_string(),
                LlmProvider::new(PK::OpenAi, "key".to_string(), Some("gpt-4".to_string())).unwrap(),
            )
            .unwrap())
        });

        mock_conv.expect_get_by_id().returning(|key| {
            Ok(Conversation {
                key: key.clone(),
                messages: vec![],
            })
        });
        let persisted: Arc<Mutex<Vec<LlmMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let persisted_for_mock = persisted.clone();
        mock_conv.expect_add_message().returning(move |_k, m| {
            persisted_for_mock.lock().unwrap().push(m);
            Ok(())
        });

        struct SentinelExec;
        #[async_trait::async_trait]
        impl ToolExecutor for SentinelExec {
            async fn execute(&self, tc: &ToolCall) -> Result<ToolResult, LlmError> {
                Ok(ToolResult {
                    tool_call_id: tc.id.clone(),
                    output: r#"{"__colmena_status":"LOAD_ATTACHMENT","document_id":"doc-eph"}"#
                        .to_string(),
                    success: true,
                    error: None,
                })
            }
            async fn available_tools(&self) -> Vec<ToolDefinition> {
                vec![]
            }
        }

        struct FakeResolver;
        #[async_trait::async_trait]
        impl LoadAttachmentResolver for FakeResolver {
            async fn resolve(&self, _sid: &str, doc_id: &str) -> Result<Option<FileData>, String> {
                if doc_id == "doc-eph" {
                    Ok(Some(FileData {
                        document_id: Some(doc_id.to_string()),
                        mime_type: "application/pdf".to_string(),
                        filename: "x.pdf".to_string(),
                        size_hint: Some(10),
                        source: FileSource::Uploaded(ProviderFileRef {
                            provider: PK::OpenAi,
                            provider_file_id: "pf-eph".to_string(),
                            mime_type: "application/pdf".to_string(),
                            filename: "x.pdf".to_string(),
                            expires_at: None,
                        }),
                        retained_inline_bytes: None,
                    }))
                } else {
                    Ok(None)
                }
            }
        }

        let svc = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
        let session = ConversationKey {
            session_id: SessionId("s_eph".to_string()),
            agent_session_id: Some(AgentSessionId("agent_eph".to_string())),
            node_id: NodeIdPath("llm_call".to_string()),
        };
        let params = AgentRunParams {
            session_id: &session,
            prompt: Some("read".to_string()),
            messages: None,
            config: create_config(),
            tools: vec![],
            tool_executor: &SentinelExec,
            max_iterations: Some(5),
            on_token: None,
            tools_provider: None,
            attachment_resolver: Some(Arc::new(FakeResolver)),
            agent_session_id: Some("agent_eph".to_string()),
        };
        let resp = svc.run(params).await.unwrap();
        assert_eq!(resp.content(), "ok");

        // Persisted history: NO user_with_files; YES marker.
        let msgs = persisted.lock().unwrap().clone();
        let persisted_has_files = msgs.iter().any(|m| {
            m.role().as_str() == "user" && m.files().map(|f| !f.is_empty()).unwrap_or(false)
        });
        assert!(
            !persisted_has_files,
            "persisted history must NOT contain user_with_files (ephemeral)"
        );
        let persisted_has_marker = msgs.iter().any(|m| {
            m.role().as_str() == "user"
                && m.content().contains("[load_attachment(")
                && m.content().contains("doc-eph")
        });
        assert!(
            persisted_has_marker,
            "persisted history must contain the marker user message"
        );

        // In-memory ReAct stream (turn 2 request): user_with_files present.
        let turn2 = captured_turn2.lock().unwrap().clone();
        let turn2_has_files = turn2.iter().any(|m| {
            m.role().as_str() == "user" && m.files().map(|f| !f.is_empty()).unwrap_or(false)
        });
        assert!(
            turn2_has_files,
            "turn-2 in-memory request must include synthetic user_with_files"
        );
    }
}
