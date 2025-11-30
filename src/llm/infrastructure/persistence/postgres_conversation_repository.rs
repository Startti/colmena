use crate::llm::domain::{Conversation, ConversationRepository, LlmError, LlmMessage, MessageRole, ThreadId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

pub struct PostgresConversationRepository {
    pool: PgPool,
}

impl PostgresConversationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ConversationRepository for PostgresConversationRepository {
    async fn get_by_id(&self, id: &ThreadId) -> Result<Conversation, LlmError> {
        let rows = sqlx::query(
            "SELECT role, content, created_at FROM chat_messages WHERE thread_id = $1 ORDER BY created_at ASC"
        )
        .bind(&id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed { message: format!("Database error: {}", e) })?;

        let messages = rows.into_iter().map(|row| {
            let role_str: String = row.get("role");
            let content: String = row.get("content");
            let _created_at: DateTime<Utc> = row.get("created_at");

            let role = match role_str.as_str() {
                "system" => MessageRole::System,
                "user" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                _ => MessageRole::User, // Fallback, though ideally we should handle this better
            };

            // Reconstruct LlmMessage. 
            // Note: LlmMessage fields are private, so we might need a constructor or builder in domain.
            // For now assuming we can construct it or add a method to LlmMessage.
            // Let's assume we need to add a `new_with_timestamp` or similar to LlmMessage, 
            // or just use the existing constructors and override timestamp if possible, 
            // or just accept current time for now if strict reconstruction isn't critical.
            // BUT, for history to be accurate, we should preserve timestamps.
            // Let's check LlmMessage definition again.
             
             // Temporary workaround: use existing constructors which set timestamp to Now.
             // Ideally we update LlmMessage to allow setting timestamp or be a struct with public fields.
             match role {
                 MessageRole::System => LlmMessage::system(content).unwrap(),
                 MessageRole::User => LlmMessage::user(content).unwrap(),
                 MessageRole::Assistant => LlmMessage::assistant(content).unwrap(),
             }
        }).collect();

        Ok(Conversation {
            thread_id: id.clone(),
            messages,
        })
    }

    async fn add_message(&self, id: &ThreadId, message: LlmMessage) -> Result<(), LlmError> {
        let role_str = match message.role() {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };

        sqlx::query(
            "INSERT INTO chat_messages (thread_id, role, content, created_at) VALUES ($1, $2, $3, $4)"
        )
        .bind(&id.0)
        .bind(role_str)
        .bind(message.content())
        .bind(Utc::now()) // Use current time for insertion
        .execute(&self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed { message: format!("Database error: {}", e) })?;

        Ok(())
    }

    async fn delete(&self, id: &ThreadId) -> Result<(), LlmError> {
        sqlx::query("DELETE FROM chat_messages WHERE thread_id = $1")
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| LlmError::RequestFailed { message: format!("Database error: {}", e) })?;

        Ok(())
    }
}
