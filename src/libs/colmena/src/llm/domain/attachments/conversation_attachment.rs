use crate::llm::domain::ProviderKind;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where the attachment was originally sourced. Drives expiry-recovery
/// strategy: `SignedUrl` and `Path` can be re-uploaded; `Inline` cannot
/// because we deliberately do not retain raw bytes after the first upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AttachmentSource {
    SignedUrl(String),
    Path(String),
    Inline,
}

impl AttachmentSource {
    pub fn kind_str(&self) -> &'static str {
        match self {
            AttachmentSource::SignedUrl(_) => "signed_url",
            AttachmentSource::Path(_) => "path",
            AttachmentSource::Inline => "inline",
        }
    }

    pub fn value(&self) -> Option<&str> {
        match self {
            AttachmentSource::SignedUrl(v) | AttachmentSource::Path(v) => Some(v),
            AttachmentSource::Inline => None,
        }
    }

    pub fn is_recoverable(&self) -> bool {
        !matches!(self, AttachmentSource::Inline)
    }
}

/// Represents an attachment registered in a conversation, with metadata
/// and reference to its location in provider storage.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationAttachment {
    pub agent_session_id: String,
    pub document_id: String,
    pub provider: ProviderKind,
    pub provider_file_id: String,
    pub mime_type: String,
    pub filename: String,
    pub size_bytes: Option<u64>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub source: AttachmentSource,
    pub registered_at: DateTime<Utc>,
    pub refreshed_at: DateTime<Utc>,
}

impl ConversationAttachment {
    /// Catalog rendering for the load_attachment tool description.
    /// Format: `"<doc_id>" — <label or filename> (<mime>, <size>)[. <description>]`
    pub fn catalog_line(&self) -> String {
        let label = self.label.as_deref().unwrap_or(self.filename.as_str());
        let size = self
            .size_bytes
            .map(human_size)
            .unwrap_or_else(|| "?".to_string());
        let mut line = format!(
            "\"{}\" — {} ({}, {})",
            self.document_id, label, self.mime_type, size
        );
        if let Some(desc) = &self.description {
            if !desc.trim().is_empty() {
                line.push_str(". ");
                line.push_str(desc.trim());
            }
        }
        line
    }
}

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(
        label: Option<&str>,
        description: Option<&str>,
        size: Option<u64>,
    ) -> ConversationAttachment {
        ConversationAttachment {
            agent_session_id: "agent_1".to_string(),
            document_id: "doc-abc".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "Q3.pdf".to_string(),
            size_bytes: size,
            label: label.map(String::from),
            description: description.map(String::from),
            source: AttachmentSource::SignedUrl("https://x".to_string()),
            registered_at: Utc::now(),
            refreshed_at: Utc::now(),
        }
    }

    #[test]
    fn source_kind_str_matches_serialized_form() {
        assert_eq!(
            AttachmentSource::SignedUrl("u".into()).kind_str(),
            "signed_url"
        );
        assert_eq!(AttachmentSource::Path("/p".into()).kind_str(), "path");
        assert_eq!(AttachmentSource::Inline.kind_str(), "inline");
    }

    #[test]
    fn inline_source_is_not_recoverable() {
        assert!(!AttachmentSource::Inline.is_recoverable());
        assert!(AttachmentSource::SignedUrl("x".into()).is_recoverable());
        assert!(AttachmentSource::Path("x".into()).is_recoverable());
    }

    #[test]
    fn catalog_line_uses_label_when_present() {
        let a = mk(Some("Q3 Financial Report"), None, Some(12 * 1024 * 1024));
        let line = a.catalog_line();
        assert!(line.contains("Q3 Financial Report"));
        assert!(line.contains("application/pdf"));
        assert!(line.contains("12.0 MB"));
        assert!(line.contains("\"doc-abc\""));
    }

    #[test]
    fn catalog_line_falls_back_to_filename_without_label() {
        let a = mk(None, None, Some(2048));
        assert!(a.catalog_line().contains("Q3.pdf"));
        assert!(a.catalog_line().contains("2.0 KB"));
    }

    #[test]
    fn catalog_line_appends_description_when_present() {
        let a = mk(Some("Report"), Some("Q3 2026 results"), Some(1024));
        assert!(a.catalog_line().contains(". Q3 2026 results"));
    }

    #[test]
    fn unknown_size_renders_as_question_mark() {
        let a = mk(Some("X"), None, None);
        assert!(a.catalog_line().contains("?"));
    }
}
