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
                 ORDER BY created_at ASC",
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
                 ORDER BY created_at ASC",
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
            .map(|row| {
                let role_str: String = row.get("role");
                let content: String = row.get("content");
                let tool_call_id: Option<String> = row.get("tool_call_id");
                let tool_calls_str: Option<String> = row.get("tool_calls");
                let _created_at_str: String = row.get("created_at");

                let role = match role_str.as_str() {
                    "system" => MessageRole::System,
                    "user" => MessageRole::User,
                    "assistant" => MessageRole::Assistant,
                    "tool" => MessageRole::Tool,
                    _ => MessageRole::User,
                };

                match role {
                    MessageRole::System => LlmMessage::system(content).unwrap(),
                    MessageRole::User => LlmMessage::user(content).unwrap(),
                    MessageRole::Assistant => {
                        if let Some(tc_str) = tool_calls_str {
                            let tool_calls: Vec<crate::llm::domain::ToolCall> =
                                serde_json::from_str(&tc_str).unwrap_or_default();
                            LlmMessage::assistant_with_tool_calls(content, tool_calls).unwrap()
                        } else {
                            LlmMessage::assistant(content).unwrap()
                        }
                    }
                    MessageRole::Tool => LlmMessage::tool(
                        tool_call_id.unwrap_or_else(|| "unknown".to_string()),
                        content,
                    )
                    .unwrap(),
                }
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
                 ORDER BY created_at ASC",
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
                 ORDER BY created_at ASC",
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
            .map(|row| {
                let role_str: String = row.get("role");
                let content: String = row.get("content");
                let tool_call_id: Option<String> = row.get("tool_call_id");
                let tool_calls_str: Option<String> = row.get("tool_calls");
                let summary: Option<String> = row.get("summary");

                let role = match role_str.as_str() {
                    "system" => MessageRole::System,
                    "user" => MessageRole::User,
                    "assistant" => MessageRole::Assistant,
                    "tool" => MessageRole::Tool,
                    _ => MessageRole::User,
                };

                let message = match role {
                    MessageRole::System => LlmMessage::system(content).unwrap(),
                    MessageRole::User => LlmMessage::user(content).unwrap(),
                    MessageRole::Assistant => {
                        if let Some(tc_str) = tool_calls_str {
                            let tool_calls: Vec<crate::llm::domain::ToolCall> =
                                serde_json::from_str(&tc_str).unwrap_or_default();
                            LlmMessage::assistant_with_tool_calls(content, tool_calls).unwrap()
                        } else {
                            LlmMessage::assistant(content).unwrap()
                        }
                    }
                    MessageRole::Tool => LlmMessage::tool(
                        tool_call_id.unwrap_or_else(|| "unknown".to_string()),
                        content,
                    )
                    .unwrap(),
                };

                StoredMessage { message, summary }
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
                     ORDER BY created_at ASC LIMIT 1 OFFSET ?\
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
                     ORDER BY created_at ASC LIMIT 1 OFFSET ?\
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
}
