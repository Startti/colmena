use crate::llm::domain::{
    LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmStreamChunk, LlmStreamPart,
    LlmUsage, MessageRole,
};
use async_trait::async_trait;
use futures::{Stream, StreamExt, TryStreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
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
                    messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: message.content().to_string(),
                    });
                }
                MessageRole::Assistant => {
                    messages.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content: message.content().to_string(),
                    });
                }
                MessageRole::Tool => {
                    // Anthropic uses a specific format for tool results (user role with tool_result content block)
                    // For now, we'll treat it as a user message with the content
                    // TODO: Implement proper tool result formatting for Anthropic
                    messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: format!("Tool result: {}", message.content()),
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

        if let Some(max_tokens) = request.config().max_tokens() {
            body["max_tokens"] = json!(max_tokens);
        }

        if let Some(top_p) = request.config().top_p() {
            body["top_p"] = json!(top_p);
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

        let content = anthropic_response
            .content
            .first()
            .map(|content| content.text.clone())
            .ok_or_else(|| LlmError::parsing_error("No content in response"))?;

        let usage = LlmUsage::new(
            anthropic_response.usage.input_tokens,
            anthropic_response.usage.output_tokens,
        );

        let mut response = LlmResponse::new(
            request.id().clone(),
            content,
            request.config().provider().clone(),
        )?;

        response = response.with_usage(usage);

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
        let sse_stream = SseParser::new(byte_stream).try_filter_map(move |event| {
            let request_id = request_id.clone();
            let provider = provider.clone();
            async move {
                match event {
                    SseEvent::Message(data) => {
                        if let Ok(event) = serde_json::from_str::<AnthropicStreamEvent>(&data) {
                            match event.event_type.as_str() {
                                "content_block_delta" => {
                                    if let Some(delta) = event.delta {
                                        if let Some(text) = delta.text {
                                            return Ok(Some(LlmStreamChunk::new(
                                                request_id,
                                                LlmStreamPart::Content(text),
                                                provider,
                                                false,
                                            )));
                                        }
                                    }
                                }
                                "message_stop" => {
                                    return Ok(Some(LlmStreamChunk::new(
                                        request_id,
                                        LlmStreamPart::Content(String::new()),
                                        provider,
                                        true,
                                    )));
                                }
                                _ => {}
                            }
                        }
                        Ok(None)
                    }
                }
            }
        });

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

// Response structures for Anthropic API
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
    usage: AnthropicUsage,
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContent {
    text: String,
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
    delta: Option<AnthropicDelta>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDelta {
    text: Option<String>,
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
