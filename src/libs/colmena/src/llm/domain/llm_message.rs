use crate::llm::domain::LlmError;
use crate::llm::domain::ProviderKind;
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
#[cfg_attr(test, derive(PartialEq))]
pub struct FileData {
    /// Identificador único enviado por el emisor. Requerido cuando `source` es `SignedUrl`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    pub mime_type: String,
    pub filename: String,
    /// Hint del campo `size_bytes` del JSON. No es ground truth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_hint: Option<u64>,
    pub source: FileSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileSource {
    /// Bytes ya en RAM (vino como `data` base64 < 30 MB, o `path` < 30 MB).
    InlineBytes { bytes: Vec<u8> },
    /// Signed URL pendiente de descarga + upload al provider.
    SignedUrl(String),
    /// Ya subido al provider.
    Uploaded(ProviderFileRef),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub struct ProviderFileRef {
    pub provider: ProviderKind,
    pub provider_file_id: String,
    pub mime_type: String,
    pub filename: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

impl FileData {
    /// Constructor retrocompatible: bytes ya en memoria.
    pub fn inline(mime_type: String, filename: String, bytes: Vec<u8>) -> Self {
        Self {
            document_id: None,
            mime_type,
            filename,
            size_hint: None,
            source: FileSource::InlineBytes { bytes },
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
    #[serde(skip_serializing_if = "Option::is_none")]
    files: Option<Vec<FileData>>,
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
            files: None,
            timestamp: Utc::now(),
        })
    }

    pub fn system(content: String) -> Result<Self, LlmError> {
        Self::new(MessageRole::System, content)
    }

    pub fn user(content: String) -> Result<Self, LlmError> {
        Self::new(MessageRole::User, content)
    }

    pub fn user_with_files(content: String, files: Vec<FileData>) -> Result<Self, LlmError> {
        let mut msg = Self::new(MessageRole::User, content)?;
        msg.files = Some(files);
        Ok(msg)
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

    pub fn files(&self) -> Option<&[FileData]> {
        self.files.as_deref()
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

    #[test]
    fn test_file_data_with_signed_url_source() {
        let file = FileData {
            document_id: Some("doc-123".to_string()),
            mime_type: "application/pdf".to_string(),
            filename: "report.pdf".to_string(),
            size_hint: Some(47_185_920),
            source: FileSource::SignedUrl("https://storage.googleapis.com/bucket/x?sig=abc".to_string()),
        };
        assert_eq!(file.document_id.as_deref(), Some("doc-123"));
        match &file.source {
            FileSource::SignedUrl(u) => assert!(u.contains("storage.googleapis.com")),
            _ => panic!("expected SignedUrl variant"),
        }
    }

    #[test]
    fn test_provider_file_ref_construction() {
        use crate::llm::domain::ProviderFileRef;
        use crate::llm::domain::ProviderKind;
        use chrono::Utc;
        let r = ProviderFileRef {
            provider: ProviderKind::Anthropic,
            provider_file_id: "file_abc".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            expires_at: None,
        };
        assert_eq!(r.provider_file_id, "file_abc");
        let _ = Utc::now();
    }
}
