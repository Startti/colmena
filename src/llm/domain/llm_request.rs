use crate::llm::domain::{LlmRequestId, LlmMessage, LlmConfig, LlmError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    id: LlmRequestId,
    messages: Vec<LlmMessage>,
    config: LlmConfig,
    stream: bool,
}

impl LlmRequest {
    pub fn new(
        messages: Vec<LlmMessage>,
        config: LlmConfig,
        stream: bool,
    ) -> Result<Self, LlmError> {
        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        Ok(Self {
            id: LlmRequestId::new(),
            messages,
            config,
            stream,
        })
    }

    pub fn with_id(mut self, id: LlmRequestId) -> Self {
        self.id = id;
        self
    }

    // Getters
    pub fn id(&self) -> &LlmRequestId {
        &self.id
    }

    pub fn messages(&self) -> &[LlmMessage] {
        &self.messages
    }

    pub fn config(&self) -> &LlmConfig {
        &self.config
    }

    pub fn stream(&self) -> bool {
        self.stream
    }

    // Convenience methods
    pub fn is_streaming(&self) -> bool {
        self.stream
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    pub fn last_message(&self) -> Option<&LlmMessage> {
        self.messages.last()
    }

    pub fn first_message(&self) -> Option<&LlmMessage> {
        self.messages.first()
    }
}