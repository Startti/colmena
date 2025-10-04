use crate::llm::domain::{
    LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmStreamChunk, LlmUsage,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

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

        println!(
            "[OpenAI Adapter] Request body: {}",
            serde_json::to_string_pretty(&body)
                .unwrap_or_else(|_| "Failed to serialize body".to_string())
        );

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
                println!("[OpenAI Adapter Stream] Network error on send: {}", e);
                LlmError::network_error(e.to_string())
            })?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            println!(
                "[OpenAI Adapter Stream] Error response body: {}",
                &error_text
            );
            return Err(LlmError::request_failed(format!(
                "OpenAI API error: {}",
                error_text
            )));
        }

        let request_id = request.id().clone();
        let provider = request.config().provider().clone();

        let bytes = response.bytes().await.map_err(|e| {
            println!("[OpenAI Adapter Stream] Error reading bytes: {}", e);
            LlmError::network_error(e.to_string())
        })?;
        let text = String::from_utf8_lossy(&bytes);
        println!("[OpenAI Adapter Stream] Full response text: {:?}", text);

        let mut results = Vec::new();
        for line in text.lines() {
            if let Some(stripped) = line.strip_prefix("data: ") {
                let data = stripped.trim();
                if data == "[DONE]" {
                    println!("[OpenAI Adapter Stream] Received [DONE] message.");
                    break;
                }

                match serde_json::from_str::<OpenAiStreamChunk>(data) {
                    Ok(chunk_response) => {
                        if let Some(choice) = chunk_response.choices.first() {
                            let is_final = choice.finish_reason.is_some();
                            if let Some(content) = &choice.delta.content {
                                println!(
                                    "[OpenAI Adapter Stream] Parsed content: '{}', is_final: {}",
                                    &content, is_final
                                );
                                results.push(Ok(LlmStreamChunk::new(
                                    request_id.clone(),
                                    content.clone(),
                                    provider.clone(),
                                    is_final,
                                )));
                            } else if is_final {
                                // Handle final chunk with no content
                                results.push(Ok(LlmStreamChunk::new(
                                    request_id.clone(),
                                    String::new(),
                                    provider.clone(),
                                    true,
                                )));
                            }
                        }
                    }
                    Err(e) => {
                        println!(
                            "[OpenAI Adapter Stream] JSON parsing error for data '{}': {}",
                            data, e
                        );
                    }
                }
            }
        }

        Ok(Box::pin(futures::stream::iter(results)))
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
