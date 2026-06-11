use crate::llm::domain::{
    FileSource, FunctionCall, LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream,
    LlmStreamChunk, LlmStreamPart, LlmUsage, MessageRole, ToolCall, ToolCallChunk,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::task::{Context, Poll};

pub struct AnthropicAdapter {
    client: Client,
    base_url: String,
}

impl Default for AnthropicAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicAdapter {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: "https://api.anthropic.com/v1".to_string(),
        }
    }

    pub fn with_base_url(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    /// The configured endpoint. Exposed for tests and diagnostics.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn convert_messages(
        &self,
        request: &LlmRequest,
    ) -> Result<(Option<String>, Vec<AnthropicMessage>), LlmError> {
        let mut system_message = None;
        let mut messages = Vec::new();

        for message in request.messages() {
            match message.role() {
                MessageRole::System => {
                    system_message = Some(message.content().to_string());
                }
                MessageRole::User => {
                    if let Some(files) = message.files() {
                        let mut blocks: Vec<AnthropicContentBlock> = Vec::new();
                        for file in files {
                            let source = match &file.source {
                                FileSource::InlineBytes { bytes } => AnthropicMediaSource {
                                    source_type: "base64".to_string(),
                                    media_type: Some(file.mime_type.clone()),
                                    data: Some(STANDARD.encode(bytes)),
                                    file_id: None,
                                    url: None,
                                },
                                FileSource::Uploaded(r) => AnthropicMediaSource {
                                    source_type: "file".to_string(),
                                    media_type: None,
                                    data: None,
                                    file_id: Some(r.provider_file_id.clone()),
                                    url: None,
                                },
                                FileSource::SignedUrl(url)
                                    if file.mime_type.starts_with("image/") =>
                                {
                                    // Anthropic does not accept file_id for images. Pass the
                                    // signed URL directly; Anthropic fetches it server-side.
                                    AnthropicMediaSource {
                                        source_type: "url".to_string(),
                                        media_type: None,
                                        data: None,
                                        file_id: None,
                                        url: Some(url.clone()),
                                    }
                                }
                                FileSource::SignedUrl(_) => {
                                    return Err(LlmError::InternalError {
                                        message: format!(
                                            "Anthropic adapter received unresolved SignedUrl for non-image file '{}' \
                                             (mime '{}'). Non-image SignedUrl files must be resolved via Files API \
                                             by LlmCallUseCase::resolve_files before reaching the adapter.",
                                            file.filename, file.mime_type
                                        ),
                                    });
                                }
                            };
                            if file.mime_type.starts_with("image/") {
                                blocks.push(AnthropicContentBlock::Image { source });
                            } else if file.mime_type == "application/pdf" {
                                blocks.push(AnthropicContentBlock::Document { source });
                            } else {
                                eprintln!(
                                    "WARN: Anthropic adapter does not support media type '{}'. Skipping file '{}'.",
                                    file.mime_type, file.filename
                                );
                            }
                        }
                        // Image/Document blocks come before the text per Anthropic's recommendation.
                        if !message.content().is_empty() {
                            blocks.push(AnthropicContentBlock::Text {
                                text: message.content().to_string(),
                            });
                        }
                        messages.push(AnthropicMessage {
                            role: "user".to_string(),
                            content: AnthropicContent::Blocks(blocks),
                        });
                    } else {
                        messages.push(AnthropicMessage {
                            role: "user".to_string(),
                            content: AnthropicContent::Text(message.content().to_string()),
                        });
                    }
                }
                MessageRole::Assistant => {
                    if let Some(tool_calls) = message.tool_calls() {
                        let mut blocks: Vec<AnthropicContentBlock> = Vec::new();
                        if !message.content().is_empty() {
                            blocks.push(AnthropicContentBlock::Text {
                                text: message.content().to_string(),
                            });
                        }
                        for tc in tool_calls {
                            let input: serde_json::Value =
                                serde_json::from_str(&tc.function.arguments)
                                    .unwrap_or(serde_json::json!({}));
                            blocks.push(AnthropicContentBlock::ToolUse {
                                id: tc.id.clone(),
                                name: tc.function.name.clone(),
                                input,
                            });
                        }
                        messages.push(AnthropicMessage {
                            role: "assistant".to_string(),
                            content: AnthropicContent::Blocks(blocks),
                        });
                    } else {
                        messages.push(AnthropicMessage {
                            role: "assistant".to_string(),
                            content: AnthropicContent::Text(message.content().to_string()),
                        });
                    }
                }
                MessageRole::Tool => {
                    // Anthropic encodes tool results as a `user` message with a single
                    // `tool_result` content block that references the assistant's tool_use id.
                    let tool_use_id = message.tool_call_id().unwrap_or_default().to_string();
                    messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: AnthropicContent::Blocks(vec![
                            AnthropicContentBlock::ToolResult {
                                tool_use_id,
                                content: message.content().to_string(),
                            },
                        ]),
                    });
                }
            }
        }

        Ok((system_message, messages))
    }

    fn build_request_body(&self, request: &LlmRequest) -> Result<serde_json::Value, LlmError> {
        let (system_message, messages) = self.convert_messages(request)?;

        let mut body = json!({
            "model": request.config().model(),
            "messages": messages,
            "stream": request.stream()
        });

        // Prompt caching (default ON, 2026-06-09).
        // Anthropic caches the *prefix* of the request up to a marked breakpoint
        // for 5 minutes (ephemeral). Two breakpoints maximize cache hits:
        //
        //   1. System message — stable across turns of the same agent.
        //   2. Last tool definition — anchors a cacheable tools[] prefix
        //      (tools are unlikely to change mid-conversation).
        //
        // Marker shape: serialize as a content-block array with
        // `cache_control: {type: "ephemeral"}` on the block. A plain string
        // `system` still works for non-cached requests, but we always use the
        // block form when there is a system message. Net effect on uncached
        // requests: zero — Anthropic accepts both shapes and bills identically.
        // On cached requests (repeats within 5 min): system + tools billed at
        // ~10% of the normal rate.
        //
        // We do NOT add a marker on user/assistant messages because the
        // conversation tail changes every turn — caching it would cause
        // constant cache-write churn with no read benefit.
        // Cache-safe temporal suffix (2026-06-11). When the config carries a
        // `volatile_system_suffix` (the per-turn temporal block), it is emitted
        // as a SECOND system block WITHOUT a cache_control marker. The cache
        // breakpoint stays on the first (stable) block, so the changing
        // timestamp lives outside the cached prefix and never busts it. When
        // there is no stable system but there IS a suffix, the suffix becomes
        // the (uncached) system on its own.
        let volatile_suffix = request.config().volatile_system_suffix();
        match (system_message, volatile_suffix) {
            (Some(system), Some(suffix)) => {
                body["system"] = json!([
                    {
                        "type": "text",
                        "text": system,
                        "cache_control": {"type": "ephemeral"}
                    },
                    {
                        "type": "text",
                        "text": suffix
                    }
                ]);
            }
            (Some(system), None) => {
                body["system"] = json!([{
                    "type": "text",
                    "text": system,
                    "cache_control": {"type": "ephemeral"}
                }]);
            }
            (None, Some(suffix)) => {
                // No stable system to cache; the suffix is the whole system.
                body["system"] = json!([{
                    "type": "text",
                    "text": suffix
                }]);
            }
            (None, None) => {}
        }

        if let Some(temp) = request.config().temperature() {
            body["temperature"] = json!(temp);
        }

        // Anthropic's Messages API requires max_tokens. Fall back to a
        // conservative default when the caller didn't configure one so nodes
        // (planner, sub-agents, final_reactor) work out-of-the-box like the
        // OpenAI and Gemini adapters do.
        let max_tokens = request.config().max_tokens().unwrap_or(4096);
        body["max_tokens"] = json!(max_tokens);

        if let Some(top_p) = request.config().top_p() {
            body["top_p"] = json!(top_p);
        }

        if let Some(budget) = request.config().thinking_budget() {
            body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
            // Extended thinking requires temperature = 1; remove any other value.
            body.as_object_mut().map(|o| o.remove("temperature"));
        }

        if let Some(tools) = request.tools() {
            let mut anthropic_tools: Vec<serde_json::Value> = tools
                .iter()
                .map(|tool| {
                    let input_schema = tool.input_schema_override.clone().unwrap_or_else(|| {
                        json!({
                            "type": tool.parameters.schema_type,
                            "properties": tool.parameters.properties,
                            "required": tool.parameters.required,
                        })
                    });
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": input_schema,
                    })
                })
                .collect();

            if !anthropic_tools.is_empty() {
                // Prompt caching marker on the LAST tool (see comment on system
                // above). Anthropic interprets this as "everything up to here
                // is cacheable" — so all tool defs in front of the marker are
                // included in the cached prefix.
                if let Some(last) = anthropic_tools.last_mut() {
                    if let Some(obj) = last.as_object_mut() {
                        obj.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
                    }
                }
                body["tools"] = json!(anthropic_tools);
            }
        }

        Ok(body)
    }
}

#[async_trait]
impl LlmRepository for AnthropicAdapter {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let body = self.build_request_body(&request)?;

        let response = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", request.config().api_key())
            .header("Content-Type", "application/json")
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "files-api-2025-04-14")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::network_error(e.to_string()))?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(LlmError::request_failed(format!(
                "Anthropic API error: {}",
                error_text
            )));
        }

        let anthropic_response: AnthropicResponse = response
            .json()
            .await
            .map_err(|e| LlmError::parsing_error(e.to_string()))?;

        let mut text_parts: Vec<String> = Vec::new();
        let mut thinking_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in anthropic_response.content {
            match block {
                AnthropicResponseBlock::Text { text } => text_parts.push(text),
                AnthropicResponseBlock::Thinking { thinking } => thinking_parts.push(thinking),
                AnthropicResponseBlock::ToolUse { id, name, input } => {
                    let arguments = serde_json::to_string(&input)
                        .map_err(|e| LlmError::parsing_error(e.to_string()))?;
                    tool_calls.push(ToolCall::new(id, FunctionCall::new(name, arguments)));
                }
                AnthropicResponseBlock::Other => {}
            }
        }

        let content = text_parts.join("");
        let thinking = thinking_parts.join("");
        let raw = &anthropic_response.usage;
        let mut usage = LlmUsage::new(raw.input_tokens, raw.output_tokens);
        if raw.cache_read_input_tokens > 0 {
            usage = usage.with_cache_read_tokens(raw.cache_read_input_tokens);
        }
        if raw.cache_creation_input_tokens > 0 {
            usage = usage.with_cache_write_tokens(raw.cache_creation_input_tokens);
        }

        let mut response = LlmResponse::new(
            request.id().clone(),
            content,
            request.config().provider().clone(),
        )?
        .with_usage(usage);

        if !thinking.is_empty() {
            response = response.with_thinking_content(thinking);
        }

        if !tool_calls.is_empty() {
            response = response.with_tool_calls(tool_calls);
        }

        if let Some(stop_reason) = anthropic_response.stop_reason {
            response = response.with_finish_reason(stop_reason);
        }

        Ok(response)
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        let body = self.build_request_body(&request)?;

        let response = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", request.config().api_key())
            .header("Content-Type", "application/json")
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "files-api-2025-04-14")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::network_error(e.to_string()))?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(LlmError::request_failed(format!(
                "Anthropic API error: {}",
                error_text
            )));
        }

        let request_id = request.id().clone();
        let provider = request.config().provider().clone();

        let byte_stream = response.bytes_stream();
        let mut sse_parser = SseParser::new(byte_stream);

        // Tracks per-block-index metadata (id + name) for tool_use blocks so that
        // subsequent input_json_delta events can be attributed to the right tool call.
        let mut tool_state: HashMap<usize, (String, String)> = HashMap::new();
        // Indices of tool_use blocks that emitted at least one input_json_delta.
        // Blocks missing from this set on content_block_stop are zero-arg tool calls
        // (e.g. get_amadeus_token()) for which Anthropic never emits deltas — we
        // synthesize `{}` so the downstream accumulator produces parseable arguments.
        let mut tool_received_delta: HashSet<usize> = HashSet::new();
        // Indices of thinking blocks (for ThinkingStart/End lifecycle).
        let mut thinking_blocks: HashSet<usize> = HashSet::new();
        let mut stop_reason: Option<String> = None;
        // Usage tracking across message_start / message_delta events.
        let mut stream_input_tokens: u32 = 0;
        let mut stream_cache_read: u32 = 0;
        let mut stream_cache_write: u32 = 0;

        let sse_stream = async_stream::try_stream! {
            while let Some(event_result) = sse_parser.next().await {
                let event = event_result?;
                let SseEvent::Message(data) = event;
                let parsed: AnthropicStreamEvent = match serde_json::from_str(&data) {
                    Ok(ev) => ev,
                    Err(_) => continue, // tolerate ping / unknown events
                };

                match parsed.event_type.as_str() {
                    "content_block_start" => {
                        if let (Some(idx), Some(block)) = (parsed.index, parsed.content_block) {
                            match block {
                                AnthropicStreamBlock::ToolUse { id, name, .. } => {
                                    tool_state.insert(idx, (id.clone(), name.clone()));
                                    yield LlmStreamChunk::new(
                                        request_id.clone(),
                                        LlmStreamPart::ToolCallChunk(ToolCallChunk {
                                            index: idx,
                                            id,
                                            name,
                                            args_chunk: String::new(),
                                        }),
                                        provider.clone(),
                                        false,
                                    );
                                }
                                AnthropicStreamBlock::Thinking { .. } => {
                                    thinking_blocks.insert(idx);
                                    yield LlmStreamChunk::new(
                                        request_id.clone(),
                                        LlmStreamPart::ThinkingStart,
                                        provider.clone(),
                                        false,
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                    "content_block_delta" => {
                        if let Some(delta) = parsed.delta {
                            match delta.delta_type.as_deref() {
                                Some("text_delta") => {
                                    if let Some(text) = delta.text {
                                        yield LlmStreamChunk::new(
                                            request_id.clone(),
                                            LlmStreamPart::Content(text),
                                            provider.clone(),
                                            false,
                                        );
                                    }
                                }
                                Some("thinking_delta") => {
                                    if let Some(thinking) = delta.thinking {
                                        yield LlmStreamChunk::new(
                                            request_id.clone(),
                                            LlmStreamPart::ThinkingContent(thinking),
                                            provider.clone(),
                                            false,
                                        );
                                    }
                                }
                                Some("input_json_delta") => {
                                    if let (Some(idx), Some(partial)) = (parsed.index, delta.partial_json) {
                                        if let Some((id, name)) = tool_state.get(&idx) {
                                            // Anthropic emits a single empty input_json_delta for
                                            // zero-arg tools. Only count non-empty deltas so the
                                            // content_block_stop fallback can synthesize `{}`.
                                            if !partial.is_empty() {
                                                tool_received_delta.insert(idx);
                                                yield LlmStreamChunk::new(
                                                    request_id.clone(),
                                                    LlmStreamPart::ToolCallChunk(ToolCallChunk {
                                                        index: idx,
                                                        id: id.clone(),
                                                        name: name.clone(),
                                                        args_chunk: partial,
                                                    }),
                                                    provider.clone(),
                                                    false,
                                                );
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    "content_block_stop" => {
                        if let Some(idx) = parsed.index {
                            if thinking_blocks.remove(&idx) {
                                yield LlmStreamChunk::new(
                                    request_id.clone(),
                                    LlmStreamPart::ThinkingEnd,
                                    provider.clone(),
                                    false,
                                );
                            } else if let Some((id, name)) = tool_state.get(&idx) {
                                if !tool_received_delta.contains(&idx) {
                                    yield LlmStreamChunk::new(
                                        request_id.clone(),
                                        LlmStreamPart::ToolCallChunk(ToolCallChunk {
                                            index: idx,
                                            id: id.clone(),
                                            name: name.clone(),
                                            args_chunk: "{}".to_string(),
                                        }),
                                        provider.clone(),
                                        false,
                                    );
                                }
                            }
                        }
                    }
                    "message_start" => {
                        if let Some(msg) = parsed.message {
                            if let Some(u) = msg.usage {
                                stream_input_tokens = u.input_tokens;
                                stream_cache_read = u.cache_read_input_tokens;
                                stream_cache_write = u.cache_creation_input_tokens;
                            }
                        }
                    }
                    "message_delta" => {
                        if let Some(delta) = parsed.delta {
                            if let Some(reason) = delta.stop_reason {
                                stop_reason = Some(reason);
                            }
                        }
                        // message_delta carries final output_tokens count
                        if let Some(u) = parsed.usage {
                            let mut usage = LlmUsage::new(stream_input_tokens, u.output_tokens);
                            if stream_cache_read > 0 {
                                usage = usage.with_cache_read_tokens(stream_cache_read);
                            }
                            if stream_cache_write > 0 {
                                usage = usage.with_cache_write_tokens(stream_cache_write);
                            }
                            yield LlmStreamChunk::new(
                                request_id.clone(),
                                LlmStreamPart::Usage(usage),
                                provider.clone(),
                                false,
                            );
                        }
                    }
                    "message_stop" => {
                        let mut chunk = LlmStreamChunk::new(
                            request_id.clone(),
                            LlmStreamPart::Content(String::new()),
                            provider.clone(),
                            true,
                        );
                        if let Some(reason) = stop_reason.clone() {
                            chunk = chunk.with_finish_reason(reason);
                        }
                        yield chunk;
                    }
                    _ => {}
                }
            }
        };

        Ok(Box::pin(sse_stream))
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        // Anthropic doesn't have a dedicated health check endpoint
        // We'll make a minimal request to test connectivity
        let minimal_body = json!({
            "model": "claude-3-haiku-20240307",
            "messages": [{"role": "user", "content": "Hi"}],
            "max_tokens": 1
        });

        let response = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("Content-Type", "application/json")
            .header("anthropic-version", "2023-06-01")
            .json(&minimal_body)
            .send()
            .await
            .map_err(|e| LlmError::network_error(e.to_string()))?;

        if response.status().is_success() || response.status().as_u16() == 401 {
            // 401 means the endpoint is working but we need a valid API key
            Ok(())
        } else {
            Err(LlmError::request_failed("Anthropic health check failed"))
        }
    }

    fn provider_name(&self) -> &'static str {
        "anthropic"
    }
}

// Request structures for Anthropic API
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum AnthropicContent {
    /// Plain text shorthand — serializes as `"content": "hello"`.
    Text(String),
    /// Structured content blocks — serializes as `"content": [ {...}, ... ]`.
    Blocks(Vec<AnthropicContentBlock>),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text {
        text: String,
    },
    Image {
        source: AnthropicMediaSource,
    },
    Document {
        source: AnthropicMediaSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct AnthropicMediaSource {
    #[serde(rename = "type")]
    source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

// Response structures for Anthropic API
#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicResponseBlock>,
    usage: AnthropicUsage,
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicResponseBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tolerate unknown / server-side blocks (e.g. `tool_search_tool_result`,
    /// `server_tool_use`) so the adapter does not crash if the caller enables
    /// beta server-side tools.
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
}

// Streaming response structures
#[derive(Debug, Deserialize)]
struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    index: Option<usize>,
    content_block: Option<AnthropicStreamBlock>,
    delta: Option<AnthropicStreamDelta>,
    /// Present in `message_start` — carries initial input token count.
    message: Option<AnthropicStreamMessage>,
    /// Present in `message_delta` — carries final output token count.
    usage: Option<AnthropicStreamUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamMessage {
    usage: Option<AnthropicStreamUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicStreamBlock {
    Text {
        #[allow(dead_code)]
        text: Option<String>,
    },
    Thinking {
        #[allow(dead_code)]
        thinking: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        #[allow(dead_code)]
        input: Option<serde_json::Value>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamDelta {
    #[serde(rename = "type")]
    delta_type: Option<String>,
    text: Option<String>,
    /// Incremental thinking text (delta_type = "thinking_delta")
    thinking: Option<String>,
    partial_json: Option<String>,
    stop_reason: Option<String>,
}

// SSE Parser implementation
enum SseEvent {
    Message(String),
}

struct SseParser<S>
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    stream: S,
    buffer: Vec<u8>,
}

impl<S> SseParser<S>
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
        }
    }
}

impl<S> Stream for SseParser<S>
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<SseEvent, LlmError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            // Check for a complete message in the buffer
            if let Some(i) = self.buffer.windows(2).position(|w| w == b"\n\n") {
                let message_bytes = self.buffer.drain(..i + 2).collect::<Vec<u8>>();
                let msg_str = String::from_utf8_lossy(&message_bytes);

                for line in msg_str.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        return Poll::Ready(Some(Ok(SseEvent::Message(data.to_string()))));
                    }
                }
                // Continue loop if message was parsed but no data field found
                continue;
            }

            // Buffer not ready, poll the underlying stream
            match self.stream.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.buffer.extend_from_slice(&chunk);
                    // Loop again to check if a full message is now in the buffer
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(LlmError::network_error(e.to_string()))));
                }
                Poll::Ready(None) => {
                    // Stream is finished. If there's anything left in the buffer, it's an incomplete message.
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_uses_production_default() {
        let a = AnthropicAdapter::new();
        assert_eq!(a.base_url(), "https://api.anthropic.com/v1");
    }

    #[test]
    fn with_base_url_overrides() {
        let a = AnthropicAdapter::with_base_url("http://127.0.0.1:4000/anthropic".to_string());
        assert_eq!(a.base_url(), "http://127.0.0.1:4000/anthropic");
    }

    fn build_request_with_file(file: crate::llm::domain::FileData) -> LlmRequest {
        use crate::llm::domain::{LlmConfig, LlmMessage, LlmProvider, ProviderKind};
        let msg = LlmMessage::user_with_files("describe".into(), vec![file]).unwrap();
        let provider =
            LlmProvider::new(ProviderKind::Anthropic, "k".into(), Some("claude-3".into())).unwrap();
        let config = LlmConfig::new(provider);
        LlmRequest::new(vec![msg], config, false).unwrap()
    }

    #[test]
    fn convert_messages_serializes_uploaded_pdf_as_file_id() {
        use crate::llm::domain::{FileData, FileSource, ProviderFileRef, ProviderKind};
        let file = FileData {
            document_id: Some("doc-1".into()),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: None,
            source: FileSource::Uploaded(ProviderFileRef {
                provider: ProviderKind::Anthropic,
                provider_file_id: "file_01abc".into(),
                mime_type: "application/pdf".into(),
                filename: "x.pdf".into(),
                expires_at: None,
            }),
            retained_inline_bytes: None,
        };
        let request = build_request_with_file(file);

        let adapter = AnthropicAdapter::new();
        let (_sys, anth_messages) = adapter.convert_messages(&request).unwrap();
        let json = serde_json::to_value(&anth_messages).unwrap();
        let blocks = json[0]["content"].as_array().unwrap();
        let doc_block = blocks.iter().find(|b| b["type"] == "document").unwrap();
        assert_eq!(doc_block["source"]["type"], "file");
        assert_eq!(doc_block["source"]["file_id"], "file_01abc");
        assert!(doc_block["source"].get("data").is_none() || doc_block["source"]["data"].is_null());
        assert!(
            doc_block["source"].get("media_type").is_none()
                || doc_block["source"]["media_type"].is_null()
        );
    }

    #[test]
    fn convert_messages_returns_error_on_signed_url() {
        use crate::llm::domain::{FileData, FileSource};
        let file = FileData {
            document_id: Some("doc-1".into()),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: None,
            source: FileSource::SignedUrl("https://example/x?sig=y".into()),
            retained_inline_bytes: None,
        };
        let request = build_request_with_file(file);

        let adapter = AnthropicAdapter::new();
        let err = adapter.convert_messages(&request).unwrap_err();
        assert!(matches!(err, LlmError::InternalError { .. }));
    }

    #[test]
    fn convert_messages_serializes_signed_url_image_as_url_source() {
        use crate::llm::domain::{
            FileData, FileSource, LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderKind,
        };
        let file = FileData {
            document_id: Some("doc-img-1".into()),
            mime_type: "image/jpeg".into(),
            filename: "x.jpeg".into(),
            size_hint: None,
            source: FileSource::SignedUrl("https://storage.googleapis.com/bucket/x?sig=y".into()),
            retained_inline_bytes: None,
        };
        let msg = LlmMessage::user_with_files("describe".into(), vec![file]).unwrap();
        let provider =
            LlmProvider::new(ProviderKind::Anthropic, "k".into(), Some("claude-3".into())).unwrap();
        let config = LlmConfig::new(provider);
        let request = LlmRequest::new(vec![msg], config, false).unwrap();

        let adapter = AnthropicAdapter::new();
        let (_sys, anth_messages) = adapter.convert_messages(&request).unwrap();
        let json = serde_json::to_value(&anth_messages).unwrap();
        let blocks = json[0]["content"].as_array().unwrap();
        let img_block = blocks.iter().find(|b| b["type"] == "image").unwrap();
        assert_eq!(img_block["source"]["type"], "url");
        assert_eq!(
            img_block["source"]["url"],
            "https://storage.googleapis.com/bucket/x?sig=y"
        );
        assert!(
            img_block["source"].get("file_id").is_none()
                || img_block["source"]["file_id"].is_null()
        );
        assert!(img_block["source"].get("data").is_none() || img_block["source"]["data"].is_null());
    }

    #[test]
    fn convert_messages_returns_error_on_signed_url_for_non_image() {
        use crate::llm::domain::{
            FileData, FileSource, LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderKind,
        };
        let file = FileData {
            document_id: Some("doc-pdf-1".into()),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: None,
            source: FileSource::SignedUrl("https://example/x?sig=y".into()),
            retained_inline_bytes: None,
        };
        let msg = LlmMessage::user_with_files("describe".into(), vec![file]).unwrap();
        let provider =
            LlmProvider::new(ProviderKind::Anthropic, "k".into(), Some("claude-3".into())).unwrap();
        let config = LlmConfig::new(provider);
        let request = LlmRequest::new(vec![msg], config, false).unwrap();

        let adapter = AnthropicAdapter::new();
        let err = adapter.convert_messages(&request).unwrap_err();
        assert!(matches!(
            err,
            crate::llm::domain::LlmError::InternalError { .. }
        ));
    }

    #[test]
    fn convert_messages_serializes_inline_pdf_as_base64() {
        use crate::llm::domain::{FileData, FileSource};
        let file = FileData {
            document_id: None,
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: None,
            source: FileSource::InlineBytes {
                bytes: b"%PDF-1.4 hello".to_vec(),
            },
            retained_inline_bytes: None,
        };
        let request = build_request_with_file(file);

        let adapter = AnthropicAdapter::new();
        let (_sys, anth_messages) = adapter.convert_messages(&request).unwrap();
        let json = serde_json::to_value(&anth_messages).unwrap();
        let blocks = json[0]["content"].as_array().unwrap();
        let doc_block = blocks.iter().find(|b| b["type"] == "document").unwrap();
        assert_eq!(doc_block["source"]["type"], "base64");
        assert_eq!(doc_block["source"]["media_type"], "application/pdf");
        assert!(doc_block["source"]["data"].is_string());
        assert!(doc_block["source"].get("file_id").is_none());
    }

    // ---------------------------------------------------------------------
    // Prompt caching (item 11, 2026-06-09) — tests verify that the request
    // body marks the system message and the last tool as cacheable so
    // Anthropic bills repeat calls within 5 min at ~10% of the normal rate.
    // ---------------------------------------------------------------------

    fn anth_request_with_system_and_tools(system: &str, tool_names: &[&str]) -> LlmRequest {
        use crate::llm::domain::tools::{ToolDefinition, ToolParameters};
        use crate::llm::domain::{LlmConfig, LlmMessage, LlmProvider, ProviderKind};
        let messages = vec![
            LlmMessage::system(system.into()).unwrap(),
            LlmMessage::user("hi".into()).unwrap(),
        ];
        let provider = LlmProvider::new(
            ProviderKind::Anthropic,
            "k".into(),
            Some("claude-3-5-sonnet".into()),
        )
        .unwrap();
        let config = LlmConfig::new(provider);
        let tools: Vec<ToolDefinition> = tool_names
            .iter()
            .map(|name| {
                ToolDefinition::new(
                    (*name).into(),
                    format!("desc {}", name),
                    ToolParameters::new(),
                )
            })
            .collect();
        let mut req = LlmRequest::new(messages, config, false).unwrap();
        if !tools.is_empty() {
            req = req.with_tools(tools);
        }
        req
    }

    #[test]
    fn cache_control_marker_on_system_message_block() {
        let adapter = AnthropicAdapter::new();
        let req = anth_request_with_system_and_tools("you are an agent", &[]);
        let body = adapter.build_request_body(&req).unwrap();

        // System must be a content-block array (NOT a plain string), with
        // the cache_control marker on the single text block.
        let system = body.get("system").expect("system field present");
        let arr = system.as_array().expect("system serialized as block array");
        assert_eq!(arr.len(), 1, "exactly one system block");
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[0]["text"], "you are an agent");
        assert_eq!(
            arr[0]["cache_control"]["type"], "ephemeral",
            "system message must carry cache_control: ephemeral"
        );
    }

    #[test]
    fn cache_control_marker_on_last_tool_only() {
        let adapter = AnthropicAdapter::new();
        let req =
            anth_request_with_system_and_tools("you are an agent", &["tool_a", "tool_b", "tool_c"]);
        let body = adapter.build_request_body(&req).unwrap();

        let tools = body
            .get("tools")
            .and_then(|t| t.as_array())
            .expect("tools array present");
        assert_eq!(tools.len(), 3);

        // First two tools: no cache_control.
        assert!(
            tools[0].get("cache_control").is_none(),
            "first tool must NOT carry cache_control"
        );
        assert!(
            tools[1].get("cache_control").is_none(),
            "middle tool must NOT carry cache_control"
        );

        // Last tool: cache_control: ephemeral (anchors cacheable prefix).
        assert_eq!(
            tools[2]["cache_control"]["type"], "ephemeral",
            "last tool must carry cache_control: ephemeral"
        );
        // Tool fields preserved.
        assert_eq!(tools[2]["name"], "tool_c");
        assert!(tools[2]["input_schema"].is_object());
    }

    #[test]
    fn cache_control_works_without_tools() {
        // Smoke: when there are no tools, the body must still be valid
        // (no `tools` key) and system still carries the marker.
        let adapter = AnthropicAdapter::new();
        let req = anth_request_with_system_and_tools("hello", &[]);
        let body = adapter.build_request_body(&req).unwrap();

        assert!(body.get("tools").is_none(), "no tools key when empty");
        let arr = body["system"].as_array().unwrap();
        assert_eq!(arr[0]["cache_control"]["type"], "ephemeral");
    }

    // ── Cache-safe temporal suffix (2026-06-11) ──────────────────────────

    fn anth_request_with_suffix(system: &str, suffix: Option<&str>) -> LlmRequest {
        use crate::llm::domain::{LlmConfig, LlmMessage, LlmProvider, ProviderKind};
        let messages = vec![
            LlmMessage::system(system.into()).unwrap(),
            LlmMessage::user("hi".into()).unwrap(),
        ];
        let provider = LlmProvider::new(
            ProviderKind::Anthropic,
            "k".into(),
            Some("claude-3-5-sonnet".into()),
        )
        .unwrap();
        let mut config = LlmConfig::new(provider);
        if let Some(s) = suffix {
            config = config.with_volatile_system_suffix(s);
        }
        LlmRequest::new(messages, config, false).unwrap()
    }

    #[test]
    fn volatile_suffix_emits_two_system_blocks_marker_on_first_only() {
        let adapter = AnthropicAdapter::new();
        let req =
            anth_request_with_suffix("stable system", Some("## Temporal\n2026-06-11T14:00:00"));
        let body = adapter.build_request_body(&req).unwrap();

        let arr = body["system"].as_array().expect("system block array");
        assert_eq!(arr.len(), 2, "stable + volatile = 2 blocks");
        // Block 0: stable, carries the cache_control marker.
        assert_eq!(arr[0]["text"], "stable system");
        assert_eq!(arr[0]["cache_control"]["type"], "ephemeral");
        // Block 1: volatile suffix, NO marker (outside the cached prefix).
        assert_eq!(arr[1]["text"], "## Temporal\n2026-06-11T14:00:00");
        assert!(
            arr[1].get("cache_control").is_none(),
            "volatile suffix block must NOT carry cache_control"
        );
    }

    #[test]
    fn no_suffix_keeps_single_marked_system_block() {
        let adapter = AnthropicAdapter::new();
        let req = anth_request_with_suffix("stable system", None);
        let body = adapter.build_request_body(&req).unwrap();

        let arr = body["system"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "single block when no suffix");
        assert_eq!(arr[0]["cache_control"]["type"], "ephemeral");
    }
}
