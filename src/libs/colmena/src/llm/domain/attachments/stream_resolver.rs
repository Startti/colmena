//! Plan A: AttachmentStreamResolver — port for resolving $attachment:<document_id>
//! to a StoredStream that consumer nodes can forward (e.g. http_request multipart).
//! Composes AttachmentRegistry (document_id → storage_key) and
//! OutputStorageRepository (storage_key → StoredStream).

use async_trait::async_trait;

use crate::llm::domain::attachments::AttachmentError;
use crate::storage::domain::storage_error::StorageError;
use crate::storage::domain::StoredStream;

/// Errors returned by [`AttachmentStreamResolver::resolve`].
///
/// Variants distinguish *catalog-level* failures (`NotFound`, `Expired`,
/// `StorageKeyMissing`) — where the attachment registry knows about (or has
/// forgotten) the document — from *infra-level* failures (`StorageError`,
/// `RegistryError`) that propagate up from the underlying adapter. Callers
/// typically want to surface catalog errors as 4xx-equivalents to the LLM
/// (so it can retry with a different `document_id`) and infra errors as 5xx.
#[derive(Debug, thiserror::Error)]
pub enum AttachmentResolveError {
    /// No row in `conversation_attachments` matches the `(agent_session_id,
    /// document_id)` pair — either the id was hallucinated by the LLM or the
    /// row was GC'd (see Plan C).
    #[error("attachment not found: document_id={document_id}")]
    NotFound { document_id: String },

    /// Row exists but `storage_key` is `NULL` — happens for legacy rows
    /// registered before Plan A (when only the provider id was stored, with
    /// no local copy). These rows cannot be re-streamed; the LLM should
    /// re-attach the document.
    #[error("attachment registered but storage_key is null (likely pre-migration row): document_id={document_id}")]
    StorageKeyMissing { document_id: String },

    /// Row exists but the registry has marked it expired (TTL elapsed or
    /// explicit revocation). The backing blob may still exist in storage but
    /// the catalog refuses to hand it out.
    #[error("attachment expired: document_id={document_id}")]
    Expired { document_id: String },

    /// The underlying `OutputStorageRepository` (GCS, local cache, local
    /// HTTP, callback) failed to open the stream — network, permission, or
    /// missing-blob error. Distinct from `NotFound`: the catalog has the row
    /// but storage cannot serve the bytes.
    #[error("storage error: {0}")]
    StorageError(#[from] StorageError),

    /// The `AttachmentRegistry` query failed (DB connection, query error,
    /// etc.). Catalog state is unknown.
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
