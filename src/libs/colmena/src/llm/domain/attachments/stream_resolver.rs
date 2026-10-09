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
    #[error("attachment not found: document_id={document_id}; use a document_id from the attachments catalog")]
    NotFound { document_id: String },

    /// The row the lookup picks has no `storage_key` — happens for legacy
    /// rows registered before Plan A (when only the provider id was stored,
    /// with no local copy) and for the newest upload of an id whose bytes
    /// failed to persist (it stays readable through its provider file id).
    /// These rows cannot be re-streamed; the LLM should re-attach the document.
    #[error("attachment registered but its bytes were not stored: document_id={document_id}")]
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

    /// The row references an object the HOST owns and the caller would read it
    /// whole into memory (see
    /// [`resolve_for_buffering`](AttachmentStreamResolver::resolve_for_buffering)).
    /// Displays the large-file refusal text.
    #[error("{}", crate::llm::domain::large_tabular::refusal_text_for_tool(*large_tool))]
    HostObject {
        large_tool: Option<crate::llm::domain::large_tabular::LargeTool>,
    },
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

    /// [`resolve`](Self::resolve) for a caller that will hold the WHOLE stream in
    /// memory (the JSON body of `http_request`, `image_edit`). A row that
    /// references an object the HOST owns is refused with
    /// [`AttachmentResolveError::HostObject`]; streamed uses (`resolve`, a read
    /// URL) stay allowed. The default is for resolvers that know no host rows.
    async fn resolve_for_buffering(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<StoredStream, AttachmentResolveError> {
        self.resolve(agent_session_id, document_id).await
    }

    /// A read URL for `document_id` of `agent_session_id`, valid for about
    /// `ttl_seconds`: the same session lookup as [`Self::resolve`] (a raw
    /// storage_key or another session's id is `NotFound`, and storage is
    /// never asked), then `OutputStorageRepository::read_url` with the row's
    /// `storage_key`. `Ok(None)`: the host's storage issues no read URLs.
    ///
    /// Default `Ok(None)`, so an implementer that predates it compiles and
    /// behaves as a host without URLs.
    async fn resolve_url(
        &self,
        agent_session_id: &str,
        document_id: &str,
        ttl_seconds: u64,
    ) -> Result<Option<String>, AttachmentResolveError> {
        let _ = (agent_session_id, document_id, ttl_seconds);
        Ok(None)
    }
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
