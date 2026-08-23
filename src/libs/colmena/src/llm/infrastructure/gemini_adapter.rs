use crate::llm::domain::{
    FileSource, FunctionCall, LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream,
    LlmStreamChunk, LlmStreamPart, LlmUsage, MessageRole, ToolCall, ToolCallChunk,
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

    /// The configured endpoint. Exposed for tests and diagnostics.
    pub fn base_url(&self) -> &str {
        &self.base_url
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
                        file_data: None,
                        thought: None,
                        thought_signature: None,
                    });

                    if let Some(files) = message.files() {
                        use base64::{engine::general_purpose::STANDARD, Engine as _};
                        for file in files {
                            let part = match &file.source {
                                FileSource::InlineBytes { bytes } => GeminiPart {
                                    text: None,
                                    function_call: None,
                                    function_response: None,
                                    inline_data: Some(GeminiInlineData {
                                        mime_type: file.mime_type.clone(),
                                        data: STANDARD.encode(bytes),
                                    }),
                                    file_data: None,
                                    thought: None,
                                    thought_signature: None,
                                },
                                FileSource::Uploaded(r) => GeminiPart {
                                    text: None,
                                    function_call: None,
                                    function_response: None,
                                    inline_data: None,
                                    file_data: Some(GeminiFileData {
                                        mime_type: r.mime_type.clone(),
                                        file_uri: r.provider_file_id.clone(),
                                    }),
                                    thought: None,
                                    thought_signature: None,
                                },
                                FileSource::SignedUrl(_) => {
                                    return Err(LlmError::InternalError {
                                        message: format!(
                                            "Gemini adapter received unresolved SignedUrl for '{}'. \
                                             LlmCallUseCase::resolve_files must run before reaching the adapter.",
                                            file.filename
                                        ),
                                    });
                                }
                            };
                            parts.push(part);
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
                            file_data: None,
                            thought: None,
                            thought_signature: None,
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
                                file_data: None,
                                thought: None,
                                // Replay the thinking-model signature verbatim, or
                                // Gemini rejects the request with HTTP 400.
                                thought_signature: tc.provider_signature.clone(),
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
                            file_data: None,
                            thought: None,
                            thought_signature: None,
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

                    // Gemini's `functionResponse.response` is typed as
                    // `google.protobuf.Struct` and only accepts JSON objects.
                    // Wrap non-object values (scalars, arrays, null) in
                    // `{ "result": <value> }`. Objects pass through unchanged
                    // so callers that already return dicts keep their keys.
                    // Non-JSON content (free-form error strings) is wrapped
                    // as a string under the same key.
                    //
                    // See: docs/superpowers/plans/2026-06-01-gemini-scalar-tool-response-fix.md
                    let parsed_content =
                        match serde_json::from_str::<serde_json::Value>(message.content()) {
                            Ok(v) if v.is_object() => v,
                            Ok(v) => serde_json::json!({ "result": v }),
                            Err(_) => serde_json::json!({ "result": message.content() }),
                        };

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
                            file_data: None,
                            thought: None,
                            thought_signature: None,
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

    /// Convert ToolDefinitions to Gemini's function declaration format.
    /// When a tool has no parameters, omit `parameters` entirely — Gemini
    /// silently fails across the whole tool set if any function declares
    /// `parameters: { type: object, properties: {}, required: [] }`.
    fn convert_tools_to_gemini(&self, request: &LlmRequest) -> Option<serde_json::Value> {
        request.tools().map(|tools| {
            let function_declarations: Vec<serde_json::Value> = tools
                .iter()
                .map(|tool| {
                    let mut decl = serde_json::Map::new();
                    decl.insert("name".to_string(), json!(tool.name));
                    decl.insert("description".to_string(), json!(tool.description));
                    if let Some(override_schema) = tool.input_schema_override.as_ref() {
                        decl.insert("parameters".to_string(), override_schema.clone());
                    } else if !tool.parameters.properties.is_empty() {
                        decl.insert(
                            "parameters".to_string(),
                            json!({
                                "type": tool.parameters.schema_type,
                                "properties": tool.parameters.properties,
                                "required": tool.parameters.required
                            }),
                        );
                    }
                    serde_json::Value::Object(decl)
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

        // Cache-safe temporal suffix (2026-06-11). Appended to the END of the
        // systemInstruction so Gemini's implicit prefix cache still matches the
        // stable prefix while the timestamp changes per turn. If there is no
        // stable system instruction, the suffix becomes the whole instruction.
        let volatile_suffix = request.config().volatile_system_suffix();
        let final_system: Option<String> = match (system_instruction, volatile_suffix) {
            (Some(system), Some(suffix)) => Some(format!("{system}\n\n{suffix}")),
            (Some(system), None) => Some(system),
            (None, Some(suffix)) => Some(suffix.to_string()),
            (None, None) => None,
        };
        if let Some(system) = final_system {
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

        if let Some(budget) = request.config().thinking_budget() {
            // Explicit budget requested — enable thoughts and surface them.
            generation_config.insert(
                "thinkingConfig".to_string(),
                json!({
                    "thinkingBudget": budget,
                    "includeThoughts": true
                }),
            );
        }
        // When no explicit budget is set, omit thinkingConfig entirely so Gemini uses its
        // model default (8 000 tokens for gemini-2.5-flash, dynamic for 2.5-pro).
        // Previously we set thinkingBudget: 0 for tool-less requests, but the Gemini
        // recommendation is to let the model decide.

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
            candidate
                .content
                .as_ref()?
                .parts
                .as_ref()
                .and_then(|parts| {
                    let function_calls: Vec<ToolCall> = parts
                        .iter()
                        .filter_map(|part| {
                            part.function_call.as_ref().map(|fc| {
                                // Generate a unique ID for the tool call
                                let call_id = format!("call_{}", uuid::Uuid::new_v4());
                                let mut call = ToolCall::new(
                                    call_id,
                                    FunctionCall::new(
                                        fc.name.clone(),
                                        super::tool_args::serialize_tool_args(&fc.args, &fc.name),
                                    ),
                                );
                                // Carry the thinking-model signature so it can be
                                // replayed when this call is sent back in history.
                                call.provider_signature = part.thought_signature.clone();
                                call
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

        // Separate thought parts from regular text parts.
        let thinking_content: Option<String> = gemini_response
            .candidates
            .first()
            .and_then(|c| c.content.as_ref())
            .and_then(|c| c.parts.as_ref())
            .map(|parts| {
                parts
                    .iter()
                    .filter(|p| p.thought == Some(true))
                    .filter_map(|p| p.text.as_deref())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .filter(|s| !s.is_empty());

        let content = gemini_response
            .candidates
            .first()
            .and_then(|candidate| {
                let content = candidate.content.as_ref()?;
                // Try direct text field first (for newer models)
                if let Some(text) = &content.text {
                    if !text.is_empty() {
                        Some(text.clone())
                    } else {
                        None
                    }
                } else {
                    // Fallback to parts — exclude thought parts
                    content.parts
                        .as_ref()
                        .and_then(|parts| {
                            let joined: String = parts
                                .iter()
                                .filter(|p| p.thought != Some(true))
                                .filter_map(|p| p.text.as_deref().filter(|t| !t.is_empty()))
                                .collect::<Vec<_>>()
                                .join("");
                            if joined.is_empty() { None } else { Some(joined) }
                        })
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
            let mut usage = LlmUsage::new(
                u.prompt_token_count.unwrap_or(0),
                u.candidates_token_count.unwrap_or(0),
            );
            if let Some(t) = u.thoughts_token_count.filter(|&n| n > 0) {
                usage = usage.with_thinking_tokens(t);
            }
            if let Some(c) = u.cached_content_token_count.filter(|&n| n > 0) {
                usage = usage.with_cached_input_tokens_included(c);
            }
            usage
        });

        let mut response = LlmResponse::new(
            request.id().clone(),
            content,
            request.config().provider().clone(),
        )?;

        if let Some(usage) = usage {
            response = response.with_usage(usage);
        }

        if let Some(thinking) = thinking_content {
            response = response.with_thinking_content(thinking);
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
            // Track whether we are currently inside a thinking block across chunks.
            let mut in_thinking = false;
            while let Some(json_bytes_result) = json_parser.next().await {
                let json_bytes = json_bytes_result?;
                let chunk_response = serde_json::from_slice::<GeminiResponse>(&json_bytes)
                    .map_err(|e| LlmError::parsing_error(e.to_string()))?;

                if let Some(candidate) = chunk_response.candidates.first() {
                    let is_final = candidate.finish_reason.is_some();
                    let finish_reason = candidate.finish_reason.clone();

                    let candidate_content = candidate.content.as_ref();
                    if let Some(parts) = candidate_content.and_then(|c| c.parts.as_ref()) {
                        for part in parts {
                            let is_thought = part.thought == Some(true);

                            if let Some(text) = &part.text {
                                if !text.is_empty() {
                                    if is_thought {
                                        if !in_thinking {
                                            in_thinking = true;
                                            yield LlmStreamChunk::new(
                                                request_id.clone(),
                                                LlmStreamPart::ThinkingStart,
                                                provider.clone(),
                                                false,
                                            );
                                        }
                                        yield LlmStreamChunk::new(
                                            request_id.clone(),
                                            LlmStreamPart::ThinkingContent(text.clone()),
                                            provider.clone(),
                                            false,
                                        );
                                    } else {
                                        if in_thinking {
                                            in_thinking = false;
                                            yield LlmStreamChunk::new(
                                                request_id.clone(),
                                                LlmStreamPart::ThinkingEnd,
                                                provider.clone(),
                                                false,
                                            );
                                        }
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
                            }

                            if let Some(fc) = &part.function_call {
                                let call_id = format!("call_{}", uuid::Uuid::new_v4());
                                let args_str =
                                    super::tool_args::serialize_tool_args(&fc.args, &fc.name);

                                let mut chunk = LlmStreamChunk::new(
                                    request_id.clone(),
                                    LlmStreamPart::ToolCallChunk(ToolCallChunk {
                                        index: tool_call_index,
                                        id: call_id,
                                        name: fc.name.clone(),
                                        args_chunk: args_str,
                                        provider_signature: part.thought_signature.clone(),
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
                    } else if let Some(text) = candidate_content.and_then(|c| c.text.as_ref()) {
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
                        // Close any open thinking block before the final content marker.
                        if in_thinking {
                            in_thinking = false;
                            yield LlmStreamChunk::new(
                                request_id.clone(),
                                LlmStreamPart::ThinkingEnd,
                                provider.clone(),
                                false,
                            );
                        }
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

                if let Some(u) = &chunk_response.usage_metadata {
                    let mut usage = LlmUsage::new(
                        u.prompt_token_count.unwrap_or(0),
                        u.candidates_token_count.unwrap_or(0),
                    );
                    if let Some(t) = u.thoughts_token_count.filter(|&n| n > 0) {
                        usage = usage.with_thinking_tokens(t);
                    }
                    if let Some(c) = u.cached_content_token_count.filter(|&n| n > 0) {
                        usage = usage.with_cached_input_tokens_included(c);
                    }
                    latest_usage = Some(usage);
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
        // Reachability-only diagnostic — model-free, no request body, no pinned
        // model string. Mirrors the OpenAI adapter's `GET /models` shape.
        let response = self
            .client
            .get(format!("{}/models", self.base_url))
            .send()
            .await
            .map_err(|e| LlmError::network_error(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(LlmError::request_failed("Gemini health check failed"))
        }
    }

    async fn validate_credentials(&self, api_key: &str) -> Result<(), LlmError> {
        let response = self
            .client
            .get(format!("{}/models", self.base_url))
            .query(&[("key", api_key)])
            .send()
            .await
            .map_err(|e| LlmError::network_error(e.to_string()))?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else if status.as_u16() == 401 || status.as_u16() == 403 {
            Err(LlmError::InvalidApiKey)
        } else {
            Err(LlmError::request_failed(format!(
                "Gemini credential validation failed with status {}",
                status
            )))
        }
    }

    fn provider_name(&self) -> &'static str {
        "google"
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
    #[serde(skip_serializing_if = "Option::is_none", rename = "fileData")]
    file_data: Option<GeminiFileData>,
    /// Present and `true` on thought/reasoning parts from Gemini 2.5+ models.
    #[serde(skip_serializing_if = "Option::is_none")]
    thought: Option<bool>,
    /// Opaque signature attached to function-call (and thought) parts by Gemini
    /// thinking models. MUST be echoed back verbatim when the part is replayed
    /// in the conversation, or the API rejects the request with HTTP 400
    /// ("Function call is missing a thought_signature").
    #[serde(skip_serializing_if = "Option::is_none", rename = "thoughtSignature")]
    thought_signature: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiInlineData {
    #[serde(rename = "mimeType")]
    mime_type: String,
    data: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiFileData {
    #[serde(rename = "mimeType")]
    mime_type: String,
    #[serde(rename = "fileUri")]
    file_uri: String,
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
    content: Option<GeminiContent>,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiUsage {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<u32>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<u32>,
    /// Thinking tokens reported separately by Gemini 2.5 models.
    #[serde(rename = "thoughtsTokenCount")]
    thoughts_token_count: Option<u32>,
    /// Tokens served from Gemini's implicit prompt cache (Gemini 2.5+ models).
    /// Implicit caching is automatic server-side — no markers needed in the
    /// request. Field present in `usageMetadata` only when the call hit the
    /// cache (minimum prefix: 1024 tokens for 2.5-flash, 2048 for 2.5-pro).
    /// Surfaced via `LlmUsage::cache_read_tokens` for cost-tracking parity
    /// with OpenAI's `cached_tokens` and Anthropic's `cache_read_input_tokens`.
    #[serde(rename = "cachedContentTokenCount")]
    cached_content_token_count: Option<u32>,
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
                let mut in_string = false;
                let mut escape_next = false;

                for (i, &byte) in self.buffer.iter().enumerate().skip(start_index) {
                    if escape_next {
                        escape_next = false;
                        continue;
                    }
                    if byte == b'\\' && in_string {
                        escape_next = true;
                        continue;
                    }
                    if byte == b'"' {
                        in_string = !in_string;
                        continue;
                    }
                    if in_string {
                        continue;
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_body_serializes_uploaded_pdf_as_file_data() {
        use crate::llm::domain::{
            FileData, FileSource, LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderFileRef,
            ProviderKind,
        };
        let file = FileData {
            document_id: Some("doc-1".into()),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: None,
            source: FileSource::Uploaded(ProviderFileRef {
                provider: ProviderKind::Google,
                provider_file_id: "https://generativelanguage.googleapis.com/v1beta/files/abc"
                    .into(),
                mime_type: "application/pdf".into(),
                filename: "x.pdf".into(),
                expires_at: None,
            }),
            retained_inline_bytes: None,
        };
        let msg = LlmMessage::user_with_files("describe".into(), vec![file]).unwrap();
        let provider = LlmProvider::new(
            ProviderKind::Google,
            "k".into(),
            Some("gemini-2.5-pro".into()),
        )
        .unwrap();
        let config = LlmConfig::new(provider);
        let request = LlmRequest::new(vec![msg], config, false).unwrap();

        let adapter = GeminiAdapter::new();
        let body = adapter.build_request_body(&request).unwrap();
        let parts = body["contents"][0]["parts"].as_array().unwrap();
        let file_part = parts.iter().find(|p| p.get("fileData").is_some()).unwrap();
        assert_eq!(file_part["fileData"]["mimeType"], "application/pdf");
        assert_eq!(
            file_part["fileData"]["fileUri"],
            "https://generativelanguage.googleapis.com/v1beta/files/abc"
        );
    }

    #[test]
    fn build_request_body_returns_error_on_signed_url() {
        use crate::llm::domain::{
            FileData, FileSource, LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderKind,
        };
        let file = FileData {
            document_id: Some("doc-1".into()),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: None,
            source: FileSource::SignedUrl("https://example/x?sig=y".into()),
            retained_inline_bytes: None,
        };
        let msg = LlmMessage::user_with_files("describe".into(), vec![file]).unwrap();
        let provider = LlmProvider::new(
            ProviderKind::Google,
            "k".into(),
            Some("gemini-2.5-pro".into()),
        )
        .unwrap();
        let config = LlmConfig::new(provider);
        let request = LlmRequest::new(vec![msg], config, false).unwrap();

        let adapter = GeminiAdapter::new();
        let err = adapter.build_request_body(&request).unwrap_err();
        assert!(matches!(
            err,
            crate::llm::domain::LlmError::InternalError { .. }
        ));
    }

    #[test]
    fn build_request_body_serializes_inline_pdf_as_inline_data() {
        use crate::llm::domain::{
            FileData, LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderKind,
        };
        let file = FileData::inline("application/pdf".into(), "x.pdf".into(), b"PDF".to_vec());
        let msg = LlmMessage::user_with_files("describe".into(), vec![file]).unwrap();
        let provider = LlmProvider::new(
            ProviderKind::Google,
            "k".into(),
            Some("gemini-2.5-pro".into()),
        )
        .unwrap();
        let config = LlmConfig::new(provider);
        let request = LlmRequest::new(vec![msg], config, false).unwrap();

        let adapter = GeminiAdapter::new();
        let body = adapter.build_request_body(&request).unwrap();
        let parts = body["contents"][0]["parts"].as_array().unwrap();
        let file_part = parts
            .iter()
            .find(|p| p.get("inlineData").is_some())
            .unwrap();
        assert_eq!(file_part["inlineData"]["mimeType"], "application/pdf");
        assert!(file_part["inlineData"]["data"].is_string());
    }

    // ----------------------------------------------------------------------
    // Regression tests for the "scalar tool response" bug.
    //
    // Gemini's `Content.parts[].functionResponse.response` field is typed as
    // `google.protobuf.Struct` and ONLY accepts JSON objects. Scalars, arrays,
    // booleans, and null are rejected with HTTP 400 INVALID_ARGUMENT.
    //
    // `LlmMessage::Tool` content is an arbitrary JSON-encoded string. The
    // adapter must wrap any non-object value in `{ "result": <value> }` so
    // Gemini accepts the round-trip. Objects must pass through unchanged so
    // callers that already return dicts keep their keys.
    //
    // See: docs/superpowers/plans/2026-06-01-gemini-scalar-tool-response-fix.md
    // ----------------------------------------------------------------------

    fn build_request_with_tool_response(content: &str) -> crate::llm::domain::LlmRequest {
        use crate::llm::domain::{
            FunctionCall, LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderKind, ToolCall,
        };
        let provider =
            LlmProvider::new(ProviderKind::Google, "test_key".to_string(), None).unwrap();
        let config = LlmConfig::new(provider);
        let tool_call = ToolCall::new(
            "call_1".to_string(),
            FunctionCall::new("runCode".to_string(), "{}".to_string()),
        );
        let messages = vec![
            LlmMessage::user("compute 7!".to_string()).unwrap(),
            LlmMessage::assistant_with_tool_calls("".to_string(), vec![tool_call]).unwrap(),
            LlmMessage::tool("call_1".to_string(), content.to_string()).unwrap(),
        ];
        LlmRequest::new(messages, config, false).unwrap()
    }

    fn extract_function_response(contents: &[GeminiContent]) -> serde_json::Value {
        let function_msg = contents.iter().find(|c| c.role == "function").unwrap();
        let part = function_msg.parts.as_ref().unwrap().first().unwrap();
        part.function_response.clone().unwrap()
    }

    #[test]
    fn tool_response_scalar_number_is_wrapped() {
        let req = build_request_with_tool_response("5040");
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let fr = extract_function_response(&contents);
        assert!(
            fr["response"].is_object(),
            "response must be an object, got {fr:?}"
        );
        assert_eq!(fr["response"]["result"], 5040);
    }

    #[test]
    fn tool_response_array_is_wrapped() {
        let req = build_request_with_tool_response("[1, 2, 3]");
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let fr = extract_function_response(&contents);
        assert!(
            fr["response"].is_object(),
            "response must be an object, got {fr:?}"
        );
        assert_eq!(fr["response"]["result"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn tool_response_null_is_wrapped() {
        let req = build_request_with_tool_response("null");
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let fr = extract_function_response(&contents);
        assert!(
            fr["response"].is_object(),
            "response must be an object, got {fr:?}"
        );
        assert!(fr["response"]["result"].is_null());
    }

    #[test]
    fn tool_response_string_is_wrapped() {
        let req = build_request_with_tool_response("\"hello\"");
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let fr = extract_function_response(&contents);
        assert!(
            fr["response"].is_object(),
            "response must be an object, got {fr:?}"
        );
        assert_eq!(fr["response"]["result"], "hello");
    }

    // ----------------------------------------------------------------------
    // thoughtSignature round-trip (Gemini thinking models)
    //
    // Thinking models (gemini-3.5-flash, 2.5 with thinking budget) attach a
    // `thoughtSignature` to functionCall parts and REQUIRE it to be replayed
    // verbatim when the call is sent back in history, else the API rejects the
    // request with HTTP 400. `ToolCall::provider_signature` carries it through.
    // ----------------------------------------------------------------------

    fn build_request_replaying_tool_call(
        signature: Option<&str>,
    ) -> crate::llm::domain::LlmRequest {
        use crate::llm::domain::{
            FunctionCall, LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderKind, ToolCall,
        };
        let provider =
            LlmProvider::new(ProviderKind::Google, "test_key".to_string(), None).unwrap();
        let config = LlmConfig::new(provider);
        let mut tool_call = ToolCall::new(
            "call_1".to_string(),
            FunctionCall::new("load_skill".to_string(), "{}".to_string()),
        );
        tool_call.provider_signature = signature.map(|s| s.to_string());
        let messages = vec![
            LlmMessage::user("hi".to_string()).unwrap(),
            LlmMessage::assistant_with_tool_calls("".to_string(), vec![tool_call]).unwrap(),
        ];
        LlmRequest::new(messages, config, false).unwrap()
    }

    fn extract_model_function_call_part(contents: &[GeminiContent]) -> &GeminiPart {
        let model_msg = contents.iter().find(|c| c.role == "model").unwrap();
        model_msg
            .parts
            .as_ref()
            .unwrap()
            .iter()
            .find(|p| p.function_call.is_some())
            .unwrap()
    }

    #[test]
    fn assistant_tool_call_replays_thought_signature() {
        let req = build_request_replaying_tool_call(Some("sig_abc123"));
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let part = extract_model_function_call_part(&contents);
        assert_eq!(part.thought_signature.as_deref(), Some("sig_abc123"));
        // The wire JSON must carry the camelCase key.
        let wire = serde_json::to_value(part).unwrap();
        assert_eq!(wire["thoughtSignature"], "sig_abc123");
    }

    #[test]
    fn tool_call_without_signature_omits_thought_signature() {
        let req = build_request_replaying_tool_call(None);
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let part = extract_model_function_call_part(&contents);
        assert!(part.thought_signature.is_none());
        // Absent signature must NOT serialize the key (wire-format unchanged
        // for non-thinking models — no regression).
        let wire = serde_json::to_value(part).unwrap();
        assert!(wire.get("thoughtSignature").is_none());
    }

    #[test]
    fn gemini_part_deserializes_thought_signature_from_response() {
        // A functionCall part as returned by a thinking model.
        let raw = serde_json::json!({
            "functionCall": { "name": "load_skill", "args": {} },
            "thoughtSignature": "sig_from_response"
        });
        let part: GeminiPart = serde_json::from_value(raw).unwrap();
        assert_eq!(part.thought_signature.as_deref(), Some("sig_from_response"));
        assert!(part.function_call.is_some());
    }

    #[test]
    fn tool_response_object_passes_through_unchanged() {
        let req = build_request_with_tool_response("{\"answer\": 42, \"unit\": \"jiffies\"}");
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let fr = extract_function_response(&contents);
        assert_eq!(fr["response"]["answer"], 42);
        assert_eq!(fr["response"]["unit"], "jiffies");
        assert!(
            fr["response"].get("result").is_none(),
            "object must NOT be double-wrapped, got {fr:?}"
        );
    }

    #[test]
    fn tool_response_non_json_content_is_wrapped_as_string() {
        let req = build_request_with_tool_response("plain error text");
        let (_, contents) = GeminiAdapter::new().convert_messages(&req).unwrap();
        let fr = extract_function_response(&contents);
        assert_eq!(fr["response"]["result"], "plain error text");
    }

    // ---------------------------------------------------------------------
    // Implicit prompt caching (item 11, 2026-06-09) — Gemini 2.5+ models
    // automatically cache request prefixes (≥1024 tokens for 2.5-flash,
    // ≥2048 for 2.5-pro). On cache hits the API surfaces the count in
    // `usageMetadata.cachedContentTokenCount`. We must parse it and surface
    // it as `LlmUsage::cache_read_tokens` for cost-tracking parity with
    // OpenAI/Anthropic.
    // ---------------------------------------------------------------------

    #[test]
    fn usage_metadata_with_cached_content_populates_cache_read_tokens() {
        // Synthetic Gemini response with a cache hit. The adapter only
        // depends on the `usageMetadata` shape, so we go straight at the
        // struct rather than mocking the whole HTTP layer.
        let raw = serde_json::json!({
            "promptTokenCount": 1500,
            "candidatesTokenCount": 200,
            "thoughtsTokenCount": 50,
            "cachedContentTokenCount": 1200
        });
        let parsed: GeminiUsage = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.prompt_token_count, Some(1500));
        assert_eq!(parsed.candidates_token_count, Some(200));
        assert_eq!(parsed.thoughts_token_count, Some(50));
        assert_eq!(parsed.cached_content_token_count, Some(1200));

        // Now exercise the LlmUsage builder path the adapter uses.
        let mut usage = LlmUsage::new(
            parsed.prompt_token_count.unwrap_or(0),
            parsed.candidates_token_count.unwrap_or(0),
        );
        if let Some(t) = parsed.thoughts_token_count.filter(|&n| n > 0) {
            usage = usage.with_thinking_tokens(t);
        }
        if let Some(c) = parsed.cached_content_token_count.filter(|&n| n > 0) {
            usage = usage.with_cached_input_tokens_included(c);
        }
        assert_eq!(usage.cache_read_tokens, Some(1200));
        assert_eq!(usage.thinking_tokens, Some(50));
        // Gemini folds the cached count INTO `promptTokenCount`, so the adapter
        // subtracts it: 1500 reported - 1200 cached = 300 billed as fresh input.
        // Verified live 2026-08-23 — on a cache hit `promptTokenCount` did not
        // drop, proving the cached tokens are counted inside it.
        assert_eq!(
            usage.prompt_tokens, 300,
            "prompt_tokens must hold only fresh input, net of the cache hit"
        );
        // No token is lost or double-counted by the normalization.
        assert_eq!(usage.prompt_tokens + usage.cache_read_tokens.unwrap(), 1500);
        // The total covers every token the turn touched, cache included.
        assert_eq!(usage.total_tokens, 300 + 200 + 50 + 1200);
    }

    #[test]
    fn cached_input_normalization_is_independent_of_builder_order() {
        // `recompute_total` sums from scratch, so applying the cache before or
        // after thinking must land on the same numbers. Guards against a future
        // builder that recomputes from a partial subtotal.
        let a = LlmUsage::new(1500, 200)
            .with_thinking_tokens(50)
            .with_cached_input_tokens_included(1200);
        let b = LlmUsage::new(1500, 200)
            .with_cached_input_tokens_included(1200)
            .with_thinking_tokens(50);
        assert_eq!(a, b);
        assert_eq!(a.total_tokens, 1750);
    }

    #[test]
    fn anthropic_style_disjoint_cache_is_added_not_subtracted() {
        // The other half of the contract: Anthropic already reports input net of
        // cache, so `with_cache_read_tokens` must leave `prompt_tokens` alone and
        // only widen the total. Measured live: 404 fresh + 1809 cached = 2213.
        let usage = LlmUsage::new(404, 8).with_cache_read_tokens(1809);
        assert_eq!(
            usage.prompt_tokens, 404,
            "disjoint input must not be reduced"
        );
        assert_eq!(usage.total_tokens, 404 + 8 + 1809);
    }

    #[test]
    fn cached_input_exceeding_prompt_saturates_at_zero() {
        // Defensive: a provider reporting more cached than prompt tokens would
        // wrap a u32 subtraction and bill an astronomical number.
        let usage = LlmUsage::new(100, 10).with_cached_input_tokens_included(500);
        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.cache_read_tokens, Some(500));
    }

    #[test]
    fn usage_metadata_without_cache_omits_cache_read_tokens() {
        // No `cachedContentTokenCount` field → cache_read_tokens stays None.
        let raw = serde_json::json!({
            "promptTokenCount": 1500,
            "candidatesTokenCount": 200
        });
        let parsed: GeminiUsage = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.cached_content_token_count, None);

        let mut usage = LlmUsage::new(
            parsed.prompt_token_count.unwrap_or(0),
            parsed.candidates_token_count.unwrap_or(0),
        );
        if let Some(c) = parsed.cached_content_token_count.filter(|&n| n > 0) {
            usage = usage.with_cached_input_tokens_included(c);
        }
        assert_eq!(
            usage.cache_read_tokens, None,
            "no cachedContentTokenCount → cache_read_tokens must remain None"
        );
    }

    #[test]
    fn usage_metadata_with_zero_cached_tokens_does_not_set_field() {
        // Field present but zero → still don't surface. Avoids polluting
        // dashboards with "0 cached tokens" noise on every uncached call.
        let raw = serde_json::json!({
            "promptTokenCount": 800,
            "candidatesTokenCount": 100,
            "cachedContentTokenCount": 0
        });
        let parsed: GeminiUsage = serde_json::from_value(raw).unwrap();
        let mut usage = LlmUsage::new(
            parsed.prompt_token_count.unwrap_or(0),
            parsed.candidates_token_count.unwrap_or(0),
        );
        if let Some(c) = parsed.cached_content_token_count.filter(|&n| n > 0) {
            usage = usage.with_cached_input_tokens_included(c);
        }
        assert_eq!(usage.cache_read_tokens, None);
    }

    #[test]
    fn new_uses_production_default() {
        let a = GeminiAdapter::new();
        assert_eq!(
            a.base_url(),
            "https://generativelanguage.googleapis.com/v1beta"
        );
    }

    #[test]
    fn with_base_url_overrides() {
        let a = GeminiAdapter::with_base_url("http://127.0.0.1:4000/gemini/v1beta".to_string());
        assert_eq!(a.base_url(), "http://127.0.0.1:4000/gemini/v1beta");
    }

    #[tokio::test]
    async fn health_check_hits_models_endpoint_with_no_pinned_model() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "models": [] })))
            .mount(&server)
            .await;

        let adapter = GeminiAdapter::with_base_url(server.uri());
        let result = adapter.health_check().await;
        assert!(result.is_ok(), "expected Ok, got {:?}", result);

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].method.as_str(), "GET");
        assert_eq!(received[0].url.path(), "/models");
        assert!(received[0].body.is_empty(), "GET /models must send no body");
        let full_url = received[0].url.to_string();
        assert!(
            !full_url.contains("1.5"),
            "no deprecated pinned model in the request, got: {}",
            full_url
        );
        assert!(
            !full_url.contains("generateContent"),
            "must not call generateContent, got: {}",
            full_url
        );
    }

    #[tokio::test]
    async fn validate_credentials_ok_on_200() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(query_param("key", "real-gemini-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "models": [] })))
            .mount(&server)
            .await;

        let adapter = GeminiAdapter::with_base_url(server.uri());
        let result = adapter.validate_credentials("real-gemini-key").await;
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
    }

    #[tokio::test]
    async fn validate_credentials_err_on_401() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "error": { "message": "API key not valid" }
            })))
            .mount(&server)
            .await;

        let adapter = GeminiAdapter::with_base_url(server.uri());
        let err = adapter
            .validate_credentials("revoked-gemini-key")
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::InvalidApiKey), "got {:?}", err);
    }

    // ── Cache-safe temporal suffix (2026-06-11) ──────────────────────────

    fn gemini_req_with_suffix(suffix: Option<&str>) -> crate::llm::domain::LlmRequest {
        use crate::llm::domain::{LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderKind};
        let provider = LlmProvider::new(ProviderKind::Google, "k".to_string(), None).unwrap();
        let mut config = LlmConfig::new(provider);
        if let Some(s) = suffix {
            config = config.with_volatile_system_suffix(s);
        }
        let messages = vec![
            LlmMessage::system("stable system".into()).unwrap(),
            LlmMessage::user("hi".into()).unwrap(),
        ];
        LlmRequest::new(messages, config, false).unwrap()
    }

    #[test]
    fn volatile_suffix_appended_to_system_instruction() {
        let adapter = GeminiAdapter::new();
        let req = gemini_req_with_suffix(Some("## Temporal\n2026-06-11T14:00:00"));
        let body = adapter.build_request_body(&req).unwrap();
        let text = body["systemInstruction"]["parts"][0]["text"]
            .as_str()
            .unwrap();
        assert!(text.starts_with("stable system"));
        assert!(text.ends_with("## Temporal\n2026-06-11T14:00:00"));
    }

    #[test]
    fn no_suffix_leaves_system_instruction_unchanged() {
        let adapter = GeminiAdapter::new();
        let req = gemini_req_with_suffix(None);
        let body = adapter.build_request_body(&req).unwrap();
        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"]
                .as_str()
                .unwrap(),
            "stable system"
        );
    }

    // ── Multiple System messages (compaction summary) ────────────────────
    //
    // Regression lock, not a new behavior: this adapter already joins every
    // System instruction with a blank line. The invariant is cross-provider —
    // `history_compaction` appends the conversation summary as a SECOND System
    // message, and no adapter may drop one (the Anthropic adapter used to).

    #[test]
    fn two_system_messages_are_joined_into_one_instruction() {
        use crate::llm::domain::{LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderKind};
        let adapter = GeminiAdapter::new();
        let provider = LlmProvider::new(ProviderKind::Google, "k".to_string(), None).unwrap();
        let config = LlmConfig::new(provider);
        let messages = vec![
            LlmMessage::system("stable system".into()).unwrap(),
            LlmMessage::user("hi".into()).unwrap(),
            LlmMessage::system("## Conversation summary (older turns)".into()).unwrap(),
        ];
        let req = LlmRequest::new(messages, config, false).unwrap();
        let body = adapter.build_request_body(&req).unwrap();

        let text = body["systemInstruction"]["parts"][0]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("stable system"), "agent prompt survives");
        assert!(
            text.contains("## Conversation summary (older turns)"),
            "compaction summary survives"
        );
    }
}
