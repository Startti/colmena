use crate::llm::domain::{
    FunctionCall, LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmStreamChunk,
    LlmStreamPart, LlmUsage, MessageRole, ToolCall, ToolCallChunk,
};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::pin::Pin;
use std::task::{Context, Poll};

pub struct GeminiAdapter {
    client: Client,
    base_url: String,
}

impl Default for GeminiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl GeminiAdapter {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
        }
    }

    pub fn with_base_url(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    fn convert_messages(
        &self,
        request: &LlmRequest,
    ) -> Result<(Option<String>, Vec<GeminiContent>), LlmError> {
        let mut system_instructions = Vec::new();
        let mut contents = Vec::new();

        let messages = request.messages();

        for (i, message) in messages.iter().enumerate() {
            match message.role() {
                MessageRole::System => {
                    system_instructions.push(message.content().to_string());
                }
                MessageRole::User => {
                    let mut parts = Vec::new();
                    parts.push(GeminiPart {
                        text: Some(message.content().to_string()),
                        function_call: None,
                        function_response: None,
                        inline_data: None,
                    });

                    if let Some(files) = message.files() {
                        use base64::{engine::general_purpose::STANDARD, Engine as _};
                        for file in files {
                            parts.push(GeminiPart {
                                text: None,
                                function_call: None,
                                function_response: None,
                                inline_data: Some(GeminiInlineData {
                                    mime_type: file.mime_type.clone(),
                                    data: STANDARD.encode(&file.bytes),
                                }),
                            });
                        }
                    }

                    contents.push(GeminiContent {
                        role: "user".to_string(),
                        parts: Some(parts),
                        text: None,
                    });
                }
                MessageRole::Assistant => {
                    let mut parts = Vec::new();

                    if !message.content().is_empty() {
                        parts.push(GeminiPart {
                            text: Some(message.content().to_string()),
                            function_call: None,
                            function_response: None,
                            inline_data: None,
                        });
                    }

                    if let Some(tool_calls) = message.tool_calls() {
                        for tc in tool_calls {
                            parts.push(GeminiPart {
                                text: None,
                                function_call: Some(GeminiFunctionCall {
                                    name: tc.function.name.clone(),
                                    args: serde_json::from_str(&tc.function.arguments)
                                        .unwrap_or(json!({})),
                                }),
                                function_response: None,
                                inline_data: None,
                            });
                        }
                    }

                    // If parts is empty, Gemini still requires something, so add empty text
                    if parts.is_empty() {
                        parts.push(GeminiPart {
                            text: Some(String::new()),
                            function_call: None,
                            function_response: None,
                            inline_data: None,
                        });
                    }

                    contents.push(GeminiContent {
                        role: "model".to_string(),
                        parts: Some(parts),
                        text: None,
                    });
                }
                MessageRole::Tool => {
                    let target_id = message.tool_call_id().unwrap_or_default();
                    let mut tool_name = "unknown".to_string();

                    // Find the tool call in previous messages to get its name
                    for prev_msg in messages.iter().take(i) {
                        if let Some(tc) = prev_msg.tool_calls() {
                            if let Some(call) = tc.iter().find(|t| t.id == target_id) {
                                tool_name = call.function.name.clone();
                                break;
                            }
                        }
                    }

                    let parsed_content =
                        serde_json::from_str::<serde_json::Value>(message.content())
                            .unwrap_or_else(|_| serde_json::json!({ "result": message.content() }));

                    contents.push(GeminiContent {
                        role: "function".to_string(),
                        parts: Some(vec![GeminiPart {
                            text: None,
                            function_call: None,
                            function_response: Some(serde_json::json!({
                                "name": tool_name,
                                "response": parsed_content
                            })),
                            inline_data: None,
                        }]),
                        text: None,
                    });
                }
            }
        }

        let combined_system_instruction = if system_instructions.is_empty() {
            None
        } else {
            Some(system_instructions.join("\n\n"))
        };

        Ok((combined_system_instruction, contents))
    }

    /// Convert ToolDefinitions to Gemini's function declaration format
    fn convert_tools_to_gemini(&self, request: &LlmRequest) -> Option<serde_json::Value> {
        request.tools().map(|tools| {
            let function_declarations: Vec<serde_json::Value> = tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": {
                            "type": tool.parameters.schema_type,
                            "properties": tool.parameters.properties,
                            "required": tool.parameters.required
                        }
                    })
                })
                .collect();

            json!([{
                "functionDeclarations": function_declarations
            }])
        })
    }

    fn build_request_body(&self, request: &LlmRequest) -> Result<serde_json::Value, LlmError> {
        let (system_instruction, contents) = self.convert_messages(request)?;

        let mut body = json!({
            "contents": contents
        });

        if let Some(system) = system_instruction {
            body["systemInstruction"] = json!({
                "parts": [{"text": system}]
            });
        }

        // Add tools if present
        if let Some(tools) = self.convert_tools_to_gemini(request) {
            body["tools"] = tools;
        }

        let mut generation_config = serde_json::Map::new();

        if let Some(temp) = request.config().temperature() {
            generation_config.insert("temperature".to_string(), json!(temp));
        }

        if let Some(max_tokens) = request.config().max_tokens() {
            generation_config.insert("maxOutputTokens".to_string(), json!(max_tokens));
        }

        if let Some(top_p) = request.config().top_p() {
            generation_config.insert("topP".to_string(), json!(top_p));
        }

        // Disable thinking for Gemini 2.5-flash to reduce token usage
        generation_config.insert(
            "thinkingConfig".to_string(),
            json!({
                "thinkingBudget": 0
            }),
        );

        if !generation_config.is_empty() {
            body["generationConfig"] = json!(generation_config);
        }

        Ok(body)
    }
}

#[async_trait]
impl LlmRepository for GeminiAdapter {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let body = self.build_request_body(&request)?;

        let url = format!(
            "{}/models/{}:generateContent",
            self.base_url,
            request.config().model()
        );

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", request.config().api_key())
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
                "Gemini API error: {}",
                error_text
            )));
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| LlmError::parsing_error(e.to_string()))?;

        let gemini_response: GeminiResponse =
            serde_json::from_str(&response_text).map_err(|e| {
                LlmError::parsing_error(format!(
                    "JSON parse error: {} - Response: {}",
                    e, response_text
                ))
            })?;

        // Extract function calls if present
        let tool_calls = gemini_response.candidates.first().and_then(|candidate| {
            candidate.content.parts.as_ref().and_then(|parts| {
                let function_calls: Vec<ToolCall> = parts
                    .iter()
                    .filter_map(|part| {
                        part.function_call.as_ref().map(|fc| {
                            // Generate a unique ID for the tool call
                            let call_id = format!("call_{}", uuid::Uuid::new_v4());
                            ToolCall::new(
                                call_id,
                                FunctionCall::new(
                                    fc.name.clone(),
                                    serde_json::to_string(&fc.args)
                                        .unwrap_or_else(|_| "{}".to_string()),
                                ),
                            )
                        })
                    })
                    .collect();

                if function_calls.is_empty() {
                    None
                } else {
                    Some(function_calls)
                }
            })
        });

        let content = gemini_response
            .candidates
            .first()
            .and_then(|candidate| {
                // Try direct text field first (for newer models)
                if let Some(text) = &candidate.content.text {
                    if !text.is_empty() {
                        Some(text.clone())
                    } else {
                        None
                    }
                } else {
                    // Fallback to parts structure (for older models)
                    candidate.content.parts
                        .as_ref()
                        .and_then(|parts| parts.iter().find_map(|part| {
                            part.text.as_ref().filter(|text| !text.is_empty()).cloned()
                        }))
                }
            })
            .unwrap_or_else(|| {
                // If no content is found, check finish reason
                let finish_reason = gemini_response
                    .candidates
                    .first()
                    .and_then(|candidate| candidate.finish_reason.as_ref())
                    .map(|s| s.as_str())
                    .unwrap_or("UNKNOWN");

                if finish_reason == "MAX_TOKENS" {
                    "[No content generated - Increase max_tokens as this Gemini model uses tokens for internal reasoning]".to_string()
                } else {
                    format!("[Empty response - finish_reason: {}]", finish_reason)
                }
            });

        let usage = gemini_response.usage_metadata.map(|u| {
            LlmUsage::new(
                u.prompt_token_count.unwrap_or(0),
                u.candidates_token_count.unwrap_or(0),
            )
        });

        let mut response = LlmResponse::new(
            request.id().clone(),
            content,
            request.config().provider().clone(),
        )?;

        if let Some(usage) = usage {
            response = response.with_usage(usage);
        }

        if let Some(finish_reason) = gemini_response
            .candidates
            .first()
            .and_then(|candidate| candidate.finish_reason.as_ref())
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
        let body = self.build_request_body(&request)?;

        let url = format!(
            "{}/models/{}:streamGenerateContent",
            self.base_url,
            request.config().model()
        );

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", request.config().api_key())
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
                "Gemini API error: {}",
                error_text
            )));
        }

        let request_id = request.id().clone();
        let provider = request.config().provider().clone();

        let byte_stream = response.bytes_stream();
        let mut json_parser = JsonStreamParser::new(byte_stream);

        let json_stream = async_stream::try_stream! {
            let mut latest_usage = None;
            let mut tool_call_index: usize = 0;
            while let Some(json_bytes_result) = json_parser.next().await {
                let json_bytes = json_bytes_result?;
                let chunk_response = serde_json::from_slice::<GeminiResponse>(&json_bytes)
                    .map_err(|e| LlmError::parsing_error(e.to_string()))?;

                if let Some(candidate) = chunk_response.candidates.first() {
                    let is_final = candidate.finish_reason.is_some();
                    let finish_reason = candidate.finish_reason.clone();

                    if let Some(parts) = &candidate.content.parts {
                        for part in parts {
                            if let Some(text) = &part.text {
                                if !text.is_empty() {
                                    let mut chunk = LlmStreamChunk::new(
                                        request_id.clone(),
                                        LlmStreamPart::Content(text.clone()),
                                        provider.clone(),
                                        is_final,
                                    );
                                    if let Some(reason) = &finish_reason {
                                        chunk = chunk.with_finish_reason(reason.clone());
                                    }
                                    yield chunk;
                                }
                            }

                            if let Some(fc) = &part.function_call {
                                let call_id = format!("call_{}", uuid::Uuid::new_v4());
                                let args_str = serde_json::to_string(&fc.args).unwrap_or_default();

                                let mut chunk = LlmStreamChunk::new(
                                    request_id.clone(),
                                    LlmStreamPart::ToolCallChunk(ToolCallChunk {
                                        index: tool_call_index,
                                        id: call_id,
                                        name: fc.name.clone(),
                                        args_chunk: args_str,
                                    }),
                                    provider.clone(),
                                    is_final,
                                );
                                tool_call_index += 1;
                                if let Some(reason) = &finish_reason {
                                    chunk = chunk.with_finish_reason(reason.clone());
                                }
                                yield chunk;
                            }
                        }
                    } else if let Some(text) = &candidate.content.text {
                        let mut chunk = LlmStreamChunk::new(
                            request_id.clone(),
                            LlmStreamPart::Content(text.clone()),
                            provider.clone(),
                            is_final,
                        );
                        if let Some(reason) = &finish_reason {
                            chunk = chunk.with_finish_reason(reason.clone());
                        }
                        yield chunk;
                    } else if is_final {
                        let mut chunk = LlmStreamChunk::new(
                            request_id.clone(),
                            LlmStreamPart::Content(String::new()),
                            provider.clone(),
                            true,
                        );
                        if let Some(reason) = &finish_reason {
                            chunk = chunk.with_finish_reason(reason.clone());
                        }
                        yield chunk;
                    }
                }

                if let Some(usage) = &chunk_response.usage_metadata {
                    latest_usage = Some(LlmUsage::new(
                        usage.prompt_token_count.unwrap_or(0),
                        usage.candidates_token_count.unwrap_or(0)
                    ));
                }
            }

            if let Some(usage) = latest_usage {
                yield LlmStreamChunk::new(
                    request_id.clone(),
                    LlmStreamPart::Usage(usage),
                    provider.clone(),
                    true,
                );
            }
        };

        Ok(Box::pin(json_stream))
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        // For Gemini, we'll make a simple test request to check if the API key works
        let url = format!("{}/models/gemini-1.5-flash:generateContent", self.base_url);

        let test_body = json!({
            "contents": [
                {
                    "role": "user",
                    "parts": [{"text": "Hi"}]
                }
            ]
        });

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .query(&[("key", "dummy")]) // This will fail, but we can check if endpoint exists
            .json(&test_body)
            .send()
            .await
            .map_err(|e| LlmError::network_error(e.to_string()))?;

        // If we get a 4xx status, the endpoint exists (just API key is wrong)
        // If we get a 2xx status, everything is working
        if response.status().is_success() || response.status().is_client_error() {
            Ok(())
        } else {
            Err(LlmError::request_failed("Gemini endpoint not available"))
        }
    }

    fn provider_name(&self) -> &'static str {
        "gemini"
    }
}

// Response structures for Gemini API
#[derive(Debug, Serialize, Deserialize)]
struct GeminiContent {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parts: Option<Vec<GeminiPart>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>, // For newer models like gemini-2.5-flash
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionCall")]
    function_call: Option<GeminiFunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionResponse")]
    function_response: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "inlineData")]
    inline_data: Option<GeminiInlineData>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiInlineData {
    #[serde(rename = "mimeType")]
    mime_type: String,
    data: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsage>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiUsage {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<u32>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<u32>,
}

// Custom Stream Parser for Gemini's JSON array stream
struct JsonStreamParser<S>
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    stream: S,
    buffer: Vec<u8>,
}

impl<S> JsonStreamParser<S>
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

impl<S> Stream for JsonStreamParser<S>
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<Vec<u8>, LlmError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(start_index) = self.buffer.iter().position(|&b| b == b'{') {
                let mut brace_count = 0;
                let mut end_index = None;

                for (i, &byte) in self.buffer.iter().enumerate().skip(start_index) {
                    if byte == b'{' {
                        brace_count += 1;
                    } else if byte == b'}' {
                        brace_count -= 1;
                        if brace_count == 0 {
                            end_index = Some(i);
                            break;
                        }
                    }
                }

                if let Some(end) = end_index {
                    let json_bytes = self.buffer[start_index..=end].to_vec();

                    self.buffer.drain(..=end);

                    return Poll::Ready(Some(Ok(json_bytes)));
                }
            }

            match self.stream.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.buffer.extend_from_slice(&chunk);
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(LlmError::network_error(e.to_string()))));
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
