use crate::llm::domain::{
    FunctionCall, LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmStreamChunk,
    LlmStreamPart, LlmUsage, ToolCall, ToolCallChunk,
};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::pin::Pin;
use std::task::{Context, Poll};

pub struct OpenAiAdapter {
    client: Client,
    base_url: String,
}

impl Default for OpenAiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAiAdapter {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: "https://api.openai.com/v1".to_string(),
        }
    }

    pub fn with_base_url(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    fn build_messages(&self, request: &LlmRequest) -> Vec<serde_json::Value> {
        request
            .messages()
            .iter()
            .map(|msg| {
                let mut message_json = json!({
                    "role": msg.role().as_str(),
                    "content": msg.content()
                });

                // Add tool_calls for assistant messages
                if let Some(tool_calls) = msg.tool_calls() {
                    let openai_tool_calls: Vec<serde_json::Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.function.name,
                                    "arguments": tc.function.arguments
                                }
                            })
                        })
                        .collect();
                    message_json["tool_calls"] = json!(openai_tool_calls);
                }

                // Add tool_call_id for tool messages
                if let Some(tool_call_id) = msg.tool_call_id() {
                    message_json["tool_call_id"] = json!(tool_call_id);
                }

                message_json
            })
            .collect()
    }

    fn build_request_body(&self, request: &LlmRequest) -> serde_json::Value {
        let mut body = json!({
            "model": request.config().model(),
            "messages": self.build_messages(request),
            "stream": request.stream()
        });

        if request.stream() {
            body["stream_options"] = json!({ "include_usage": true });
        }

        // Add tools if present (OpenAI format)
        if let Some(tools) = request.tools() {
            let openai_tools: Vec<serde_json::Value> = tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": {
                                "type": tool.parameters.schema_type,
                                "properties": tool.parameters.properties,
                                "required": tool.parameters.required
                            }
                        }
                    })
                })
                .collect();

            body["tools"] = json!(openai_tools);

            // Add tool_choice if specified
            if let Some(choice) = request.tool_choice() {
                body["tool_choice"] = json!(choice);
            }
        }

        if let Some(temp) = request.config().temperature() {
            body["temperature"] = json!(temp);
        }

        if let Some(max_tokens) = request.config().max_tokens() {
            body["max_completion_tokens"] = json!(max_tokens);
        }

        if let Some(top_p) = request.config().top_p() {
            body["top_p"] = json!(top_p);
        }

        if let Some(freq_penalty) = request.config().frequency_penalty() {
            body["frequency_penalty"] = json!(freq_penalty);
        }

        if let Some(pres_penalty) = request.config().presence_penalty() {
            body["presence_penalty"] = json!(pres_penalty);
        }

        body
    }
}

#[async_trait]
impl LlmRepository for OpenAiAdapter {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let body = self.build_request_body(&request);

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header(
                "Authorization",
                format!("Bearer {}", request.config().api_key()),
            )
            .header("Content-Type", "application/json")
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
                "OpenAI API error: {}",
                error_text
            )));
        }

        let openai_response: OpenAiResponse = response
            .json()
            .await
            .map_err(|e| LlmError::parsing_error(e.to_string()))?;

        // Extract tool calls if present
        let tool_calls = openai_response
            .choices
            .first()
            .and_then(|choice| choice.message.tool_calls.as_ref())
            .map(|calls| {
                calls
                    .iter()
                    .map(|tc| {
                        ToolCall::new(
                            tc.id.clone(),
                            FunctionCall::new(
                                tc.function.name.clone(),
                                tc.function.arguments.clone(),
                            ),
                        )
                    })
                    .collect::<Vec<_>>()
            });

        // Content might be None when there are tool calls
        let content = openai_response
            .choices
            .first()
            .and_then(|choice| choice.message.content.as_ref())
            .unwrap_or(&String::new())
            .clone();

        let usage = openai_response
            .usage
            .map(|u| LlmUsage::new(u.prompt_tokens, u.completion_tokens));

        let mut response = LlmResponse::new(
            request.id().clone(),
            content,
            request.config().provider().clone(),
        )?;

        if let Some(usage) = usage {
            response = response.with_usage(usage);
        }

        if let Some(finish_reason) = openai_response
            .choices
            .first()
            .and_then(|choice| choice.finish_reason.as_ref())
        {
            response = response.with_finish_reason(finish_reason.clone());
        }

        // Add tool calls if present
        if let Some(calls) = tool_calls {
            response = response.with_tool_calls(calls);
        }

        Ok(response)
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        let body = self.build_request_body(&request);

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header(
                "Authorization",
                format!("Bearer {}", request.config().api_key()),
            )
            .header("Content-Type", "application/json")
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
                "OpenAI API error: {}",
                error_text
            )));
        }

        let request_id = request.id().clone();
        let provider = request.config().provider().clone();

        let byte_stream = response.bytes_stream();
        let mut sse_parser = SseParser::new(byte_stream);
        let mut tool_ids_by_index = std::collections::HashMap::new();

        let sse_stream = async_stream::try_stream! {
            while let Some(event_result) = sse_parser.next().await {
                let event = event_result?;
                match event {
                    SseEvent::Message(data) => {
                        if data == "[DONE]" {
                            continue;
                        }
                        match serde_json::from_str::<OpenAiStreamChunk>(&data) {
                            Ok(chunk_response) => {
                                // 1. Check for Usage
                                if let Some(usage) = chunk_response.usage {
                                    yield LlmStreamChunk::new(
                                        request_id.clone(),
                                        LlmStreamPart::Usage(LlmUsage::new(
                                            usage.prompt_tokens,
                                            usage.completion_tokens,
                                        )),
                                        provider.clone(),
                                        false,
                                    );
                                    continue;
                                }

                                // 2. Check for content/tool_calls
                                if let Some(choice) = chunk_response.choices.first() {
                                    let is_final = choice.finish_reason.is_some();
                                    let finish_reason = choice.finish_reason.clone();

                                    if let Some(content) = &choice.delta.content {
                                        let mut chunk = LlmStreamChunk::new(
                                            request_id.clone(),
                                            LlmStreamPart::Content(content.clone()),
                                            provider.clone(),
                                            is_final,
                                        );
                                        if let Some(reason) = finish_reason {
                                            chunk = chunk.with_finish_reason(reason);
                                        }
                                        yield chunk;
                                    } else if let Some(tool_calls) = &choice.delta.tool_calls {
                                        if let Some(tc) = tool_calls.first() {
                                            // Register ID if provided
                                            if let Some(id) = &tc.id {
                                                tool_ids_by_index.insert(tc.index, id.clone());
                                            }

                                            // Retrieve ID from tracking
                                            let final_id = tc.id.clone()
                                                .or_else(|| tool_ids_by_index.get(&tc.index).cloned())
                                                .unwrap_or_default();

                                            let mut chunk = LlmStreamChunk::new(
                                                request_id.clone(),
                                                LlmStreamPart::ToolCallChunk(ToolCallChunk {
                                                    index: tc.index,
                                                    id: final_id,
                                                    name: tc.function.name.clone().unwrap_or_default(),
                                                    args_chunk: tc.function.arguments.clone().unwrap_or_default(),
                                                }),
                                                provider.clone(),
                                                is_final,
                                            );
                                            if let Some(reason) = finish_reason {
                                                chunk = chunk.with_finish_reason(reason);
                                            }
                                            yield chunk;
                                        }
                                    } else if is_final {
                                        let mut chunk = LlmStreamChunk::new(
                                            request_id.clone(),
                                            LlmStreamPart::Content(String::new()),
                                            provider.clone(),
                                            true,
                                        );
                                        if let Some(reason) = finish_reason {
                                            chunk = chunk.with_finish_reason(reason);
                                        }
                                        yield chunk;
                                    }
                                }
                            }
                            Err(e) => Err(LlmError::parsing_error(format!(
                                "Failed to parse stream chunk: {}",
                                e
                            )))?,
                        }
                    }
                }
            }
        };

        Ok(Box::pin(sse_stream))
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        let response = self
            .client
            .get(format!("{}/models", self.base_url))
            .send()
            .await
            .map_err(|e| LlmError::network_error(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(LlmError::request_failed("OpenAI health check failed"))
        }
    }

    fn provider_name(&self) -> &'static str {
        "openai"
    }
}

// Response structures for OpenAI API
#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCall {
    id: String,
    #[serde(rename = "type")]
    #[allow(dead_code)] // Required for deserialization, always "function" in OpenAI API
    call_type: String,
    function: OpenAiFunctionCall,
}

#[derive(Debug, Deserialize)]
struct OpenAiFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

// Streaming response structures
#[derive(Debug, Deserialize)]
struct OpenAiStreamChunk {
    choices: Vec<OpenAiStreamChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    delta: OpenAiDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiDelta {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiStreamToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamToolCall {
    #[allow(dead_code)]
    index: usize,
    id: Option<String>,
    #[allow(dead_code)]
    #[serde(rename = "type")]
    call_type: Option<String>,
    function: OpenAiStreamFunctionCall,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamFunctionCall {
    name: Option<String>,
    arguments: Option<String>,
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
