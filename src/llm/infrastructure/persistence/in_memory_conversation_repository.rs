use crate::llm::domain::{Conversation, ConversationRepository, LlmError, LlmMessage, ThreadId};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// In-memory conversation repository that doesn't persist to disk
/// Useful for stateless LLM calls or testing
#[derive(Clone)]
pub struct InMemoryConversationRepository {
    conversations: Arc<RwLock<HashMap<String, Vec<LlmMessage>>>>,
}

impl InMemoryConversationRepository {
    pub fn new() -> Self {
        Self {
            conversations: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryConversationRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConversationRepository for InMemoryConversationRepository {
    async fn get_by_id(&self, id: &ThreadId) -> Result<Conversation, LlmError> {
        let conversations = self.conversations.read().unwrap();
        let messages = conversations.get(&id.0).cloned().unwrap_or_default();

        Ok(Conversation {
            thread_id: id.clone(),
            messages,
        })
    }

    async fn add_message(&self, id: &ThreadId, message: LlmMessage) -> Result<(), LlmError> {
        let mut conversations = self.conversations.write().unwrap();
        conversations.entry(id.0.clone()).or_default().push(message);
        Ok(())
    }

    async fn delete(&self, id: &ThreadId) -> Result<(), LlmError> {
        let mut conversations = self.conversations.write().unwrap();
        conversations.remove(&id.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_add_and_get_messages() {
        let repo = InMemoryConversationRepository::new();
        let thread_id = ThreadId("test_thread".to_string());

        // Add messages
        let msg1 = LlmMessage::user("Hello".to_string()).unwrap();
        let msg2 = LlmMessage::assistant("Hi there!".to_string()).unwrap();

        repo.add_message(&thread_id, msg1.clone()).await.unwrap();
        repo.add_message(&thread_id, msg2.clone()).await.unwrap();

        // Retrieve conversation
        let conversation = repo.get_by_id(&thread_id).await.unwrap();
        assert_eq!(conversation.messages.len(), 2);
        assert_eq!(conversation.messages[0].content(), "Hello");
        assert_eq!(conversation.messages[1].content(), "Hi there!");
    }

    #[tokio::test]
    async fn test_delete_conversation() {
        let repo = InMemoryConversationRepository::new();
        let thread_id = ThreadId("test_thread".to_string());

        // Add a message
        let msg = LlmMessage::user("Hello".to_string()).unwrap();
        repo.add_message(&thread_id, msg).await.unwrap();

        // Delete conversation
        repo.delete(&thread_id).await.unwrap();

        // Verify it's gone
        let conversation = repo.get_by_id(&thread_id).await.unwrap();
        assert!(conversation.messages.is_empty());
    }

    #[tokio::test]
    async fn test_get_nonexistent_conversation() {
        let repo = InMemoryConversationRepository::new();
        let thread_id = ThreadId("nonexistent".to_string());

        let conversation = repo.get_by_id(&thread_id).await.unwrap();
        assert!(conversation.messages.is_empty());
    }
}
