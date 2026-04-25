use crate::llm::domain::{
    FunctionCall, LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmStreamChunk,
    LlmStreamPart, LlmUsage, MessageRole, ToolCall, ToolCallChunk,
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

    fn convert_messages(&self, request: &LlmRequest) -> (Option<String>, Vec<AnthropicMessage>) {
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
                            if file.mime_type.starts_with("image/") {
                                blocks.push(AnthropicContentBlock::Image {
                                    source: AnthropicMediaSource {
                                        source_type: "base64".to_string(),
                                        media_type: file.mime_type.clone(),
                                        data: STANDARD.encode(&file.bytes),
                                    },
                                });
                            } else if file.mime_type == "application/pdf" {
                                blocks.push(AnthropicContentBlock::Document {
                                    source: AnthropicMediaSource {
                                        source_type: "base64".to_string(),
                                        media_type: file.mime_type.clone(),
                                        data: STANDARD.encode(&file.bytes),
                                    },
                                });
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

        (system_message, messages)
    }

    fn build_request_body(&self, request: &LlmRequest) -> serde_json::Value {
        let (system_message, messages) = self.convert_messages(request);

        let mut body = json!({
            "model": request.config().model(),
            "messages": messages,
            "stream": request.stream()
        });

        if let Some(system) = system_message {
            body["system"] = json!(system);
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

        if let Some(tools) = request.tools() {
            let anthropic_tools: Vec<serde_json::Value> = tools
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
                body["tools"] = json!(anthropic_tools);
            }
        }

        body
    }
}

#[async_trait]
impl LlmRepository for AnthropicAdapter {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let body = self.build_request_body(&request);

        let response = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", request.config().api_key())
            .header("Content-Type", "application/json")
            .header("anthropic-version", "2023-06-01")
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
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in anthropic_response.content {
            match block {
                AnthropicResponseBlock::Text { text } => text_parts.push(text),
                AnthropicResponseBlock::ToolUse { id, name, input } => {
                    let arguments = serde_json::to_string(&input)
                        .map_err(|e| LlmError::parsing_error(e.to_string()))?;
                    tool_calls.push(ToolCall::new(id, FunctionCall::new(name, arguments)));
                }
                AnthropicResponseBlock::Other => {}
            }
        }

        let content = text_parts.join("");
        let usage = LlmUsage::new(
            anthropic_response.usage.input_tokens,
            anthropic_response.usage.output_tokens,
        );

        let mut response = LlmResponse::new(
            request.id().clone(),
            content,
            request.config().provider().clone(),
        )?
        .with_usage(usage);

        if !tool_calls.is_empty() {
            response = response.with_tool_calls(tool_calls);
        }

        if let Some(stop_reason) = anthropic_response.stop_reason {
            response = response.with_finish_reason(stop_reason);
        }

        Ok(response)
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        let body = self.build_request_body(&request);

        let response = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", request.config().api_key())
            .header("Content-Type", "application/json")
            .header("anthropic-version", "2023-06-01")
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
        let mut stop_reason: Option<String> = None;

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
                            if let AnthropicStreamBlock::ToolUse { id, name, .. } = block {
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
                            if let Some((id, name)) = tool_state.get(&idx) {
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
                    "message_delta" => {
                        if let Some(delta) = parsed.delta {
                            if let Some(reason) = delta.stop_reason {
                                stop_reason = Some(reason);
                            }
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

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMediaSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: String,
    data: String,
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
}

// Streaming response structures
#[derive(Debug, Deserialize)]
struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    index: Option<usize>,
    content_block: Option<AnthropicStreamBlock>,
    delta: Option<AnthropicStreamDelta>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicStreamBlock {
    Text {
        #[allow(dead_code)]
        text: Option<String>,
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
