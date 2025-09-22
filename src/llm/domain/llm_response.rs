use crate::llm::domain::{LlmProvider, LlmRequestId, LlmResponseId, LlmUsage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    id: LlmResponseId,
    request_id: LlmRequestId,
    content: String,
    usage: Option<LlmUsage>,
    provider: LlmProvider,
    timestamp: DateTime<Utc>,
    finish_reason: Option<String>,
}

impl LlmResponse {
    pub fn new(request_id: LlmRequestId, content: String, provider: LlmProvider) -> Self {
        Self {
            id: LlmResponseId::new(),
            request_id,
            content,
            usage: None,
            provider,
            timestamp: Utc::now(),
            finish_reason: None,
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

    // Getters
    pub fn id(&self) -> &LlmResponseId {
        &self.id
    }

    pub fn request_id(&self) -> &LlmRequestId {
        &self.request_id
    }

    pub fn content(&self) -> &str {
        &self.content
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
}

// For streaming responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmStreamChunk {
    id: LlmResponseId,
    request_id: LlmRequestId,
    content: String,
    provider: LlmProvider,
    timestamp: DateTime<Utc>,
    is_final: bool,
}

impl LlmStreamChunk {
    pub fn new(
        request_id: LlmRequestId,
        content: String,
        provider: LlmProvider,
        is_final: bool,
    ) -> Self {
        Self {
            id: LlmResponseId::new(),
            request_id,
            content,
            provider,
            timestamp: Utc::now(),
            is_final,
        }
    }

    // Getters
    pub fn id(&self) -> &LlmResponseId {
        &self.id
    }

    pub fn request_id(&self) -> &LlmRequestId {
        &self.request_id
    }

    pub fn content(&self) -> &str {
        &self.content
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
        );

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
            "chunk content".to_string(),
            provider.clone(),
            true,
        );

        assert_eq!(chunk.request_id(), &request_id);
        assert_eq!(chunk.content(), "chunk content");
        assert_eq!(chunk.provider().kind(), provider.kind());
        assert!(chunk.is_final());
    }
}
