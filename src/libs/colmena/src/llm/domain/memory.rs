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

impl ConversationKey {
    /// Resolves this key's precedence into `(column, value)` for history
    /// queries: `agent_session_id` when present, else `session_id`.
    ///
    /// The returned column is a FIXED identifier drawn from a closed
    /// 2-element set (`"agent_session_id"` | `"session_id"`) — it is never
    /// user- or LLM-supplied, so interpolating it into SQL via `format!` is
    /// safe and is NOT an audit-#26 (dynamic SQL identifier injection)
    /// violation. The returned value is always passed to the query as a
    /// bound parameter, never interpolated.
    pub fn keying(&self) -> (&'static str, &str) {
        match &self.agent_session_id {
            Some(agent) => ("agent_session_id", agent.0.as_str()),
            None => ("session_id", self.session_id.0.as_str()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Conversation {
    pub key: ConversationKey,
    pub messages: Vec<LlmMessage>,
}

/// Un mensaje persistido junto con su resumen cacheado (si existe).
/// `summary == None` → aún no resumido (o por debajo del umbral → verbatim).
#[derive(Debug, Clone)]
pub struct StoredMessage {
    pub message: LlmMessage,
    pub summary: Option<String>,
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
    async fn add_message(&self, key: &ConversationKey, message: LlmMessage)
        -> Result<(), LlmError>;

    /// Deletes all messages for the given thread (matches the same filter as `get_by_id`).
    async fn delete(&self, key: &ConversationKey) -> Result<(), LlmError>;

    /// Como `get_by_id`, pero devuelve cada mensaje junto a su `summary` cacheado.
    /// Default: delega en `get_by_id` con summaries en `None` (impls de DB lo overridean).
    async fn get_with_summaries(
        &self,
        key: &ConversationKey,
    ) -> Result<Vec<StoredMessage>, LlmError> {
        let conv = self.get_by_id(key).await?;
        Ok(conv
            .messages
            .into_iter()
            .map(|message| StoredMessage {
                message,
                summary: None,
            })
            .collect())
    }

    /// Persiste el `summary` del mensaje en la posición `ordinal` (0-based en orden `created_at`).
    /// Default: no-op (impls de DB lo overridean).
    async fn set_summary(
        &self,
        _key: &ConversationKey,
        _ordinal: usize,
        _summary: &str,
    ) -> Result<(), LlmError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keying_prefers_agent_session_id_when_present() {
        let key = ConversationKey {
            session_id: SessionId("sess_y".into()),
            agent_session_id: Some(AgentSessionId("agent_x".into())),
            node_id: NodeIdPath("n".into()),
        };
        assert_eq!(key.keying(), ("agent_session_id", "agent_x"));
    }

    #[test]
    fn keying_falls_back_to_session_id_when_agent_session_id_absent() {
        let key = ConversationKey {
            session_id: SessionId("sess_y".into()),
            agent_session_id: None,
            node_id: NodeIdPath("n".into()),
        };
        assert_eq!(key.keying(), ("session_id", "sess_y"));
    }
}
