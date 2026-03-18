use crate::llm::domain::{LlmConfig, LlmError, LlmMessage, LlmRequestId, ToolDefinition};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    id: LlmRequestId,
    messages: Vec<LlmMessage>,
    config: LlmConfig,
    stream: bool,

    /// Optional tools available for this request
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolDefinition>>,

    /// Control how the model uses tools ("auto", "none", or specific function name)
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
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

        // Validate consecutive roles, ignoring system messages
        for i in 1..messages.len() {
            let prev_msg = &messages[i - 1];
            let current_msg = &messages[i];

            if prev_msg.role() == current_msg.role() {
                // Allow consecutive Tool messages (for parallel tool calls)
                if *current_msg.role() == crate::llm::domain::MessageRole::Tool {
                    continue;
                }

                return Err(LlmError::ConsecutiveRoles {
                    role: current_msg.role().to_string(),
                    index1: i - 1,
                    index2: i,
                });
            }
        }

        Ok(Self {
            id: LlmRequestId::new(),
            messages,
            config,
            stream,
            tools: None,
            tool_choice: None,
        })
    }

    pub fn with_id(mut self, id: LlmRequestId) -> Self {
        self.id = id;
        self
    }

    /// Add tools to the request
    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Set how the model should use tools
    pub fn with_tool_choice(mut self, choice: String) -> Self {
        self.tool_choice = Some(choice);
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

    /// Get the tools available for this request
    pub fn tools(&self) -> Option<&[ToolDefinition]> {
        self.tools.as_deref()
    }

    /// Get the tool choice setting
    pub fn tool_choice(&self) -> Option<&str> {
        self.tool_choice.as_deref()
    }

    /// Check if tools are available
    pub fn has_tools(&self) -> bool {
        self.tools.as_ref().map(|t| !t.is_empty()).unwrap_or(false)
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
        assert_eq!(
            request.config().provider().api_key(),
            config.provider().api_key()
        );
        assert!(!request.stream());
        assert_eq!(request.last_message(), messages.last());
    }

    #[test]
    fn test_request_creation_fails_on_consecutive_roles() {
        let config = create_test_config();
        let messages = vec![
            LlmMessage::new(MessageRole::User, "Hello".to_string()).unwrap(),
            LlmMessage::new(MessageRole::User, "How are you?".to_string()).unwrap(),
        ];
        let result = LlmRequest::new(messages, config, false);

        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::ConsecutiveRoles {
                role,
                index1,
                index2,
            } => {
                assert_eq!(role, "user");
                assert_eq!(index1, 0);
                assert_eq!(index2, 1);
            }
            e => panic!("Expected ConsecutiveRoles error, but got {:?}", e),
        }
    }

    #[test]
    fn test_request_creation_succeeds_with_interspersed_system_messages() {
        let config = create_test_config();
        let messages = vec![
            LlmMessage::new(MessageRole::User, "Hello".to_string()).unwrap(),
            LlmMessage::new(MessageRole::System, "You are a bot.".to_string()).unwrap(),
            LlmMessage::new(MessageRole::User, "How are you?".to_string()).unwrap(),
        ];
        // This should not fail because the consecutive check ignores system messages
        let result = LlmRequest::new(messages, config, false);
        assert!(result.is_ok());
    }
}
