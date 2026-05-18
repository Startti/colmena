use crate::llm::domain::{
    FileSource, FunctionCall, LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream,
    LlmStreamChunk, LlmStreamPart, LlmUsage, ToolCall, ToolCallChunk,
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

    fn build_messages(&self, request: &LlmRequest) -> Result<Vec<serde_json::Value>, LlmError> {
        let mut out = Vec::with_capacity(request.messages().len());
        for msg in request.messages() {
            let mut message_json = json!({
                "role": msg.role().as_str(),
            });

            if let Some(files) = msg.files() {
                let mut content_arr = vec![json!({
                    "type": "text",
                    "text": msg.content()
                })];

                use base64::{engine::general_purpose::STANDARD, Engine as _};
                for file in files {
                    if file.mime_type.starts_with("image/") {
                        let image_url = match &file.source {
                            FileSource::InlineBytes { bytes } => {
                                let b64 = STANDARD.encode(bytes);
                                json!({ "url": format!("data:{};base64,{}", file.mime_type, b64) })
                            }
                            FileSource::SignedUrl(url) => {
                                // OpenAI's chat completions image_url requires `url`
                                // (no `file_id` support — that's Responses API only).
                                // Pass the signed URL through; OpenAI fetches it server-side.
                                json!({ "url": url })
                            }
                            FileSource::Uploaded(r) => {
                                // chat completions image_url does NOT accept file_id;
                                // only the Responses API does. If we're here for an
                                // image, the use case did not short-circuit (unexpected).
                                // Surface the bug explicitly via InternalError.
                                return Err(LlmError::InternalError {
                                    message: format!(
                                        "OpenAI chat completions adapter received Uploaded image '{}' (file_id={}). \
                                         image_url does not accept file_id; only Responses API does. \
                                         The use case should have short-circuited image+OpenAI to keep SignedUrl.",
                                        file.filename, r.provider_file_id
                                    ),
                                });
                            }
                        };
                        content_arr.push(json!({
                            "type": "image_url",
                            "image_url": image_url,
                        }));
                    } else {
                        eprintln!(
                            "WARN: OpenAI chat completions only support image files. Ignoring '{}' (mime '{}')",
                            file.filename, file.mime_type
                        );
                    }
                }
                message_json["content"] = json!(content_arr);
            } else {
                message_json["content"] = json!(msg.content());
            }

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

            out.push(message_json);
        }
        Ok(out)
    }

    fn build_request_body(&self, request: &LlmRequest) -> Result<serde_json::Value, LlmError> {
        let mut body = json!({
            "model": request.config().model(),
            "messages": self.build_messages(request)?,
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
                    let parameters = tool.input_schema_override.clone().unwrap_or_else(|| {
                        json!({
                            "type": tool.parameters.schema_type,
                            "properties": tool.parameters.properties,
                            "required": tool.parameters.required
                        })
                    });
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": parameters,
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

        // o-series reasoning models: map thinking_budget → reasoning_effort.
        // OpenAI does not surface reasoning content, so no stream changes needed.
        if let Some(budget) = request.config().thinking_budget() {
            let effort = if budget <= 1000 {
                "low"
            } else if budget <= 5000 {
                "medium"
            } else {
                "high"
            };
            body["reasoning_effort"] = json!(effort);
        }

        Ok(body)
    }

    fn is_responses_api_required(&self, request: &LlmRequest) -> bool {
        request.messages().iter().any(|msg| {
            if let Some(files) = msg.files() {
                files.iter().any(|f| !f.mime_type.starts_with("image/"))
            } else {
                false
            }
        })
    }

    async fn call_chat_completions(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let body = self.build_request_body(&request)?;

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

        let response_text = response
            .text()
            .await
            .map_err(|e| LlmError::parsing_error(e.to_string()))?;

        let openai_response: OpenAiResponse = serde_json::from_str(&response_text)
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

        let usage = openai_response.usage.map(openai_usage_to_llm_usage);

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

    async fn stream_chat_completions(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        let body = self.build_request_body(&request)?;

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
                                        LlmStreamPart::Usage(openai_usage_to_llm_usage(usage)),
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
}

#[async_trait]
impl LlmRepository for OpenAiAdapter {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        if self.is_responses_api_required(&request) {
            self.call_responses(request).await
        } else {
            self.call_chat_completions(request).await
        }
    }

    async fn stream(
        &self,
        request: LlmRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamChunk, LlmError>> + Send>>, LlmError>
    {
        if self.is_responses_api_required(&request) {
            self.stream_responses(request).await
        } else {
            self.stream_chat_completions(request).await
        }
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
struct OpenAiPromptDetails {
    #[serde(default)]
    cached_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompletionDetails {
    #[serde(default)]
    reasoning_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    prompt_tokens_details: Option<OpenAiPromptDetails>,
    completion_tokens_details: Option<OpenAiCompletionDetails>,
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
fn openai_usage_to_llm_usage(u: OpenAiUsage) -> LlmUsage {
    let mut usage = LlmUsage::new(u.prompt_tokens, u.completion_tokens);
    if let Some(r) = u
        .completion_tokens_details
        .filter(|d| d.reasoning_tokens > 0)
    {
        usage = usage.with_thinking_tokens(r.reasoning_tokens);
    }
    if let Some(p) = u.prompt_tokens_details.filter(|d| d.cached_tokens > 0) {
        usage = usage.with_cache_read_tokens(p.cached_tokens);
    }
    usage
}

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

impl OpenAiAdapter {
    fn build_responses_request_body(
        &self,
        request: &LlmRequest,
    ) -> Result<serde_json::Value, LlmError> {
        let mut input_msgs = Vec::with_capacity(request.messages().len());
        for msg in request.messages() {
            let mut content_arr = vec![json!({
                "type": "input_text",
                "text": msg.content()
            })];

            if let Some(files) = msg.files() {
                use base64::{engine::general_purpose::STANDARD, Engine as _};
                for file in files {
                    let file_part = match &file.source {
                        FileSource::InlineBytes { bytes } => {
                            let b64 = STANDARD.encode(bytes);
                            let data_uri = format!("data:{};base64,{}", file.mime_type, b64);
                            json!({
                                "type": "input_file",
                                "filename": file.filename,
                                "file_data": data_uri,
                            })
                        }
                        FileSource::Uploaded(r) => json!({
                            "type": "input_file",
                            "file_id": r.provider_file_id,
                        }),
                        FileSource::SignedUrl(_) => {
                            return Err(LlmError::InternalError {
                                message: format!(
                                    "OpenAI responses adapter received unresolved SignedUrl for '{}'. \
                                     LlmCallUseCase::resolve_files must run before reaching the adapter.",
                                    file.filename
                                ),
                            });
                        }
                    };
                    content_arr.push(file_part);
                }
            }

            input_msgs.push(json!({
                "role": msg.role().as_str(),
                "content": content_arr
            }));
        }

        let mut body = json!({
            "model": request.config().model(),
            "input": input_msgs,
        });

        if let Some(temp) = request.config().temperature() {
            body["temperature"] = json!(temp);
        }
        if let Some(max_tokens) = request.config().max_tokens() {
            body["max_tokens"] = json!(max_tokens);
        }
        if let Some(top_p) = request.config().top_p() {
            body["top_p"] = json!(top_p);
        }

        Ok(body)
    }

    async fn call_responses(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let body = self.build_responses_request_body(&request)?;

        let response = self
            .client
            .post(format!("{}/responses", self.base_url))
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
                "OpenAI Responses API error: {}",
                error_text
            )));
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| LlmError::parsing_error(e.to_string()))?;
        let json_val: serde_json::Value = serde_json::from_str(&response_text)
            .map_err(|e| LlmError::parsing_error(e.to_string()))?;

        let mut content = String::new();
        if let Some(outputs) = json_val.get("output").and_then(|o| o.as_array()) {
            if let Some(first) = outputs.first() {
                if let Some(contents) = first.get("content").and_then(|c| c.as_array()) {
                    for block in contents {
                        if block.get("type").and_then(|t| t.as_str()) == Some("output_text") {
                            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                content.push_str(t);
                            }
                        }
                    }
                }
            }
        }

        let mut usage_obj = None;
        if let Some(usage) = json_val.get("usage") {
            if let (Some(input), Some(output)) = (
                usage.get("input_tokens").and_then(|v| v.as_u64()),
                usage.get("output_tokens").and_then(|v| v.as_u64()),
            ) {
                usage_obj = Some(LlmUsage::new(input as u32, output as u32));
            }
        }

        let mut llm_response = LlmResponse::new(
            request.id().clone(),
            content,
            request.config().provider().clone(),
        )?;
        if let Some(u) = usage_obj {
            llm_response = llm_response.with_usage(u);
        }

        Ok(llm_response)
    }

    async fn stream_responses(
        &self,
        request: LlmRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamChunk, LlmError>> + Send>>, LlmError>
    {
        let mut body = self.build_responses_request_body(&request)?;
        body["stream"] = json!(true);

        let response = self
            .client
            .post(format!("{}/responses", self.base_url))
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
                "OpenAI Responses API error: {}",
                error_text
            )));
        }

        let stream = response.bytes_stream();
        let sse_parser = SseParser::new(stream);
        let request_id = request.id().clone();
        let provider = request.config().provider().clone();

        let sse_stream = async_stream::stream! {
            let mut parser = sse_parser;
            while let Some(event_res) = parser.next().await {
                match event_res {
                    Ok(SseEvent::Message(data)) => {
                        if data.starts_with("[DONE]") {
                            break;
                        }

                        let event_json: serde_json::Value = match serde_json::from_str(&data) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        let event_type = event_json.get("type").and_then(|t| t.as_str()).unwrap_or("");

                        if event_type == "response.output_text.delta" {
                            if let Some(delta) = event_json.get("delta").and_then(|d| d.as_str()) {
                                yield Ok(LlmStreamChunk::new(
                                    request_id.clone(),
                                    LlmStreamPart::Content(delta.to_string()),
                                    provider.clone(),
                                    false,
                                ));
                            }
                        } else if event_type == "response.completed" {
                            if let Some(usage) = event_json.get("response").and_then(|r| r.get("usage")) {
                                if let (Some(input_tokens), Some(output_tokens)) = (
                                    usage.get("input_tokens").and_then(|v| v.as_u64()),
                                    usage.get("output_tokens").and_then(|v| v.as_u64()),
                                ) {
                                    yield Ok(LlmStreamChunk::new(
                                        request_id.clone(),
                                        LlmStreamPart::Usage(LlmUsage::new(
                                            input_tokens as u32,
                                            output_tokens as u32,
                                        )),
                                        provider.clone(),
                                        false,
                                    ));
                                }
                            }
                            yield Ok(LlmStreamChunk::new(
                                request_id.clone(),
                                LlmStreamPart::Content(String::new()),
                                provider.clone(),
                                true,
                            ).with_finish_reason("stop".to_string()));
                        }
                    }
                    Err(e) => yield Err(e),
                }
            }
        };

        Ok(Box::pin(sse_stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_serializes_uploaded_pdf_with_file_id() {
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
                provider: ProviderKind::OpenAi,
                provider_file_id: "file-abc".into(),
                mime_type: "application/pdf".into(),
                filename: "x.pdf".into(),
                expires_at: None,
            }),
            retained_inline_bytes: None,
        };
        let msg = LlmMessage::user_with_files("describe".into(), vec![file]).unwrap();
        let provider =
            LlmProvider::new(ProviderKind::OpenAi, "k".into(), Some("gpt-5".into())).unwrap();
        let config = LlmConfig::new(provider);
        let request = LlmRequest::new(vec![msg], config, false).unwrap();

        let adapter = OpenAiAdapter::new();
        let body = adapter.build_responses_request_body(&request).unwrap();
        let input = &body["input"];
        let content = &input[0]["content"];
        let file_part = content
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["type"] == "input_file")
            .unwrap();
        assert_eq!(file_part["file_id"], "file-abc");
        assert!(file_part.get("file_data").is_none() || file_part["file_data"].is_null());
        // OpenAI Responses API requires mutually-exclusive file_id XOR filename;
        // when using file_id we must NOT include filename.
        assert!(file_part.get("filename").is_none() || file_part["filename"].is_null());
    }

    #[test]
    fn responses_returns_error_on_signed_url() {
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
        let provider =
            LlmProvider::new(ProviderKind::OpenAi, "k".into(), Some("gpt-5".into())).unwrap();
        let config = LlmConfig::new(provider);
        let request = LlmRequest::new(vec![msg], config, false).unwrap();

        let adapter = OpenAiAdapter::new();
        let err = adapter.build_responses_request_body(&request).unwrap_err();
        assert!(matches!(
            err,
            crate::llm::domain::LlmError::InternalError { .. }
        ));
    }

    #[test]
    fn chat_completions_serializes_signed_url_image_as_url() {
        use crate::llm::domain::{
            FileData, FileSource, LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderKind,
        };
        let file = FileData {
            document_id: Some("doc-img-1".into()),
            mime_type: "image/png".into(),
            filename: "x.png".into(),
            size_hint: None,
            source: FileSource::SignedUrl("https://storage.googleapis.com/bucket/x?sig=y".into()),
            retained_inline_bytes: None,
        };
        let msg = LlmMessage::user_with_files("describe".into(), vec![file]).unwrap();
        let provider =
            LlmProvider::new(ProviderKind::OpenAi, "k".into(), Some("gpt-4o-mini".into())).unwrap();
        let config = LlmConfig::new(provider);
        let request = LlmRequest::new(vec![msg], config, false).unwrap();

        let adapter = OpenAiAdapter::new();
        let body = adapter.build_request_body(&request).unwrap();
        let messages = body["messages"].as_array().unwrap();
        let content = messages[0]["content"].as_array().unwrap();
        let img = content.iter().find(|c| c["type"] == "image_url").unwrap();
        assert_eq!(
            img["image_url"]["url"],
            "https://storage.googleapis.com/bucket/x?sig=y"
        );
        assert!(img["image_url"].get("file_id").is_none() || img["image_url"]["file_id"].is_null());
    }

    #[test]
    fn chat_completions_returns_error_on_uploaded_image() {
        use crate::llm::domain::{
            FileData, FileSource, LlmConfig, LlmMessage, LlmProvider, LlmRequest, ProviderFileRef,
            ProviderKind,
        };
        let file = FileData {
            document_id: Some("doc-1".into()),
            mime_type: "image/png".into(),
            filename: "x.png".into(),
            size_hint: None,
            source: FileSource::Uploaded(ProviderFileRef {
                provider: ProviderKind::OpenAi,
                provider_file_id: "file-abc".into(),
                mime_type: "image/png".into(),
                filename: "x.png".into(),
                expires_at: None,
            }),
            retained_inline_bytes: None,
        };
        let msg = LlmMessage::user_with_files("describe".into(), vec![file]).unwrap();
        let provider =
            LlmProvider::new(ProviderKind::OpenAi, "k".into(), Some("gpt-4o-mini".into())).unwrap();
        let config = LlmConfig::new(provider);
        let request = LlmRequest::new(vec![msg], config, false).unwrap();

        let adapter = OpenAiAdapter::new();
        let err = adapter.build_request_body(&request).unwrap_err();
        assert!(matches!(
            err,
            crate::llm::domain::LlmError::InternalError { .. }
        ));
    }
}
