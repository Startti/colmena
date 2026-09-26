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

/// Max rows a single `list_node_activity` call returns for one `node_id_prefix`.
/// Enumeration is model-driven (one row per invented thread id) and each row
/// carries an `opening` snippet, so an unbounded result can inject tens of KB
/// into a single tool result. Mirrors the pagination discipline of the sibling
/// `recall_history` tool. Shared by every `ConversationRepository` backend.
pub const MAX_LISTED_NODE_ACTIVITY: i64 = 100;

/// The path segment a tool's memory key hangs from: `tool/<name>…` from a
/// root-level caller, `<caller>/tool/<name>…` from a caller inside a
/// tool-invoked child, whose own path starts with `tool/`. Every rule that
/// builds or recognizes those keys reads it from here: the key builder and
/// its nested-caller check (`memory_node_path`), [`is_nested_tool_memory`],
/// and the `NOT LIKE` pattern of the SQL backends' `list_node_activity`.
/// Those put it in the pattern unescaped, so it holds no LIKE metacharacter
/// (`%`, `_`, `\`), and no `/`, being one segment.
pub const TOOL_MEMORY_SEGMENT: &str = "tool";

/// Whether a `node_id` listed under a thread prefix is the memory of a tool
/// called from inside that thread rather than the thread's own conversation.
/// `rest` is the `node_id` after the prefix. A caller inside a tool-invoked
/// child keys its tools under its own path (`<caller>/tool/<name>…`), so
/// such rows hold a `/tool/` segment ([`TOOL_MEMORY_SEGMENT`]). Known edge:
/// a child node whose id is literally `tool` also holds one.
pub fn is_nested_tool_memory(rest: &str) -> bool {
    rest.contains(&format!("/{TOOL_MEMORY_SEGMENT}/"))
}

/// Per-`node_id` activity summary for thread enumeration (`list_threads`).
#[derive(Debug, Clone)]
pub struct NodeActivity {
    pub node_id: String,
    pub message_count: i64,
    // Backend-formatted timestamp string (not a guaranteed ISO-8601 UTC format):
    // Postgres renders `max(created_at)::text` per the session TimeZone with a
    // space separator (e.g. "2026-08-24 10:00:00+00"), while SQLite/in-memory
    // produce RFC 3339. Comparable/sortable within a single backend only.
    pub last_activity: String,
    pub opening: Option<String>, // earliest `user` message content
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

    /// List per-`node_id` activity for every `node_id` starting with `node_id_prefix`,
    /// keyed by `keying` (("agent_session_id"|"session_id", value)), leaving out
    /// the memory of tools called from inside a listed thread
    /// ([`is_nested_tool_memory`]) before the [`MAX_LISTED_NODE_ACTIVITY`] cap.
    /// Backends override; the default returns empty so non-DB stubs stay valid.
    async fn list_node_activity(
        &self,
        keying: (&str, &str),
        node_id_prefix: &str,
    ) -> Result<Vec<NodeActivity>, LlmError> {
        let _ = (keying, node_id_prefix);
        Ok(Vec::new())
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

    /// The SQL backends put the segment in a LIKE pattern unescaped.
    #[test]
    fn the_tool_memory_segment_is_one_segment_with_no_like_metacharacter() {
        assert!(!TOOL_MEMORY_SEGMENT.is_empty());
        assert!(!TOOL_MEMORY_SEGMENT.contains(['%', '_', '\\', '/']));
    }
}
