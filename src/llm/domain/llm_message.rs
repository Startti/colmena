use crate::llm::domain::LlmError;
use chrono::{DateTime, Utc};
#[cfg(test)]
use derivative::Derivative;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        }
    }

    pub fn from_str(s: &str) -> Result<Self, LlmError> {
        match s.to_lowercase().as_str() {
            "system" => Ok(MessageRole::System),
            "user" => Ok(MessageRole::User),
            "assistant" => Ok(MessageRole::Assistant),
            _ => Err(LlmError::invalid_message_role(s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(Derivative))]
#[cfg_attr(test, derivative(PartialEq))]
pub struct LlmMessage {
    role: MessageRole,
    content: String,
    #[cfg_attr(test, derivative(PartialEq = "ignore"))]
    timestamp: DateTime<Utc>,
}

impl LlmMessage {
    pub fn new(role: MessageRole, content: String) -> Result<Self, LlmError> {
        if content.trim().is_empty() {
            return Err(LlmError::EmptyMessageContent);
        }

        Ok(Self {
            role,
            content: content.trim().to_string(),
            timestamp: Utc::now(),
        })
    }

    pub fn system(content: String) -> Result<Self, LlmError> {
        Self::new(MessageRole::System, content)
    }

    pub fn user(content: String) -> Result<Self, LlmError> {
        Self::new(MessageRole::User, content)
    }

    pub fn assistant(content: String) -> Result<Self, LlmError> {
        Self::new(MessageRole::Assistant, content)
    }

    pub fn role(&self) -> &MessageRole {
        &self.role
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn timestamp(&self) -> &DateTime<Utc> {
        &self.timestamp
    }

    pub fn with_timestamp(mut self, timestamp: DateTime<Utc>) -> Self {
        self.timestamp = timestamp;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_creation_success() {
        let msg = LlmMessage::new(MessageRole::User, "  Hello World  ".to_string()).unwrap();
        assert_eq!(msg.role(), &MessageRole::User);
        assert_eq!(msg.content(), "Hello World"); // Verifica que el contenido se ha trimeado
    }

    #[test]
    fn test_message_creation_fails_on_empty_content() {
        let result = LlmMessage::new(MessageRole::User, "".to_string());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), LlmError::EmptyMessageContent);
    }

    #[test]
    fn test_message_creation_fails_on_whitespace_content() {
        let result = LlmMessage::new(MessageRole::User, "   ".to_string());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), LlmError::EmptyMessageContent);
    }

    #[test]
    fn test_message_role_from_str() {
        assert_eq!(
            MessageRole::from_str("system").unwrap(),
            MessageRole::System
        );
        assert_eq!(MessageRole::from_str("USER").unwrap(), MessageRole::User);
        assert_eq!(
            MessageRole::from_str("assistant").unwrap(),
            MessageRole::Assistant
        );
        assert!(MessageRole::from_str("invalid").is_err());

        // Test específico del error
        match MessageRole::from_str("invalid_role") {
            Err(LlmError::InvalidMessageRole { role }) => {
                assert_eq!(role, "invalid_role");
            }
            _ => panic!("Expected InvalidMessageRole error"),
        }
    }
}
