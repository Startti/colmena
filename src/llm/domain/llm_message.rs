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
    Tool,
}

use std::fmt;
use std::str::FromStr;

impl fmt::Display for MessageRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for MessageRole {
    type Err = LlmError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "system" => Ok(MessageRole::System),
            "user" => Ok(MessageRole::User),
            "assistant" => Ok(MessageRole::Assistant),
            "tool" => Ok(MessageRole::Tool),
            _ => Err(LlmError::invalid_message_role(s)),
        }
    }
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(Derivative))]
#[cfg_attr(test, derivative(PartialEq))]
pub struct LlmMessage {
    role: MessageRole,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<crate::llm::domain::ToolCall>>,
    #[cfg_attr(test, derivative(PartialEq = "ignore"))]
    timestamp: DateTime<Utc>,
}

impl LlmMessage {
    pub fn new(role: MessageRole, content: String) -> Result<Self, LlmError> {
        if role != MessageRole::Assistant && content.trim().is_empty() {
            return Err(LlmError::EmptyMessageContent);
        }

        Ok(Self {
            role,
            content: content.trim().to_string(),
            tool_call_id: None,
            tool_calls: None,
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

    pub fn assistant_with_tool_calls(
        content: String,
        tool_calls: Vec<crate::llm::domain::ToolCall>,
    ) -> Result<Self, LlmError> {
        let mut msg = Self::new(MessageRole::Assistant, content)?;
        msg.tool_calls = Some(tool_calls);
        Ok(msg)
    }

    pub fn tool(tool_call_id: String, content: String) -> Result<Self, LlmError> {
        let mut msg = Self::new(MessageRole::Tool, content)?;
        msg.tool_call_id = Some(tool_call_id);
        Ok(msg)
    }

    pub fn role(&self) -> &MessageRole {
        &self.role
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn tool_call_id(&self) -> Option<&str> {
        self.tool_call_id.as_deref()
    }

    pub fn tool_calls(&self) -> Option<&[crate::llm::domain::ToolCall]> {
        self.tool_calls.as_deref()
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
