use crate::llm::domain::{
    LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmStreamChunk, LlmUsage,
};
use async_trait::async_trait;
use futures::{Stream, StreamExt, TryStreamExt};
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
                json!({
                    "role": msg.role().as_str(),
                    "content": msg.content()
                })
            })
            .collect()
    }

    fn build_request_body(&self, request: &LlmRequest) -> serde_json::Value {
        let mut body = json!({
            "model": request.config().model(),
            "messages": self.build_messages(request),
            "stream": request.stream()
        });

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

        println!("[OpenAI Adapter] Request body: {}", serde_json::to_string_pretty(&body).unwrap_or_else(|_| "Failed to serialize body".to_string()));

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
            .map_err(|e| {
                println!("[OpenAI Adapter Call] Network error on send: {}", e);
                LlmError::network_error(e.to_string())
            })?;

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

        let content = openai_response
            .choices
            .first()
            .and_then(|choice| choice.message.content.as_ref())
            .ok_or_else(|| LlmError::parsing_error("No content in response"))?;

        let usage = openai_response
            .usage
            .map(|u| LlmUsage::new(u.prompt_tokens, u.completion_tokens));

        let mut response = LlmResponse::new(
            request.id().clone(),
            content.clone(),
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

        Ok(response)
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        println!("[OpenAI Adapter Stream] Entering stream method.");
        let body = self.build_request_body(&request);

        let response = self
            .client
            .post(&format!("{}/chat/completions", self.base_url))
            .header(
                "Authorization",
                format!("Bearer {}", request.config().api_key()),
            )
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                println!("[OpenAI Adapter Stream] Network error on send: {}", e);
                LlmError::network_error(e.to_string())
            })?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            println!("[OpenAI Adapter Stream] Error response body: {}", &error_text);
            return Err(LlmError::request_failed(format!(
                "OpenAI API error: {}",
                error_text
            )));
        }

        let request_id = request.id().clone();
        let provider = request.config().provider().clone();

        let byte_stream = response.bytes_stream();

        let sse_stream = SseParser::new(byte_stream).try_filter_map(move |event| {
            let request_id = request_id.clone();
            let provider = provider.clone();
            let fut = async move {
                match event {
                    SseEvent::Message(data) => {
                        if data == "[DONE]" {
                            println!("[OpenAI Adapter Stream] Received [DONE] message.");
                            return Ok(None);
                        }
                        match serde_json::from_str::<OpenAiStreamChunk>(&data) {
                            Ok(chunk_response) => {
                                if let Some(choice) = chunk_response.choices.first() {
                                    let is_final = choice.finish_reason.is_some();
                                    if let Some(content) = &choice.delta.content {
                                        println!("[OpenAI Adapter Stream] Parsed content: '{}', is_final: {}", &content, is_final);
                                        Ok(Some(LlmStreamChunk::new(
                                            request_id,
                                            content.clone(),
                                            provider,
                                            is_final,
                                        )))
                                    } else if is_final {
                                        Ok(Some(LlmStreamChunk::new(
                                            request_id,
                                            String::new(),
                                            provider,
                                            true,
                                        )))
                                    } else {
                                        Ok(None)
                                    }
                                } else {
                                    Ok(None)
                                }
                            }
                            Err(e) => {
                                println!("[OpenAI Adapter Stream] JSON parsing error for data '{}': {}", data, e);
                                Err(LlmError::parsing_error(format!("Failed to parse stream chunk: {}", e)))
                            }
                        }
                    }
                }
            };
            fut
        });

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
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    delta: OpenAiDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiDelta {
    content: Option<String>,
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
                    if line.starts_with("data: ") {
                        let data = line[6..].to_string();
                        return Poll::Ready(Some(Ok(SseEvent::Message(data))));
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
