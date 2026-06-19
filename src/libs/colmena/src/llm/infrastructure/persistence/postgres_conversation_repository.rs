use crate::llm::domain::{
    Conversation, ConversationKey, ConversationRepository, LlmError, LlmMessage, MessageRole,
    StoredMessage,
};

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
    async fn get_by_id(&self, key: &ConversationKey) -> Result<Conversation, LlmError> {
        let rows = if let Some(agent) = &key.agent_session_id {
            sqlx::query(
                "SELECT role, content, tool_call_id, tool_calls, created_at \
                 FROM llm_node_history \
                 WHERE agent_session_id = $1 AND node_id = $2 \
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
                 WHERE session_id = $1 AND node_id = $2 \
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
                let tool_calls_json: Option<serde_json::Value> = row.get("tool_calls");
                let _created_at: DateTime<Utc> = row.get("created_at");

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
                        if let Some(tc_json) = tool_calls_json {
                            let tool_calls: Vec<crate::llm::domain::ToolCall> =
                                serde_json::from_value(tc_json).unwrap_or_default();
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

        let tool_calls_json = message
            .tool_calls()
            .and_then(|tc| serde_json::to_value(tc).ok());

        sqlx::query(
            "INSERT INTO llm_node_history (\
                session_id, agent_session_id, node_id, \
                role, content, tool_call_id, tool_calls, created_at\
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(&key.session_id.0)
        .bind(key.agent_session_id.as_ref().map(|a| a.0.clone()))
        .bind(&key.node_id.0)
        .bind(role_str)
        .bind(message.content())
        .bind(message.tool_call_id())
        .bind(tool_calls_json)
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed {
            message: format!("Database error: {}", e),
        })?;

        Ok(())
    }

    async fn delete(&self, key: &ConversationKey) -> Result<(), LlmError> {
        let res = if let Some(agent) = &key.agent_session_id {
            sqlx::query("DELETE FROM llm_node_history WHERE agent_session_id = $1 AND node_id = $2")
                .bind(&agent.0)
                .bind(&key.node_id.0)
                .execute(&self.pool)
                .await
        } else {
            sqlx::query("DELETE FROM llm_node_history WHERE session_id = $1 AND node_id = $2")
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
                 WHERE agent_session_id = $1 AND node_id = $2 \
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
                 WHERE session_id = $1 AND node_id = $2 \
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
                let tool_calls_json: Option<serde_json::Value> = row.get("tool_calls");
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
                        if let Some(tc_json) = tool_calls_json {
                            let tool_calls: Vec<crate::llm::domain::ToolCall> =
                                serde_json::from_value(tc_json).unwrap_or_default();
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
                "UPDATE llm_node_history SET summary = $1 \
                 WHERE id = (\
                     SELECT id FROM llm_node_history \
                     WHERE agent_session_id = $2 AND node_id = $3 \
                     ORDER BY created_at ASC OFFSET $4 LIMIT 1\
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
                "UPDATE llm_node_history SET summary = $1 \
                 WHERE id = (\
                     SELECT id FROM llm_node_history \
                     WHERE session_id = $2 AND node_id = $3 \
                     ORDER BY created_at ASC OFFSET $4 LIMIT 1\
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

    fn key(agent: &str) -> ConversationKey {
        ConversationKey {
            session_id: SessionId(format!("sess_{agent}")),
            agent_session_id: Some(AgentSessionId(agent.to_string())),
            node_id: NodeIdPath("n".to_string()),
        }
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn set_and_get_summary_roundtrip() {
        let pool = sqlx::PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let repo = PostgresConversationRepository::new(pool);
        let k = key("pg_summary_test_001");
        repo.delete(&k).await.unwrap();
        repo.add_message(&k, LlmMessage::user("first".into()).unwrap())
            .await
            .unwrap();
        repo.add_message(&k, LlmMessage::assistant("second".into()).unwrap())
            .await
            .unwrap();
        let before = repo.get_with_summaries(&k).await.unwrap();
        assert_eq!(before.len(), 2);
        assert!(before[0].summary.is_none());
        repo.set_summary(&k, 1, "summary of second").await.unwrap();
        let after = repo.get_with_summaries(&k).await.unwrap();
        assert!(after[0].summary.is_none());
        assert_eq!(after[1].summary.as_deref(), Some("summary of second"));
        assert_eq!(after[1].message.content(), "second");
        repo.delete(&k).await.unwrap();
    }
}
