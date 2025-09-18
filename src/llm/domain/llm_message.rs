use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "system" => Ok(MessageRole::System),
            "user" => Ok(MessageRole::User),
            "assistant" => Ok(MessageRole::Assistant),
            _ => Err(format!("Invalid message role: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    role: MessageRole,
    content: String,
    timestamp: DateTime<Utc>,
}

impl LlmMessage {
    pub fn new(role: MessageRole, content: String) -> Result<Self, String> {
        if content.trim().is_empty() {
            return Err("Message content cannot be empty".to_string());
        }

        Ok(Self {
            role,
            content: content.trim().to_string(),
            timestamp: Utc::now(),
        })
    }

    pub fn system(content: String) -> Result<Self, String> {
        Self::new(MessageRole::System, content)
    }

    pub fn user(content: String) -> Result<Self, String> {
        Self::new(MessageRole::User, content)
    }

    pub fn assistant(content: String) -> Result<Self, String> {
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