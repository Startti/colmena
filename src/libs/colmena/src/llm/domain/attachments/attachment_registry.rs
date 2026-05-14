use crate::llm::domain::attachments::{AttachmentError, AttachmentSource, ConversationAttachment};
use crate::llm::domain::ProviderKind;
use async_trait::async_trait;

/// Input record for `AttachmentRegistry::upsert`. Mirrors the columns of the
/// `conversation_attachments` table 1:1.
#[derive(Debug, Clone)]
pub struct UpsertAttachmentInput {
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
}

#[async_trait]
pub trait AttachmentRegistry: Send + Sync {
    /// Insert or update a registry entry. Idempotent on
    /// `(agent_session_id, document_id, provider)`.
    async fn upsert(&self, input: UpsertAttachmentInput) -> Result<(), AttachmentError>;

    /// Fetch a single entry for the given session + document.
    /// Returns `Ok(None)` when nothing is registered.
    async fn lookup(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
    ) -> Result<Option<ConversationAttachment>, AttachmentError>;

    /// Replace the `provider_file_id` (and `refreshed_at`) for an existing row.
    /// Returns `Err(NotFound)` when the row does not exist.
    async fn refresh_provider_file_id(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
        new_provider_file_id: &str,
    ) -> Result<(), AttachmentError>;

    /// Replace the `description` for an existing row. Used by the auto-summary
    /// generator to persist the produced summary after the upsert. Returns
    /// `Err(NotFound)` when the row does not exist.
    async fn update_description(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
        description: &str,
    ) -> Result<(), AttachmentError>;

    /// List every entry registered for the given session. Used to build the
    /// `load_attachment` catalog at the start of an llm_call execute. Filtering
    /// by provider happens in the caller (one provider per llm_call execution).
    async fn list_for_session(
        &self,
        agent_session_id: &str,
    ) -> Result<Vec<ConversationAttachment>, AttachmentError>;
}
