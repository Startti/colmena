use crate::llm::domain::{Conversation, ConversationRepository, LlmError, LlmMessage, MessageRole, ThreadId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{SqlitePool, Row};

pub struct SqliteConversationRepository {
    pool: SqlitePool,
}

impl SqliteConversationRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ConversationRepository for SqliteConversationRepository {
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
                _ => MessageRole::User,
            };

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

        let msg_id = uuid::Uuid::new_v4().to_string();
        let created_at = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO chat_messages (id, thread_id, role, content, created_at) VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(msg_id)
        .bind(&id.0)
        .bind(role_str)
        .bind(message.content())
        .bind(created_at)
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
