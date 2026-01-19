use crate::llm::domain::{
    LlmMessage, LlmProvider, LlmRequestId, LlmResponseId, LlmUsage, ToolCall, ToolResult,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    id: LlmResponseId,
    request_id: LlmRequestId,
    message: LlmMessage,
    usage: Option<LlmUsage>,
    provider: LlmProvider,
    timestamp: DateTime<Utc>,
    finish_reason: Option<String>,

    /// Tool calls requested by the LLM
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCall>>,
}

impl LlmResponse {
    pub fn new(
        request_id: LlmRequestId,
        content: String,
        provider: LlmProvider,
    ) -> Result<Self, crate::llm::domain::LlmError> {
        let message = LlmMessage::assistant(content)?;
        Ok(Self {
            id: LlmResponseId::new(),
            request_id,
            message,
            usage: None,
            provider,
            timestamp: Utc::now(),
            finish_reason: None,
            tool_calls: None,
        })
    }

    pub fn with_message(
        request_id: LlmRequestId,
        message: LlmMessage,
        provider: LlmProvider,
    ) -> Self {
        Self {
            id: LlmResponseId::new(),
            request_id,
            message,
            usage: None,
            provider,
            timestamp: Utc::now(),
            finish_reason: None,
            tool_calls: None,
        }
    }

    pub fn with_usage(mut self, usage: LlmUsage) -> Self {
        self.usage = Some(usage);
        self
    }

    pub fn with_finish_reason(mut self, reason: String) -> Self {
        self.finish_reason = Some(reason);
        self
    }

    pub fn with_timestamp(mut self, timestamp: DateTime<Utc>) -> Self {
        self.timestamp = timestamp;
        self
    }

    pub fn with_tool_calls(mut self, tool_calls: Vec<ToolCall>) -> Self {
        // Update the message to include tool_calls
        let content = self.message.content().to_string();
        self.message = LlmMessage::assistant_with_tool_calls(content, tool_calls.clone())
            .unwrap_or(self.message); // Fallback to original if fails
        self.tool_calls = Some(tool_calls);
        self
    }

    // Getters
    pub fn id(&self) -> &LlmResponseId {
        &self.id
    }

    pub fn request_id(&self) -> &LlmRequestId {
        &self.request_id
    }

    pub fn content(&self) -> &str {
        self.message.content()
    }

    pub fn message(&self) -> &LlmMessage {
        &self.message
    }

    pub fn usage(&self) -> Option<&LlmUsage> {
        self.usage.as_ref()
    }

    pub fn provider(&self) -> &LlmProvider {
        &self.provider
    }

    pub fn model(&self) -> &str {
        self.provider.model()
    }

    pub fn timestamp(&self) -> &DateTime<Utc> {
        &self.timestamp
    }

    pub fn finish_reason(&self) -> Option<&str> {
        self.finish_reason.as_deref()
    }

    // Utility methods
    pub fn is_complete(&self) -> bool {
        self.finish_reason.is_some()
    }

    pub fn token_count(&self) -> Option<u32> {
        self.usage.as_ref().map(|u| u.total_tokens)
    }

    /// Get tool calls requested by the LLM
    pub fn tool_calls(&self) -> Option<&[ToolCall]> {
        self.tool_calls.as_deref()
    }

    /// Check if the response contains tool calls
    pub fn has_tool_calls(&self) -> bool {
        self.tool_calls
            .as_ref()
            .map(|t| !t.is_empty())
            .unwrap_or(false)
    }
}

// For streaming responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallChunk {
    pub index: usize,
    pub id: String,
    pub name: String,
    pub args_chunk: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LlmStreamPart {
    Content(String),
    ToolCallChunk(ToolCallChunk),
    Usage(LlmUsage),
    ToolCallStart(ToolCall),
    ToolCallFinish(ToolResult),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmStreamChunk {
    id: LlmResponseId,
    request_id: LlmRequestId,
    part: LlmStreamPart,
    provider: LlmProvider,
    timestamp: DateTime<Utc>,
    is_final: bool,
    finish_reason: Option<String>,
}

impl LlmStreamChunk {
    pub fn new(
        request_id: LlmRequestId,
        part: LlmStreamPart,
        provider: LlmProvider,
        is_final: bool,
    ) -> Self {
        Self {
            id: LlmResponseId::new(),
            request_id,
            part,
            provider,
            timestamp: Utc::now(),
            is_final,
            finish_reason: None,
        }
    }

    pub fn with_finish_reason(mut self, reason: String) -> Self {
        self.finish_reason = Some(reason);
        self
    }

    // Getters
    pub fn id(&self) -> &LlmResponseId {
        &self.id
    }

    pub fn request_id(&self) -> &LlmRequestId {
        &self.request_id
    }

    pub fn part(&self) -> &LlmStreamPart {
        &self.part
    }

    pub fn content(&self) -> &str {
        match &self.part {
            LlmStreamPart::Content(c) => c,
            _ => "",
        }
    }

    pub fn provider(&self) -> &LlmProvider {
        &self.provider
    }

    pub fn model(&self) -> &str {
        self.provider.model()
    }

    pub fn timestamp(&self) -> &DateTime<Utc> {
        &self.timestamp
    }

    pub fn is_final(&self) -> bool {
        self.is_final
    }

    pub fn finish_reason(&self) -> Option<&str> {
        self.finish_reason.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{LlmProvider, ProviderKind};

    // Helper para crear un LlmProvider de prueba
    fn create_test_provider() -> LlmProvider {
        LlmProvider::new(
            ProviderKind::Gemini,
            "test_api_key".to_string(),
            Some("gemini-pro".to_string()),
        )
        .unwrap()
    }

    #[test]
    fn test_response_creation() {
        let request_id = LlmRequestId::new();
        let provider = create_test_provider();
        let response = LlmResponse::new(
            request_id.clone(),
            "Test content".to_string(),
            provider.clone(),
        )
        .unwrap();

        assert_eq!(response.request_id(), &request_id);
        assert_eq!(response.content(), "Test content");
        assert_eq!(response.provider().kind(), provider.kind());
        assert!(response.usage().is_none());
        assert!(response.finish_reason().is_none());
        assert!(!response.is_complete());
    }

    #[test]
    fn test_response_builder_methods() {
        let request_id = LlmRequestId::new();
        let provider = create_test_provider();
        let usage = LlmUsage::new(10, 20);

        let response = LlmResponse::new(
            request_id.clone(),
            "Test content".to_string(),
            provider.clone(),
        )
        .unwrap()
        .with_usage(usage.clone())
        .with_finish_reason("stop".to_string());

        assert_eq!(response.usage().unwrap(), &usage);
        assert_eq!(response.finish_reason().unwrap(), "stop");
        assert!(response.is_complete());
        assert_eq!(response.token_count(), Some(30));
    }

    #[test]
    fn test_stream_chunk_creation() {
        let request_id = LlmRequestId::new();
        let provider = create_test_provider();
        let chunk = LlmStreamChunk::new(
            request_id.clone(),
            LlmStreamPart::Content("chunk content".to_string()),
            provider.clone(),
            true,
        )
        .with_finish_reason("stop".to_string());

        assert_eq!(chunk.request_id(), &request_id);
        assert_eq!(chunk.content(), "chunk content");
        assert_eq!(chunk.provider().kind(), provider.kind());
        assert!(chunk.is_final());
        assert_eq!(chunk.finish_reason(), Some("stop"));
    }
}
