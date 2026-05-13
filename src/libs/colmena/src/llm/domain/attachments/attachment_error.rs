use thiserror::Error;

/// Error type for attachment-related operations.
#[derive(Debug, Error)]
pub enum AttachmentError {
    /// Attachment not found in the session.
    #[error("attachment '{document_id}' not found in session")]
    NotFound { document_id: String },

    /// Attachment expired and cannot be re-uploaded.
    #[error("attachment '{document_id}' expired and cannot be re-uploaded: {reason}")]
    ExpiredUnrecoverable { document_id: String, reason: String },

    /// Agent session ID is missing from the run.
    #[error(
        "agent_session_id is missing from the run; load_attachment requires a stable agent session"
    )]
    SessionMissing,

    /// Repository operation failed.
    #[error("repository failure: {0}")]
    RepositoryFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_renders_document_id_in_message() {
        let e = AttachmentError::NotFound {
            document_id: "doc-x".to_string(),
        };
        assert_eq!(format!("{}", e), "attachment 'doc-x' not found in session");
    }

    #[test]
    fn expired_unrecoverable_renders_reason() {
        let e = AttachmentError::ExpiredUnrecoverable {
            document_id: "doc-y".to_string(),
            reason: "inline bytes not retained".to_string(),
        };
        assert!(format!("{}", e).contains("inline bytes not retained"));
    }
}
