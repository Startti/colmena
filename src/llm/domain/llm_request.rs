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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{LlmConfig, LlmProvider, MessageRole, ProviderKind};

    // Helper para crear una configuración de prueba
    fn create_test_config() -> LlmConfig {
        let provider = LlmProvider::new(
            ProviderKind::Gemini,
            "test_api_key".to_string(),
            Some("gemini-pro".to_string()),
        )
        .unwrap();
        LlmConfig::new(provider)
    }

    // Helper para crear mensajes de prueba
    fn create_test_messages() -> Vec<LlmMessage> {
        vec![LlmMessage::new(MessageRole::User, "Hello".to_string()).unwrap()]
    }

    #[test]
    fn test_request_creation_success() {
        let config = create_test_config();
        let messages = create_test_messages();
        let request = LlmRequest::new(messages, config, true).unwrap();

        assert!(!request.id().value().to_string().is_empty());
        assert_eq!(request.message_count(), 1);
        assert_eq!(request.config().provider().kind(), &ProviderKind::Gemini);
        assert!(request.is_streaming());
    }

    #[test]
    fn test_request_creation_fails_on_empty_messages() {
        let config = create_test_config();
        let messages: Vec<LlmMessage> = vec![];
        let result = LlmRequest::new(messages, config, false);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), LlmError::EmptyMessages);
    }

    #[test]
    fn test_getters_return_correct_values() {
        let config = create_test_config();
        let messages = create_test_messages();
        let request = LlmRequest::new(messages.clone(), config.clone(), false).unwrap();

        assert_eq!(request.messages(), &messages[..]);
        assert_eq!(request.config().provider().api_key(), config.provider().api_key());
        assert!(!request.stream());
        assert_eq!(request.last_message(), messages.last());
    }
}