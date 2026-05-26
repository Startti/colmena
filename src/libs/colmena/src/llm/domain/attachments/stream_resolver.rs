//! Plan A: AttachmentStreamResolver — port for resolving $attachment:<document_id>
//! to a StoredStream that consumer nodes can forward (e.g. http_request multipart).
//! Composes AttachmentRegistry (document_id → storage_key) and
//! OutputStorageRepository (storage_key → StoredStream).

use async_trait::async_trait;

use crate::llm::domain::attachments::AttachmentError;
use crate::storage::domain::storage_error::StorageError;
use crate::storage::domain::StoredStream;

#[derive(Debug, thiserror::Error)]
pub enum AttachmentResolveError {
    #[error("attachment not found: document_id={document_id}")]
    NotFound { document_id: String },

    #[error("attachment registered but storage_key is null (likely pre-migration row): document_id={document_id}")]
    StorageKeyMissing { document_id: String },

    #[error("attachment expired: document_id={document_id}")]
    Expired { document_id: String },

    #[error("storage error: {0}")]
    StorageError(#[from] StorageError),

    #[error("registry error: {0}")]
    RegistryError(#[from] AttachmentError),
}

#[async_trait]
pub trait AttachmentStreamResolver: Send + Sync {
    /// Given an agent session and a `document_id`, returns a `StoredStream`
    /// that the caller can forward to a downstream consumer (e.g. an HTTP
    /// multipart part). Updates `last_used_at` as a side effect.
    async fn resolve(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<StoredStream, AttachmentResolveError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_variants_are_distinct() {
        let nf = AttachmentResolveError::NotFound {
            document_id: "x".into(),
        };
        let exp = AttachmentResolveError::Expired {
            document_id: "x".into(),
        };
        assert_ne!(format!("{:?}", nf), format!("{:?}", exp));
    }
}
