use crate::llm::domain::{
    Conversation, ConversationKey, ConversationRepository, LlmError, LlmMessage, StoredMessage,
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
}
