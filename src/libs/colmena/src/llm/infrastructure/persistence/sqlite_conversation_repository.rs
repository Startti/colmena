use super::hydration::hydrate_message;
use crate::llm::domain::{
    Conversation, ConversationKey, ConversationRepository, LlmError, LlmMessage, MessageRole,
    StoredMessage,
};

use async_trait::async_trait;
use chrono::Utc;
use sqlx::{Row, SqlitePool};

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
    async fn get_by_id(&self, key: &ConversationKey) -> Result<Conversation, LlmError> {
        let rows = if let Some(agent) = &key.agent_session_id {
            sqlx::query(
                "SELECT role, content, tool_call_id, tool_calls, created_at \
                 FROM llm_node_history \
                 WHERE agent_session_id = ? AND node_id = ? \
                 ORDER BY created_at ASC, id ASC",
            )
            .bind(&agent.0)
            .bind(&key.node_id.0)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT role, content, tool_call_id, tool_calls, created_at \
                 FROM llm_node_history \
                 WHERE session_id = ? AND node_id = ? \
                 ORDER BY created_at ASC, id ASC",
            )
            .bind(&key.session_id.0)
            .bind(&key.node_id.0)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| LlmError::RequestFailed {
            message: format!("Database error: {}", e),
        })?;

        let messages = rows
            .into_iter()
            .filter_map(|row| {
                let role_str: String = row.get("role");
                let content: String = row.get("content");
                let tool_call_id: Option<String> = row.get("tool_call_id");
                let tool_calls_str: Option<String> = row.get("tool_calls");

                let tool_calls =
                    tool_calls_str.map(|tc_str| serde_json::from_str(&tc_str).unwrap_or_default());

                hydrate_message(&role_str, content, tool_call_id, tool_calls)
            })
            .collect();

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
        let role_str = match message.role() {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };

        let tool_calls_str = message
            .tool_calls()
            .and_then(|tc| serde_json::to_string(tc).ok());

        let id = uuid::Uuid::new_v4().to_string();

        sqlx::query(
            "INSERT INTO llm_node_history (\
                id, session_id, agent_session_id, node_id, \
                role, content, tool_call_id, tool_calls, created_at\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&key.session_id.0)
        .bind(key.agent_session_id.as_ref().map(|a| a.0.clone()))
        .bind(&key.node_id.0)
        .bind(role_str)
        .bind(message.content())
        .bind(message.tool_call_id())
        .bind(tool_calls_str)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed {
            message: format!("Database error: {}", e),
        })?;

        Ok(())
    }

    async fn delete(&self, key: &ConversationKey) -> Result<(), LlmError> {
        let res = if let Some(agent) = &key.agent_session_id {
            sqlx::query("DELETE FROM llm_node_history WHERE agent_session_id = ? AND node_id = ?")
                .bind(&agent.0)
                .bind(&key.node_id.0)
                .execute(&self.pool)
                .await
        } else {
            sqlx::query("DELETE FROM llm_node_history WHERE session_id = ? AND node_id = ?")
                .bind(&key.session_id.0)
                .bind(&key.node_id.0)
                .execute(&self.pool)
                .await
        };

        res.map_err(|e| LlmError::RequestFailed {
            message: format!("Database error: {}", e),
        })?;
        Ok(())
    }

    async fn get_with_summaries(
        &self,
        key: &ConversationKey,
    ) -> Result<Vec<StoredMessage>, LlmError> {
        let rows = if let Some(agent) = &key.agent_session_id {
            sqlx::query(
                "SELECT role, content, tool_call_id, tool_calls, summary \
                 FROM llm_node_history \
                 WHERE agent_session_id = ? AND node_id = ? \
                 ORDER BY created_at ASC, id ASC",
            )
            .bind(&agent.0)
            .bind(&key.node_id.0)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT role, content, tool_call_id, tool_calls, summary \
                 FROM llm_node_history \
                 WHERE session_id = ? AND node_id = ? \
                 ORDER BY created_at ASC, id ASC",
            )
            .bind(&key.session_id.0)
            .bind(&key.node_id.0)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| LlmError::RequestFailed {
            message: format!("Database error: {}", e),
        })?;

        let stored = rows
            .into_iter()
            .filter_map(|row| {
                let role_str: String = row.get("role");
                let content: String = row.get("content");
                let tool_call_id: Option<String> = row.get("tool_call_id");
                let tool_calls_str: Option<String> = row.get("tool_calls");
                let summary: Option<String> = row.get("summary");

                let tool_calls =
                    tool_calls_str.map(|tc_str| serde_json::from_str(&tc_str).unwrap_or_default());

                let message = hydrate_message(&role_str, content, tool_call_id, tool_calls)?;
                Some(StoredMessage { message, summary })
            })
            .collect();

        Ok(stored)
    }

    async fn set_summary(
        &self,
        key: &ConversationKey,
        ordinal: usize,
        summary: &str,
    ) -> Result<(), LlmError> {
        if let Some(agent) = &key.agent_session_id {
            sqlx::query(
                "UPDATE llm_node_history SET summary = ? \
                 WHERE id = (\
                     SELECT id FROM llm_node_history \
                     WHERE agent_session_id = ? AND node_id = ? \
                     ORDER BY created_at ASC, id ASC LIMIT 1 OFFSET ?\
                 )",
            )
            .bind(summary)
            .bind(&agent.0)
            .bind(&key.node_id.0)
            .bind(ordinal as i64)
            .execute(&self.pool)
            .await
        } else {
            sqlx::query(
                "UPDATE llm_node_history SET summary = ? \
                 WHERE id = (\
                     SELECT id FROM llm_node_history \
                     WHERE session_id = ? AND node_id = ? \
                     ORDER BY created_at ASC, id ASC LIMIT 1 OFFSET ?\
                 )",
            )
            .bind(summary)
            .bind(&key.session_id.0)
            .bind(&key.node_id.0)
            .bind(ordinal as i64)
            .execute(&self.pool)
            .await
        }
        .map_err(|e| LlmError::RequestFailed {
            message: format!("Database error: {}", e),
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod summary_tests {
    use super::*;
    use crate::llm::domain::{AgentSessionId, NodeIdPath, SessionId};

    async fn pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE llm_node_history (\
                id TEXT PRIMARY KEY, session_id TEXT, agent_session_id TEXT, node_id TEXT, \
                role TEXT, content TEXT, tool_call_id TEXT, tool_calls TEXT, \
                summary TEXT, created_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn key() -> ConversationKey {
        ConversationKey {
            session_id: SessionId("s".into()),
            agent_session_id: Some(AgentSessionId("a".into())),
            node_id: NodeIdPath("n".into()),
        }
    }

    #[tokio::test]
    async fn set_and_get_summary_roundtrip_sqlite() {
        let repo = SqliteConversationRepository::new(pool().await);
        let k = key();
        repo.add_message(&k, LlmMessage::user("first".into()).unwrap())
            .await
            .unwrap();
        repo.add_message(&k, LlmMessage::assistant("second".into()).unwrap())
            .await
            .unwrap();
        repo.set_summary(&k, 1, "summary of second").await.unwrap();
        let after = repo.get_with_summaries(&k).await.unwrap();
        assert_eq!(after.len(), 2);
        assert!(after[0].summary.is_none());
        assert_eq!(after[1].summary.as_deref(), Some("summary of second"));
    }

    /// Inserts a raw row bypassing `add_message` so we can persist a shape the
    /// domain constructors reject (e.g. a non-assistant row with empty content),
    /// simulating a legacy/corrupt DB row.
    async fn insert_raw(pool: &SqlitePool, role: &str, content: &str, tool_call_id: Option<&str>) {
        sqlx::query(
            "INSERT INTO llm_node_history \
                (id, session_id, agent_session_id, node_id, role, content, \
                 tool_call_id, tool_calls, summary, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind("s")
        .bind("a")
        .bind("n")
        .bind(role)
        .bind(content)
        .bind(tool_call_id)
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .bind(Utc::now().to_rfc3339())
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn get_by_id_skips_malformed_row_instead_of_panicking() {
        let pool = pool().await;
        let repo = SqliteConversationRepository::new(pool.clone());
        let k = key();
        // One valid row, then a corrupt `tool` row with empty content that the
        // `LlmMessage::tool` constructor rejects.
        repo.add_message(&k, LlmMessage::user("hello".into()).unwrap())
            .await
            .unwrap();
        insert_raw(&pool, "tool", "", Some("call_x")).await;

        let conv = repo.get_by_id(&k).await.unwrap();
        assert_eq!(conv.messages.len(), 1, "malformed row must be skipped");
        assert_eq!(conv.messages[0].content(), "hello");
    }

    #[tokio::test]
    async fn get_with_summaries_skips_malformed_row() {
        let pool = pool().await;
        let repo = SqliteConversationRepository::new(pool.clone());
        let k = key();
        repo.add_message(&k, LlmMessage::user("hi".into()).unwrap())
            .await
            .unwrap();
        insert_raw(&pool, "system", "   ", None).await; // whitespace-only → rejected

        let stored = repo.get_with_summaries(&k).await.unwrap();
        assert_eq!(stored.len(), 1, "malformed row must be skipped");
        assert_eq!(stored[0].message.content(), "hi");
    }
}
