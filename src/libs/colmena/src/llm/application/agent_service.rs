use crate::llm::domain::{
    ConversationKey, ConversationRepository, FileData, LlmConfig, LlmError, LlmMessage,
    LlmRepository, LlmRequest, LlmResponse, LlmStreamPart, LlmUsage, MessageRole, ToolCall,
    ToolDefinition, ToolExecutor, ToolResult,
};
use std::sync::Arc;

/// Number of trailing messages to keep verbatim when compacting old
/// discovery/scaffolding tool results (e.g. `load_skill`, `describe_tool`)
/// in the request. `keep_recent_msgs = 8` covers roughly the last 3 ReAct
/// turns (assistant + 1-2 tool results per turn); anything older with a
/// discovery-tool result is replaced by a short marker.
/// Set to `usize::MAX` to disable.
const DISCOVERY_KEEP_RECENT_MSGS: usize = 8;

/// Nombres de tools de "andamiaje" (discovery/scaffolding del lazy loading + skills).
/// Sus resultados viejos se colapsan a markers (recuperables re-llamando la tool).
const DISCOVERY_TOOL_NAMES: &[&str] = &["load_skill", "describe_tool"];

/// LLM-facing text shown when a tool call with an identical `(name+args)`
/// signature is repeated (loop guard). The prior result is prepended to this.
const REPEAT_NUDGE_TEXT: &str = include_str!("../../../text/prompts/agent_loop/repeat_nudge.md");

/// LLM-facing instruction for the forced final synthesis ("rescue"). Appended
/// as a user message before the terminal, tool-less LLM call.
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
    /// Maximum number of times one `(name+args)` tool-call signature may be
    /// emitted **consecutively** before the loop guard rescues (forced final
    /// synthesis). The public `max_iterations` config key feeds this. Default 3.
    pub max_tool_repeats: Option<usize>,
    /// Hard ceiling on total LLM turns for this run. `None` → resolved from env
    /// `COLMENA_HARD_TURN_CAP` (fallback 50). Single-shot callers pass `Some(1)`.
    pub max_turns: Option<usize>,
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
    message_summarizer: Option<std::sync::Arc<dyn crate::llm::domain::MessageSummarizer>>,
}

impl AgentService {
    pub fn new(
        llm_repository: Arc<dyn LlmRepository>,
        conversation_repository: Arc<dyn ConversationRepository>,
    ) -> Self {
        Self {
            llm_repository,
            conversation_repository,
            message_summarizer: None,
        }
    }

    /// Inject the cheap-model summarizer used to compact old history at load.
    pub fn with_message_summarizer(
        mut self,
        summarizer: std::sync::Arc<dyn crate::llm::domain::MessageSummarizer>,
    ) -> Self {
        self.message_summarizer = Some(summarizer);
        self
    }

    /// Run the agent with tool execution capabilities
    ///
    /// # Arguments
    /// * `params` - Agent execution parameters
    ///
    /// # Returns
    /// Final response from the LLM after tool execution
    pub async fn run<'a>(&self, params: AgentRunParams<'a>) -> Result<LlmResponse, LlmError> {
        let max_tool_repeats = params.max_tool_repeats.unwrap_or(3);
        let max_turns = params.max_turns.unwrap_or_else(default_hard_turn_cap);
        let session_id = params.session_id;
        let prompt = params.prompt;
        let config = params.config;
        let tools = params.tools;
        let tool_executor = params.tool_executor;
        let on_token = params.on_token;
        let tools_provider = params.tools_provider;
        let params_resolver = params.attachment_resolver;
        let params_agent_session_id = params.agent_session_id;

        // 1. Load conversation history (with cached per-message summaries).
        let stored_loaded = self
            .conversation_repository
            .get_with_summaries(session_id)
            .await?;

        // 1b. Migration shim (2026-06-11): conversations persisted BEFORE the
        // cache-safe temporal fix carry the `## Temporal & Geographic Context`
        // block baked into the FRONT of their system message. The fix now
        // injects a fresh temporal block per turn as a volatile suffix, so a
        // loaded pre-fix system would produce a stale duplicate. Strip the
        // leading temporal block from any system message loaded from history.
        // New conversations never hit this (their persisted system has no
        // temporal block).
        //
        // Non-dropping shim: NEVER remove an element so ordinals stay aligned
        // with the DB / `recall_history`. Only rewrite a System message when the
        // strip yields a NON-empty result that differs from the original.
        let mut messages: Vec<LlmMessage> = stored_loaded
            .into_iter()
            .map(|sm| {
                if sm.message.role() == &MessageRole::System {
                    let stripped = strip_leading_temporal_block(sm.message.content());
                    if !stripped.trim().is_empty() && stripped != sm.message.content() {
                        if let Ok(m) = LlmMessage::system(stripped) {
                            return m;
                        }
                    }
                }
                sm.message
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

        // Compute the compacted base ONCE (Hook C). Reload with summaries so it
        // sees the just-persisted prompt and any cached summaries. The same
        // non-dropping temporal strip is applied to `stored_now` before
        // compaction so legacy conversations (pre-2026-06-11) that baked a
        // stale `## Temporal & Geographic Context` block into their persisted
        // System message don't emit it alongside the fresh volatile suffix.
        let prefix_len = messages.len();
        let stored_now = self
            .conversation_repository
            .get_with_summaries(session_id)
            .await?;
        let stored_now = strip_temporal_from_stored(stored_now);
        let base_compacted = crate::llm::application::history_compaction::build_compacted_messages(
            &stored_now,
            session_id,
            self.conversation_repository.as_ref(),
            self.message_summarizer.as_ref(),
            crate::llm::application::history_compaction::RECENT_TOKEN_BUDGET,
        )
        .await;

        let mut cumulative_usage = LlmUsage::default();
        let mut all_tool_calls_executed = Vec::new();
        let mut cumulative_content = String::new();

        // Loop-guard streak: counts CONSECUTIVE repeats of one signature, and
        // resets the moment a different signature appears (the model made
        // progress). `streak_first` is the raw output of this streak's one real
        // execution, echoed back in a nudge.
        let mut streak_sig: Option<String> = None;
        let mut streak_count: u32 = 0;
        let mut streak_first = String::new();

        // 3. ReAct Loop — bounded by the per-run turn ceiling. Productive work
        //    is gated by the loop guard below, not by turns.
        for _iteration in 0..max_turns {
            tracing::info!(
                target: "colmena::agent",
                iteration = _iteration,
                max = max_turns,
                "agent_service: iteration start"
            );

            // A. Call LLM with tools
            let should_stream = on_token.is_some();
            let iteration_tools: Vec<ToolDefinition> = match &tools_provider {
                Some(p) => p(&messages),
                None => tools.clone(),
            };
            // Assemble the request from the once-computed semantic-summary base
            // plus the live tail (NEW turn messages appended this run, at
            // indices >= prefix_len). Then apply the cheap per-iteration
            // discovery-tool compaction (load_skill, describe_tool) on top.
            // Persistence in `conversation_repository` is unchanged so
            // `recall_history(turn=N)` always returns the original verbatim.
            let mut live: Vec<LlmMessage> = base_compacted.clone();
            live.extend_from_slice(&messages[prefix_len..]);
            let request_messages =
                compact_discovery_tools_in_history(&live, DISCOVERY_KEEP_RECENT_MSGS);

            // Per-iteration prompt-size diagnostic. Gated by env var so it has
            // ZERO runtime cost when disabled. Used during F-T13/F-T14 to measure
            // token-optimization wins between commits.
            //   COLMENA_DUMP_PROMPT_SIZES=1  → one-line summary per iteration
            //   COLMENA_DUMP_PROMPT_FULL=1   → full per-message + per-tool breakdown
            if std::env::var("COLMENA_DUMP_PROMPT_SIZES").is_ok() {
                // Dump the EXACT messages about to be sent over the wire (the
                // semantic-summary base + live tail + discovery compaction).
                // Persisted history in conversation_repository keeps the
                // originals for recall.
                let msgs_to_dump = &request_messages;
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

            let mut request = LlmRequest::new(request_messages, config.clone(), should_stream)?;
            if !iteration_tools.is_empty() {
                request = request.with_tools(iteration_tools.clone());
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

                // D. Execute each tool call (with consecutive-streak loop guard)
                let mut rescue = false;
                for tool_call in tool_calls {
                    let sig = tool_call_signature(
                        &tool_call.function.name,
                        &tool_call.function.arguments,
                    );
                    if streak_sig.as_deref() == Some(sig.as_str()) {
                        streak_count += 1;
                    } else {
                        streak_sig = Some(sig.clone());
                        streak_count = 1;
                        streak_first.clear();
                    }
                    let count = streak_count;

                    // Repeated signature in a row (streak >= 2): nudge or rescue.
                    if count > 1 {
                        // `streak_first` is empty when the streak's first call
                        // took an early-return path that never stored a result
                        // (e.g. a repeated `load_attachment`, whose content was
                        // already injected for that turn). The bare redirect is
                        // the right nudge there — there is no prior result to echo.
                        let body = if streak_first.is_empty() {
                            REPEAT_NUDGE_TEXT.to_string()
                        } else {
                            format!("{streak_first}\n\n{REPEAT_NUDGE_TEXT}")
                        };

                        if let Some(callback) = &on_token {
                            (callback)(LlmStreamPart::LlmToolCallStart(tool_call.clone()));
                            (callback)(LlmStreamPart::LlmToolCallFinish(ToolResult {
                                tool_call_id: tool_call.id.clone(),
                                output: body.clone(),
                                success: true,
                                error: None,
                            }));
                        }

                        let mut nudged_call = tool_call.clone();
                        nudged_call.response = Some(serde_json::Value::String(body.clone()));
                        all_tool_calls_executed.push(nudged_call);

                        let tool_message = LlmMessage::tool(tool_call.id.clone(), body)?;
                        messages.push(tool_message.clone());
                        self.conversation_repository
                            .add_message(session_id, tool_message)
                            .await?;

                        if count >= max_tool_repeats as u32 {
                            // Loop guard tripped: still answer the rest of this
                            // turn's tool ids (done by continuing the loop), then
                            // break to synthesis after the for-loop.
                            rescue = true;
                        }
                        continue;
                    }

                    // Streak start (count == 1): real execution (existing path).
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

                    // Store this streak's first result so a later repeat can
                    // echo it in the nudge.
                    streak_first = result.output.clone();

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

                if rescue {
                    break;
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

        // Reached here by the hard turn ceiling OR a loop-guard `break`.
        // Forced final synthesis ("rescue"): one terminal, tool-less LLM call.
        tracing::info!(
            target: "colmena::agent",
            "agent_service: forced final synthesis (rescue)"
        );
        messages.push(LlmMessage::user(RESCUE_SYNTHESIS_TEXT.to_string())?);

        let mut live: Vec<LlmMessage> = base_compacted.clone();
        live.extend_from_slice(&messages[prefix_len..]);
        let request_messages =
            compact_discovery_tools_in_history(&live, DISCOVERY_KEEP_RECENT_MSGS);
        let should_stream = on_token.is_some();
        // No tools on the request → the model cannot call a tool.
        let request = LlmRequest::new(request_messages, config.clone(), should_stream)?;
        let (mut response, _usage) = self.invoke_llm(request, &on_token, &config).await?;

        accumulate_usage(&mut cumulative_usage, &response);
        self.conversation_repository
            .add_message(session_id, response.message().clone())
            .await?;

        let content = response.content();
        if !content.is_empty() {
            if !cumulative_content.is_empty() {
                cumulative_content.push_str("\n\n");
            }
            cumulative_content.push_str(content);
        }

        response = response.with_usage(cumulative_usage);
        response = response.with_content(cumulative_content);
        if !all_tool_calls_executed.is_empty() {
            response = response.with_tool_calls(all_tool_calls_executed);
        }
        Ok(response)
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
                                let entry =
                                    accumulated_tool_calls.entry(tc.index).or_insert_with(|| {
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

/// Default hard ceiling on total LLM turns, read from env `COLMENA_HARD_TURN_CAP`
/// (positive integer), falling back to 50. Pure cost/termination backstop —
/// reaching it triggers the same forced-synthesis rescue as the loop guard.
fn default_hard_turn_cap() -> usize {
    std::env::var("COLMENA_HARD_TURN_CAP")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(50)
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
fn tool_call_signature(name: &str, arguments: &str) -> String {
    let canon = serde_json::from_str::<serde_json::Value>(arguments)
        .map(|v| canonical_json(&v))
        .unwrap_or_else(|_| arguments.to_string());
    format!("{name}\u{0}{canon}")
}

/// Deterministic, key-sorted serialization of a JSON value (for signatures only).
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

/// Non-dropping temporal strip over loaded rows: removes a stale leading
/// `## Temporal & Geographic Context` block from any System message WITHOUT
/// changing the count/order (so ordinals stay aligned with the DB / recall).
///
/// Called on `stored_now` (the reload before compaction) so legacy
/// conversations never duplicate the stale block alongside the fresh
/// `volatile_system_suffix` injected per turn.
fn strip_temporal_from_stored(
    stored: Vec<crate::llm::domain::StoredMessage>,
) -> Vec<crate::llm::domain::StoredMessage> {
    stored
        .into_iter()
        .map(|mut sm| {
            if sm.message.role() == &MessageRole::System {
                let stripped = strip_leading_temporal_block(sm.message.content());
                if !stripped.trim().is_empty() && stripped != sm.message.content() {
                    if let Ok(m) = LlmMessage::system(stripped) {
                        sm.message = m;
                    }
                }
            }
            sm
        })
        .collect()
}

/// Compact old discovery/scaffolding tool results into short markers.
///
/// Discovery tools (`load_skill`, `describe_tool`) emit large results that are
/// only useful immediately after the call. Once older than `keep_recent_msgs`,
/// their Tool messages are replaced by a one-line marker that tells the model
/// it can re-call the tool to re-read. Non-discovery tool results
/// (`crdt_doc_*`, `sql_query`, etc.) stay intact — they're either small or
/// stateful.
///
/// Provider-agnostic: each provider adapter serializes `LlmMessage` to its
/// own request format; the compact marker is just a Tool message with shorter
/// content, so no adapter changes needed.
fn compact_discovery_tools_in_history(
    messages: &[LlmMessage],
    keep_recent_msgs: usize,
) -> Vec<LlmMessage> {
    let mut out: Vec<LlmMessage> = messages.to_vec();
    if out.len() <= keep_recent_msgs {
        return out;
    }

    // tool_call_id → (tool_name, arguments) para las discovery tools.
    let mut discovery_calls: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    for msg in out.iter() {
        if let Some(tcs) = msg.tool_calls() {
            for tc in tcs {
                if DISCOVERY_TOOL_NAMES.contains(&tc.function.name.as_str()) {
                    discovery_calls.insert(
                        tc.id.clone(),
                        (tc.function.name.clone(), tc.function.arguments.clone()),
                    );
                }
            }
        }
    }
    if discovery_calls.is_empty() {
        return out;
    }

    let boundary = out.len().saturating_sub(keep_recent_msgs);
    let mut to_compact: Vec<(usize, String)> = Vec::new();
    for (i, msg) in out.iter().enumerate().take(boundary) {
        if msg.role() != &MessageRole::Tool {
            continue;
        }
        let Some(tcid) = msg.tool_call_id().map(|s| s.to_string()) else {
            continue;
        };
        let Some((name, _args)) = discovery_calls.get(&tcid) else {
            continue;
        };
        // Idempotente: saltar los ya marcados.
        if msg.content().starts_with("[tool '") && msg.content().ends_with(']') {
            continue;
        }
        to_compact.push((i, name.clone()));
    }

    for (i, name) in to_compact {
        let original_size = out[i].content().len();
        let tcid = out[i].tool_call_id().unwrap_or("unknown").to_string();
        let marker = format!(
            "[tool '{name}' result loaded earlier ({original_size} chars). \
             Call {name} again to re-read.]"
        );
        if let Ok(new_msg) = LlmMessage::tool(tcid, marker) {
            out[i] = new_msg;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::*;
    use async_trait::async_trait;

    use mockall::mock;
    use mockall::predicate::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Configures a `MockConversationRepo` backed by shared, mutable state so
    /// that `get_with_summaries` reflects messages appended via `add_message`.
    /// The agent loop reloads history (via `get_with_summaries`) twice — once at
    /// load, once to build the compacted base — so a stateless snapshot mock
    /// would return stale (e.g. pre-prompt) history on the second read. This
    /// helper persists appends so both reads see the up-to-date conversation.
    ///
    /// Returns the shared state handle so callers can inspect/assert what was
    /// persisted. `add_message` is left WITHOUT a `.times()` constraint here;
    /// callers that assert a precise persisted count should inspect the returned
    /// `Vec` instead.
    fn stateful_conv_mock(
        initial: Vec<LlmMessage>,
    ) -> (MockConversationRepo, Arc<Mutex<Vec<LlmMessage>>>) {
        let state = Arc::new(Mutex::new(initial));
        let mut mock = MockConversationRepo::new();

        let s_get = state.clone();
        mock.expect_get_with_summaries().returning(move |_| {
            Ok(s_get
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .map(|message| StoredMessage {
                    message,
                    summary: None,
                })
                .collect())
        });

        let s_by_id = state.clone();
        mock.expect_get_by_id().returning(move |k| {
            Ok(Conversation {
                key: k.clone(),
                messages: s_by_id.lock().unwrap().clone(),
            })
        });

        let s_add = state.clone();
        mock.expect_add_message().returning(move |_, msg| {
            s_add.lock().unwrap().push(msg);
            Ok(())
        });

        (mock, state)
    }

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

    #[test]
    fn strip_temporal_from_stored_strips_legacy_system_keeps_count() {
        use crate::llm::domain::StoredMessage;
        let legacy_sys = "## Temporal & Geographic Context\n\
                          Current date and time: 2026-06-11T10:00:00-05:00\n\
                          Locale: es-CO\n\n---\n## Tools\nAvailable: add.";
        let stored = vec![
            StoredMessage {
                message: LlmMessage::user("hi".into()).unwrap(),
                summary: None,
            },
            StoredMessage {
                message: LlmMessage::system(legacy_sys.to_string()).unwrap(),
                summary: None,
            },
        ];
        let out = strip_temporal_from_stored(stored);
        assert_eq!(out.len(), 2, "count preserved (no drop)");
        assert!(!out[1]
            .message
            .content()
            .contains("Temporal & Geographic Context"));
        assert!(out[1].message.content().contains("## Tools"));
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
        let out = compact_discovery_tools_in_history(&msgs, 10);
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
        let out = compact_discovery_tools_in_history(&msgs, 3);
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

        let out = compact_discovery_tools_in_history(&msgs, 4);
        assert_eq!(out.len(), 12);

        // msg[2] (load_skill foo result) is at index 2, boundary = 12 - 4 = 8 → compacted.
        assert!(
            out[2]
                .content()
                .starts_with("[tool 'load_skill' result loaded earlier ("),
            "expected marker; got: {}",
            out[2].content()
        );
        // msg[4] (load_skill bar ref1 result) at index 4 → also compacted.
        assert!(
            out[4]
                .content()
                .starts_with("[tool 'load_skill' result loaded earlier ("),
            "expected marker; got: {}",
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

        let out = compact_discovery_tools_in_history(&msgs, 4);
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
        let out = compact_discovery_tools_in_history(&msgs, 4);
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
                "[tool 'load_skill' result loaded earlier (1234 chars). Call load_skill again to re-read.]"
                    .to_string(),
            )
            .unwrap(),
        );
        for i in 0..10 {
            msgs.push(LlmMessage::assistant(format!("filler {i}")).unwrap());
        }
        let out = compact_discovery_tools_in_history(&msgs, 4);
        // Already a marker → not re-marked (would show "(NaN chars)" or grow).
        assert_eq!(out[1].content(), msgs[1].content());
    }

    #[test]
    fn discovery_compaction_markers_old_describe_tool() {
        let mut msgs = vec![
            LlmMessage::user("hola".into()).unwrap(),
            LlmMessage::assistant_with_tool_calls(
                String::new(),
                vec![tool_call("c1", "describe_tool", r#"{"name":"sql_query"}"#)],
            )
            .unwrap(),
            LlmMessage::tool(
                "c1".to_string(),
                "# sql_query\n\n<schema gigante...>".repeat(50),
            )
            .unwrap(),
        ];
        for i in 0..9 {
            msgs.push(LlmMessage::user(format!("relleno {i}")).unwrap());
        }
        let out = compact_discovery_tools_in_history(&msgs, DISCOVERY_KEEP_RECENT_MSGS);
        assert!(out[2].content().starts_with("[tool 'describe_tool'"));
        assert!(out[2].content().len() < 120);
    }

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
            async fn get_with_summaries(&self, key: &ConversationKey) -> Result<Vec<StoredMessage>, LlmError>;
            async fn set_summary(&self, key: &ConversationKey, ordinal: usize, summary: &str) -> Result<(), LlmError>;
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
        let mock_tool_exec = MockToolExec::new();

        let key = test_key();
        let prompt = "Hello".to_string();

        // Setup Conversation Repo (stateful so the loop's reload sees the prompt)
        let (mock_conv, conv_state) = stateful_conv_mock(vec![]);

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
                max_tool_repeats: None,
                max_turns: None,
                on_token: None,
                tools_provider: None,
                attachment_resolver: None,
                agent_session_id: None,
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content(), "Hi there!");
        // 1 user message + 1 assistant message persisted.
        assert_eq!(conv_state.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_agent_service_with_tool_call() {
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_tool_exec = MockToolExec::new();

        let key = test_key();
        let prompt = "Add 2+2".to_string();

        // Setup Conversation Repo (stateful so the loop's reload sees the prompt)
        let (mock_conv, _conv_state) = stateful_conv_mock(vec![]);

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
                max_tool_repeats: None,
                max_turns: None,
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
        let mock_tool_exec = MockToolExec::new();

        let key = test_key();

        // Conversation already has a prior user message (simulating resume).
        // Stateful so the loop's reload sees the same history.
        let (mock_conv, conv_state) =
            stateful_conv_mock(vec![
                LlmMessage::user("original question".to_string()).unwrap()
            ]);

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
                max_tool_repeats: None,
                max_turns: None,
                on_token: None,
                tools_provider: None,
                attachment_resolver: None,
                agent_session_id: None,
            })
            .await;

        assert!(result.is_ok(), "run with None prompt must succeed");
        assert_eq!(result.unwrap().content(), "Resumed answer");
        // No new user message persisted: original user + assistant reply only.
        let state = conv_state.lock().unwrap();
        assert_eq!(state.len(), 2, "only the assistant reply must be appended");
        assert_eq!(state[1].role(), &MessageRole::Assistant);
    }

    fn loop_tool_call(args: &str) -> ToolCall {
        ToolCall {
            id: "call_loop".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "loop".to_string(),
                arguments: args.to_string(),
            },
            response: None,
        }
    }

    fn text_response(text: &str) -> LlmResponse {
        LlmResponse::new(
            LlmRequestId::from_string("req".to_string()).unwrap(),
            text.to_string(),
            LlmProvider::new(
                ProviderKind::OpenAi,
                "key".to_string(),
                Some("gpt-4".to_string()),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn tool_call_response(call: ToolCall) -> LlmResponse {
        text_response("").with_tool_calls(vec![call])
    }

    #[tokio::test]
    async fn repeated_signature_nudges_then_rescues_with_synthesis() {
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_tool_exec = MockToolExec::new();
        let key = test_key();

        // Stateful conv mock so the loop's reload sees the persisted prompt.
        let (mock_conv, _conv_state) = stateful_conv_mock(vec![]);

        // The tool must be executed EXACTLY ONCE despite 3 identical requests:
        // occurrence 1 executes, 2 is nudged, 3 triggers rescue (no execution).
        mock_tool_exec.expect_execute().times(1).returning(|call| {
            Ok(ToolResult {
                tool_call_id: call.id.clone(),
                success: true,
                output: "first-result".to_string(),
                error: None,
            })
        });

        // Calls 1-3 → identical tool call; call 4 (synthesis, tool-less) → final text.
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        mock_llm.expect_call().returning(move |_req| {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n < 3 {
                Ok(tool_call_response(loop_tool_call("{}")))
            } else {
                Ok(text_response("Best-effort final answer."))
            }
        });

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
        let result = service
            .run(AgentRunParams {
                session_id: &key,
                prompt: Some("loop me".to_string()),
                messages: None,
                config: create_config(),
                tools: vec![],
                tool_executor: &mock_tool_exec,
                max_tool_repeats: Some(3),
                max_turns: None,
                on_token: None,
                tools_provider: None,
                attachment_resolver: None,
                agent_session_id: None,
            })
            .await;

        assert!(result.is_ok(), "rescue must return Ok, not Err");
        assert_eq!(result.unwrap().content(), "Best-effort final answer.");
    }

    #[tokio::test]
    async fn distinct_signatures_are_never_nudged() {
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_tool_exec = MockToolExec::new();
        let key = test_key();

        // Stateful conv mock so the loop's reload sees the persisted prompt.
        let (mock_conv, _conv_state) = stateful_conv_mock(vec![]);

        // 5 DISTINCT calls all execute (no nudge), then a final text answer.
        mock_tool_exec.expect_execute().times(5).returning(|call| {
            Ok(ToolResult {
                tool_call_id: call.id.clone(),
                success: true,
                output: "ok".to_string(),
                error: None,
            })
        });

        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        mock_llm.expect_call().returning(move |_req| {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n < 5 {
                Ok(tool_call_response(loop_tool_call(&format!(
                    "{{\"i\":{n}}}"
                ))))
            } else {
                Ok(text_response("done"))
            }
        });

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
        let result = service
            .run(AgentRunParams {
                session_id: &key,
                prompt: Some("go".to_string()),
                messages: None,
                config: create_config(),
                tools: vec![],
                tool_executor: &mock_tool_exec,
                max_tool_repeats: Some(3),
                max_turns: None,
                on_token: None,
                tools_provider: None,
                attachment_resolver: None,
                agent_session_id: None,
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content(), "done");
    }

    #[tokio::test]
    async fn streak_resets_when_a_different_signature_appears() {
        // Sequence A, A, B, A: the first A-run reaches 2 (nudge) but never 3
        // because B resets the streak; the final A starts a fresh run. No rescue
        // from the loop guard — the model ends with a normal text answer.
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_tool_exec = MockToolExec::new();
        let key = test_key();

        // Stateful conv mock so the loop's reload sees the persisted prompt.
        let (mock_conv, _conv_state) = stateful_conv_mock(vec![]);

        // A executes (turn 0), A nudged (turn 1, no exec), B executes (turn 2),
        // A executes again (turn 3, fresh streak) → 3 real executions total.
        mock_tool_exec.expect_execute().times(3).returning(|call| {
            Ok(ToolResult {
                tool_call_id: call.id.clone(),
                success: true,
                output: "ok".to_string(),
                error: None,
            })
        });

        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        mock_llm.expect_call().returning(move |_req| {
            let n = c.fetch_add(1, Ordering::SeqCst);
            match n {
                0 | 1 | 3 => Ok(tool_call_response(loop_tool_call("{}"))), // A
                2 => Ok(tool_call_response(ToolCall {
                    id: "call_b".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "other".to_string(),
                        arguments: "{}".to_string(),
                    },
                    response: None,
                })), // B (different signature → resets streak)
                _ => Ok(text_response("finished")),
            }
        });

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
        let result = service
            .run(AgentRunParams {
                session_id: &key,
                prompt: Some("vary".to_string()),
                messages: None,
                config: create_config(),
                tools: vec![],
                tool_executor: &mock_tool_exec,
                max_tool_repeats: Some(3),
                max_turns: None,
                on_token: None,
                tools_provider: None,
                attachment_resolver: None,
                agent_session_id: None,
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content(), "finished");
    }

    #[tokio::test]
    async fn max_turns_ceiling_triggers_synthesis_not_error() {
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_tool_exec = MockToolExec::new();
        let key = test_key();

        // Stateful conv mock so the loop's reload sees the persisted prompt.
        let (mock_conv, _conv_state) = stateful_conv_mock(vec![]);

        // Every turn makes a DISTINCT call (never nudged) so only the turn ceiling
        // can stop the loop. With max_turns = 4: 4 executions, then synthesis.
        mock_tool_exec.expect_execute().times(4).returning(|call| {
            Ok(ToolResult {
                tool_call_id: call.id.clone(),
                success: true,
                output: "ok".to_string(),
                error: None,
            })
        });

        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        mock_llm.expect_call().returning(move |_req| {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n < 4 {
                Ok(tool_call_response(loop_tool_call(&format!(
                    "{{\"i\":{n}}}"
                ))))
            } else {
                Ok(text_response("capped answer"))
            }
        });

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
        let result = service
            .run(AgentRunParams {
                session_id: &key,
                prompt: Some("loop forever".to_string()),
                messages: None,
                config: create_config(),
                tools: vec![],
                tool_executor: &mock_tool_exec,
                max_tool_repeats: Some(3),
                max_turns: Some(4),
                on_token: None,
                tools_provider: None,
                attachment_resolver: None,
                agent_session_id: None,
            })
            .await;

        assert!(
            result.is_ok(),
            "hitting the turn ceiling must synthesize, not error"
        );
        assert_eq!(result.unwrap().content(), "capped answer");
    }

    #[tokio::test]
    async fn single_shot_max_turns_one_returns_directly() {
        // The single-shot callers pass max_turns: Some(1). A turn-1 text answer
        // (no tools) returns normally — the loop guard never engages.
        let mut mock_llm = MockLlmRepo::new();
        let mock_tool_exec = MockToolExec::new();
        let key = test_key();

        let (mock_conv, _conv_state) = stateful_conv_mock(vec![]);
        mock_llm
            .expect_call()
            .times(1)
            .returning(|_| Ok(text_response("one shot")));

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
        let result = service
            .run(AgentRunParams {
                session_id: &key,
                prompt: Some("answer once".to_string()),
                messages: None,
                config: create_config(),
                tools: vec![],
                tool_executor: &mock_tool_exec,
                max_tool_repeats: None,
                max_turns: Some(1),
                on_token: None,
                tools_provider: None,
                attachment_resolver: None,
                agent_session_id: None,
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content(), "one shot");
    }

    #[tokio::test]
    async fn two_identical_calls_in_one_turn_nudges_the_second() {
        // A SINGLE assistant turn emits the same `(name+args)` signature twice
        // (distinct tool_call_ids). The streak updates per processed tool call,
        // so the first runs and the second is nudged WITHIN the same turn — the
        // tool executes exactly once. The next turn answers with text.
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_tool_exec = MockToolExec::new();
        let key = test_key();

        // Stateful conv mock so the loop's reload sees the persisted prompt.
        let (mock_conv, _conv_state) = stateful_conv_mock(vec![]);

        // Only the first of the two identical calls executes.
        mock_tool_exec.expect_execute().times(1).returning(|call| {
            Ok(ToolResult {
                tool_call_id: call.id.clone(),
                success: true,
                output: "ok".to_string(),
                error: None,
            })
        });

        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        mock_llm.expect_call().returning(move |_req| {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                let twin = |id: &str| ToolCall {
                    id: id.to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "loop".to_string(),
                        arguments: "{}".to_string(),
                    },
                    response: None,
                };
                Ok(text_response("").with_tool_calls(vec![twin("c1"), twin("c2")]))
            } else {
                Ok(text_response("done"))
            }
        });

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
        let result = service
            .run(AgentRunParams {
                session_id: &key,
                prompt: Some("twin".to_string()),
                messages: None,
                config: create_config(),
                tools: vec![],
                tool_executor: &mock_tool_exec,
                max_tool_repeats: Some(3),
                max_turns: None,
                on_token: None,
                tools_provider: None,
                attachment_resolver: None,
                agent_session_id: None,
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().content(), "done");
    }

    #[tokio::test]
    async fn three_identical_calls_in_one_turn_rescue_intra_turn() {
        // A SINGLE assistant turn emits the same `(name+args)` signature THREE
        // times (distinct tool_call_ids). The streak climbs 1→2→3 WITHIN the one
        // turn: 1st executes, 2nd is nudged, 3rd hits max_tool_repeats and flags
        // `rescue`. After the inner tool loop the `if rescue { break; }` exits the
        // turn loop and the forced tool-less synthesis runs, returned as Ok. The
        // tool executes exactly once; every tool_call_id is still answered.
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_tool_exec = MockToolExec::new();
        let key = test_key();

        // Stateful conv mock so the loop's reload sees the persisted prompt.
        let (mock_conv, _conv_state) = stateful_conv_mock(vec![]);

        // Only the first of the three identical calls executes.
        mock_tool_exec.expect_execute().times(1).returning(|call| {
            Ok(ToolResult {
                tool_call_id: call.id.clone(),
                success: true,
                output: "ok".to_string(),
                error: None,
            })
        });

        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        mock_llm.expect_call().returning(move |_req| {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                let triplet = |id: &str| ToolCall {
                    id: id.to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "loop".to_string(),
                        arguments: "{}".to_string(),
                    },
                    response: None,
                };
                Ok(text_response("").with_tool_calls(vec![
                    triplet("c1"),
                    triplet("c2"),
                    triplet("c3"),
                ]))
            } else {
                // The post-rescue forced synthesis is tool-less → return text.
                Ok(text_response("best effort after intra-turn rescue"))
            }
        });

        let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
        let result = service
            .run(AgentRunParams {
                session_id: &key,
                prompt: Some("triple".to_string()),
                messages: None,
                config: create_config(),
                tools: vec![],
                tool_executor: &mock_tool_exec,
                max_tool_repeats: Some(3),
                max_turns: None,
                on_token: None,
                tools_provider: None,
                attachment_resolver: None,
                agent_session_id: None,
            })
            .await;

        assert!(
            result.is_ok(),
            "intra-turn rescue must synthesize, not error"
        );
        assert_eq!(
            result.unwrap().content(),
            "best effort after intra-turn rescue"
        );
    }

    /// When a tool returns `__colmena_status: "SUSPENDED"` the agent service must
    /// stop iterating, persist only the assistant message (not a tool message), and
    /// return an `LlmResponse` whose `suspend()` is `Some`.
    #[tokio::test]
    async fn detects_suspended_tool_result_and_short_circuits() {
        let mut mock_llm = MockLlmRepo::new();
        let mut mock_tool_exec = MockToolExec::new();

        let key = test_key();
        let prompt = "hello".to_string();

        // Conversation starts empty; stateful so the loop's reload sees the
        // persisted prompt. We assert the persisted count at the end:
        //   1. user message
        //   2. assistant message (with tool_calls) — persisted before the loop
        // The tool result must NOT be persisted (we short-circuit before that).
        let (mock_conv, conv_state) = stateful_conv_mock(vec![]);

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
                max_tool_repeats: None,
                max_turns: None,
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

        // Only user + assistant(tool_calls) persisted — tool result NOT saved.
        assert_eq!(conv_state.lock().unwrap().len(), 2);
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

        // In-memory conversation repo. Reads reflect what was persisted so the
        // loop's reload (get_with_summaries) sees the prompt.
        let persisted: Arc<Mutex<Vec<LlmMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let persisted_for_get = persisted.clone();
        mock_conv.expect_get_by_id().returning(move |key| {
            Ok(Conversation {
                key: key.clone(),
                messages: persisted_for_get.lock().unwrap().clone(),
            })
        });
        let persisted_for_summaries = persisted.clone();
        mock_conv
            .expect_get_with_summaries()
            .returning(move |_key| {
                Ok(persisted_for_summaries
                    .lock()
                    .unwrap()
                    .iter()
                    .cloned()
                    .map(|message| StoredMessage {
                        message,
                        summary: None,
                    })
                    .collect())
            });
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
            max_tool_repeats: Some(5),
            max_turns: None,
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

        // Reads reflect persisted messages so the loop's reload sees the prompt.
        let persisted: Arc<Mutex<Vec<LlmMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let persisted_for_get = persisted.clone();
        mock_conv.expect_get_by_id().returning(move |key| {
            Ok(Conversation {
                key: key.clone(),
                messages: persisted_for_get.lock().unwrap().clone(),
            })
        });
        let persisted_for_summaries = persisted.clone();
        mock_conv
            .expect_get_with_summaries()
            .returning(move |_key| {
                Ok(persisted_for_summaries
                    .lock()
                    .unwrap()
                    .iter()
                    .cloned()
                    .map(|message| StoredMessage {
                        message,
                        summary: None,
                    })
                    .collect())
            });
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
            max_tool_repeats: Some(5),
            max_turns: None,
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

        let persisted: Arc<Mutex<Vec<LlmMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let persisted_for_get = persisted.clone();
        mock_conv.expect_get_by_id().returning(move |key| {
            Ok(Conversation {
                key: key.clone(),
                messages: persisted_for_get.lock().unwrap().clone(),
            })
        });
        let persisted_for_summaries = persisted.clone();
        mock_conv
            .expect_get_with_summaries()
            .returning(move |_key| {
                Ok(persisted_for_summaries
                    .lock()
                    .unwrap()
                    .iter()
                    .cloned()
                    .map(|message| StoredMessage {
                        message,
                        summary: None,
                    })
                    .collect())
            });
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
            max_tool_repeats: Some(5),
            max_turns: None,
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
