use crate::llm::domain::{LlmError, LlmMessage};
use async_trait::async_trait;

/// Value Object that identifies the run scope of a single message.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

/// Value Object that identifies the conversation a message belongs to.
/// `None` means the message belongs only to a single run (legacy mode).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentSessionId(pub String);

/// Path-qualified node identifier (e.g., "router" or "ventas/responder").
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeIdPath(pub String);

/// Identifies a single LLM thread to read/write history for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationKey {
    pub session_id: SessionId,
    pub agent_session_id: Option<AgentSessionId>,
    pub node_id: NodeIdPath,
}

#[derive(Debug, Clone)]
pub struct Conversation {
    pub key: ConversationKey,
    pub messages: Vec<LlmMessage>,
}

#[async_trait]
pub trait ConversationRepository: Send + Sync {
    /// Loads all messages for the given thread.
    /// When `key.agent_session_id` is `Some`, filters by `(agent_session_id, node_id)`.
    /// When `None`, falls back to `(session_id, node_id)`.
    async fn get_by_id(&self, key: &ConversationKey) -> Result<Conversation, LlmError>;

    /// Appends a single message to the thread.
    /// Always writes `session_id` and `node_id`; `agent_session_id` is written
    /// when present.
    async fn add_message(
        &self,
        key: &ConversationKey,
        message: LlmMessage,
    ) -> Result<(), LlmError>;

    /// Deletes all messages for the given thread (matches the same filter as `get_by_id`).
    async fn delete(&self, key: &ConversationKey) -> Result<(), LlmError>;
}
