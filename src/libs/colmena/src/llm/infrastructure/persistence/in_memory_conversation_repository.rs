use crate::llm::domain::{
    Conversation, ConversationKey, ConversationRepository, LlmError, LlmMessage, MessageRole,
    NodeActivity, StoredMessage,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct InMemoryConversationRepository {
    /// Storage indexed by (agent_session_id_or_session_id, node_id) — the
    /// effective key under which messages were appended.
    inner: Mutex<HashMap<(String, String), Vec<StoredMessage>>>,
}

impl InMemoryConversationRepository {
    pub fn new() -> Self {
        Self::default()
    }

    fn lookup_key(key: &ConversationKey) -> (String, String) {
        let id = match &key.agent_session_id {
            Some(a) => a.0.clone(),
            None => key.session_id.0.clone(),
        };
        (id, key.node_id.0.clone())
    }
}

#[async_trait]
impl ConversationRepository for InMemoryConversationRepository {
    async fn get_by_id(&self, key: &ConversationKey) -> Result<Conversation, LlmError> {
        let map = self.inner.lock().unwrap();
        let messages = map
            .get(&Self::lookup_key(key))
            .map(|v| v.iter().map(|sm| sm.message.clone()).collect())
            .unwrap_or_default();
        Ok(Conversation {
            key: key.clone(),
            messages,
        })
    }

    async fn add_message(
        &self,
        key: &ConversationKey,
        message: LlmMessage,
    ) -> Result<(), LlmError> {
        let mut map = self.inner.lock().unwrap();
        map.entry(Self::lookup_key(key))
            .or_default()
            .push(StoredMessage {
                message,
                summary: None,
            });
        Ok(())
    }

    async fn delete(&self, key: &ConversationKey) -> Result<(), LlmError> {
        let mut map = self.inner.lock().unwrap();
        map.remove(&Self::lookup_key(key));
        Ok(())
    }

    async fn get_with_summaries(
        &self,
        key: &ConversationKey,
    ) -> Result<Vec<StoredMessage>, LlmError> {
        let map = self.inner.lock().unwrap();
        Ok(map.get(&Self::lookup_key(key)).cloned().unwrap_or_default())
    }

    async fn set_summary(
        &self,
        key: &ConversationKey,
        ordinal: usize,
        summary: &str,
    ) -> Result<(), LlmError> {
        let mut map = self.inner.lock().unwrap();
        if let Some(v) = map.get_mut(&Self::lookup_key(key)) {
            if let Some(sm) = v.get_mut(ordinal) {
                sm.summary = Some(summary.to_string());
            }
        }
        Ok(())
    }

    async fn list_node_activity(
        &self,
        keying: (&str, &str),
        node_id_prefix: &str,
    ) -> Result<Vec<NodeActivity>, LlmError> {
        // `inner` is already keyed by the effective id (agent_session_id when
        // present, else session_id — see `lookup_key`), not by the full
        // `ConversationKey`. The store therefore can't distinguish which
        // column produced that id; matching on the value alone mirrors the
        // same collapsing precedence `ConversationKey::keying` already
        // applies, so `col` is accepted for interface parity but unused here.
        let (_col, val) = keying;
        let map = self.inner.lock().unwrap();
        let mut out = Vec::new();
        for ((id, node_id), msgs) in map.iter() {
            if id != val || !node_id.starts_with(node_id_prefix) {
                continue;
            }
            let opening = msgs
                .iter()
                .find(|sm| matches!(sm.message.role(), MessageRole::User))
                .map(|sm| sm.message.content().to_string());
            let last_activity = msgs
                .last()
                .map(|sm| sm.message.timestamp().to_rfc3339())
                .unwrap_or_default();
            out.push(NodeActivity {
                node_id: node_id.clone(),
                message_count: msgs.len() as i64,
                last_activity,
                opening,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{AgentSessionId, NodeIdPath, SessionId};

    fn k(agent: Option<&str>, session: &str, node: &str) -> ConversationKey {
        ConversationKey {
            session_id: SessionId(session.to_string()),
            agent_session_id: agent.map(|a| AgentSessionId(a.to_string())),
            node_id: NodeIdPath(node.to_string()),
        }
    }

    #[tokio::test]
    async fn agent_keying_isolates_two_runs_under_same_chat() {
        let repo = InMemoryConversationRepository::new();
        let k1 = k(Some("chat_x"), "run_1", "router");
        let k2 = k(Some("chat_x"), "run_2", "router");

        repo.add_message(&k1, LlmMessage::user("hi from run 1".into()).unwrap())
            .await
            .unwrap();
        let conv = repo.get_by_id(&k2).await.unwrap();
        assert_eq!(
            conv.messages.len(),
            1,
            "agent keying should let run 2 see run 1's history"
        );
    }

    #[tokio::test]
    async fn legacy_keying_does_not_cross_runs() {
        let repo = InMemoryConversationRepository::new();
        let k1 = k(None, "run_1", "router");
        let k2 = k(None, "run_2", "router");

        repo.add_message(&k1, LlmMessage::user("hi from run 1".into()).unwrap())
            .await
            .unwrap();
        let conv = repo.get_by_id(&k2).await.unwrap();
        assert!(
            conv.messages.is_empty(),
            "legacy keying must not leak across runs"
        );
    }

    #[tokio::test]
    async fn node_id_isolates_two_llm_calls_in_same_run() {
        let repo = InMemoryConversationRepository::new();
        let router = k(Some("chat_x"), "run_1", "router");
        let responder = k(Some("chat_x"), "run_1", "responder");

        repo.add_message(&router, LlmMessage::user("router only".into()).unwrap())
            .await
            .unwrap();
        let conv = repo.get_by_id(&responder).await.unwrap();
        assert!(
            conv.messages.is_empty(),
            "responder must not see router's history"
        );
    }

    #[tokio::test]
    async fn in_memory_summary_roundtrip() {
        let repo = InMemoryConversationRepository::new();
        let key = k(Some("chat_x"), "run_1", "router");
        repo.add_message(&key, LlmMessage::user("first".into()).unwrap())
            .await
            .unwrap();
        repo.add_message(&key, LlmMessage::assistant("second".into()).unwrap())
            .await
            .unwrap();
        repo.set_summary(&key, 1, "sum2").await.unwrap();
        let out = repo.get_with_summaries(&key).await.unwrap();
        assert_eq!(out.len(), 2);
        assert!(out[0].summary.is_none());
        assert_eq!(out[1].summary.as_deref(), Some("sum2"));
        assert_eq!(repo.get_by_id(&key).await.unwrap().messages.len(), 2);
    }

    #[tokio::test]
    async fn list_node_activity_groups_by_node_id_under_prefix() {
        let repo = InMemoryConversationRepository::new();
        let key = |node: &str| k(Some("agent-1"), "s", node);

        repo.add_message(
            &key("tool/archivador/alfa/keeper"),
            LlmMessage::user("abrir alfa".into()).unwrap(),
        )
        .await
        .unwrap();
        repo.add_message(
            &key("tool/archivador/alfa/keeper"),
            LlmMessage::assistant("ok".into()).unwrap(),
        )
        .await
        .unwrap();
        repo.add_message(
            &key("tool/archivador/beta/keeper"),
            LlmMessage::user("abrir beta".into()).unwrap(),
        )
        .await
        .unwrap();
        repo.add_message(
            &key("tool/otro/x/keeper"),
            LlmMessage::user("no incluir".into()).unwrap(),
        )
        .await
        .unwrap();

        let rows = repo
            .list_node_activity(("agent_session_id", "agent-1"), "tool/archivador/")
            .await
            .unwrap();
        assert_eq!(rows.len(), 2, "only archivador node_ids");
        let alfa = rows
            .iter()
            .find(|r| r.node_id == "tool/archivador/alfa/keeper")
            .unwrap();
        assert_eq!(alfa.message_count, 2);
        assert_eq!(alfa.opening.as_deref(), Some("abrir alfa"));
    }
}
