use crate::llm::domain::{LlmResponseId, LlmRequestId, LlmProvider, LlmUsage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    id: LlmResponseId,
    request_id: LlmRequestId,
    content: String,
    usage: Option<LlmUsage>,
    provider: LlmProvider,
    model: String,
    timestamp: DateTime<Utc>,
    finish_reason: Option<String>,
}

impl LlmResponse {
    pub fn new(
        request_id: LlmRequestId,
        content: String,
        provider: LlmProvider,
        model: String,
    ) -> Self {
        Self {
            id: LlmResponseId::new(),
            request_id,
            content,
            usage: None,
            provider,
            model,
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
        &self.model
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
    model: String,
    timestamp: DateTime<Utc>,
    is_final: bool,
}

impl LlmStreamChunk {
    pub fn new(
        request_id: LlmRequestId,
        content: String,
        provider: LlmProvider,
        model: String,
        is_final: bool,
    ) -> Self {
        Self {
            id: LlmResponseId::new(),
            request_id,
            content,
            provider,
            model,
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
        &self.model
    }

    pub fn timestamp(&self) -> &DateTime<Utc> {
        &self.timestamp
    }

    pub fn is_final(&self) -> bool {
        self.is_final
    }
}