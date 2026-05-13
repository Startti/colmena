use crate::llm::domain::{
    AttachmentError, AttachmentRegistry, AttachmentSource, ConversationAttachment, ProviderKind,
    UpsertAttachmentInput,
};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use std::sync::Arc;

pub struct SqliteAttachmentRegistry {
    pool: Arc<SqlitePool>,
}

impl SqliteAttachmentRegistry {
    pub async fn new(database_url: &str) -> Result<Self, AttachmentError> {
        let pool = SqlitePool::connect(database_url).await.map_err(|e| {
            AttachmentError::RepositoryFailed(format!("sqlite connect: {}", e))
        })?;
        sqlx::migrate!("migrations/sqlite")
            .set_ignore_missing(true)
            .run(&pool)
            .await
            .map_err(|e| AttachmentError::RepositoryFailed(format!("migrate: {}", e)))?;
        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    pub fn from_pool(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>, AttachmentError> {
    // SQLite default `CURRENT_TIMESTAMP` produces "YYYY-MM-DD HH:MM:SS".
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|n| DateTime::<Utc>::from_naive_utc_and_offset(n, Utc))
        .map_err(|e| AttachmentError::RepositoryFailed(format!("bad ts '{}': {}", s, e)))
}

fn row_to_attachment(row: &sqlx::sqlite::SqliteRow) -> Result<ConversationAttachment, AttachmentError> {
    let provider_db: String = row
        .try_get("provider")
        .map_err(|e| AttachmentError::RepositoryFailed(format!("provider: {}", e)))?;
    let provider = ProviderKind::from_str(&provider_db)
        .map_err(|e| AttachmentError::RepositoryFailed(format!("provider parse: {}", e)))?;

    let source_kind: String = row
        .try_get("source_kind")
        .map_err(|e| AttachmentError::RepositoryFailed(format!("source_kind: {}", e)))?;
    let source_value: Option<String> = row.try_get("source_value").ok();
    let source = match source_kind.as_str() {
        "signed_url" => AttachmentSource::SignedUrl(source_value.unwrap_or_default()),
        "path" => AttachmentSource::Path(source_value.unwrap_or_default()),
        "inline" => AttachmentSource::Inline,
        other => {
            return Err(AttachmentError::RepositoryFailed(format!(
                "unknown source_kind '{}'",
                other
            )));
        }
    };

    let registered_at_str: String = row
        .try_get("registered_at")
        .map_err(|e| AttachmentError::RepositoryFailed(format!("registered_at: {}", e)))?;
    let refreshed_at_str: String = row
        .try_get("refreshed_at")
        .map_err(|e| AttachmentError::RepositoryFailed(format!("refreshed_at: {}", e)))?;

    let size_bytes_db: Option<i64> = row.try_get("size_bytes").ok();
    let size_bytes = size_bytes_db.map(|n| n as u64);

    Ok(ConversationAttachment {
        agent_session_id: row
            .try_get("agent_session_id")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("agent_session_id: {}", e)))?,
        document_id: row
            .try_get("document_id")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("document_id: {}", e)))?,
        provider,
        provider_file_id: row
            .try_get("provider_file_id")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("provider_file_id: {}", e)))?,
        mime_type: row
            .try_get("mime_type")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("mime_type: {}", e)))?,
        filename: row
            .try_get("filename")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("filename: {}", e)))?,
        size_bytes,
        label: row.try_get("label").ok(),
        description: row.try_get("description").ok(),
        source,
        registered_at: parse_ts(&registered_at_str)?,
        refreshed_at: parse_ts(&refreshed_at_str)?,
    })
}

#[async_trait]
impl AttachmentRegistry for SqliteAttachmentRegistry {
    async fn upsert(&self, input: UpsertAttachmentInput) -> Result<(), AttachmentError> {
        let provider_str = input.provider.to_string();
        let source_kind = input.source.kind_str().to_string();
        let source_value = input.source.value().map(|s| s.to_string());
        let size_db: Option<i64> = input.size_bytes.map(|n| n as i64);

        sqlx::query(
            "INSERT INTO conversation_attachments (
                agent_session_id, document_id, provider, provider_file_id,
                mime_type, filename, size_bytes, label, description,
                source_kind, source_value, registered_at, refreshed_at
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            ON CONFLICT (agent_session_id, document_id, provider) DO UPDATE
            SET provider_file_id = excluded.provider_file_id,
                mime_type        = excluded.mime_type,
                filename         = excluded.filename,
                size_bytes       = excluded.size_bytes,
                label            = excluded.label,
                description      = excluded.description,
                source_kind      = excluded.source_kind,
                source_value     = excluded.source_value,
                refreshed_at     = CURRENT_TIMESTAMP",
        )
        .bind(&input.agent_session_id)
        .bind(&input.document_id)
        .bind(&provider_str)
        .bind(&input.provider_file_id)
        .bind(&input.mime_type)
        .bind(&input.filename)
        .bind(size_db)
        .bind(&input.label)
        .bind(&input.description)
        .bind(&source_kind)
        .bind(&source_value)
        .execute(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("upsert: {}", e)))?;
        Ok(())
    }

    async fn lookup(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
    ) -> Result<Option<ConversationAttachment>, AttachmentError> {
        let provider_str = provider.to_string();
        let row = sqlx::query(
            "SELECT agent_session_id, document_id, provider, provider_file_id,
                    mime_type, filename, size_bytes, label, description,
                    source_kind, source_value, registered_at, refreshed_at
             FROM conversation_attachments
             WHERE agent_session_id = ? AND document_id = ? AND provider = ?",
        )
        .bind(agent_session_id)
        .bind(document_id)
        .bind(&provider_str)
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("lookup: {}", e)))?;

        match row {
            None => Ok(None),
            Some(r) => Ok(Some(row_to_attachment(&r)?)),
        }
    }

    async fn refresh_provider_file_id(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
        new_provider_file_id: &str,
    ) -> Result<(), AttachmentError> {
        let provider_str = provider.to_string();
        let res = sqlx::query(
            "UPDATE conversation_attachments
                SET provider_file_id = ?, refreshed_at = CURRENT_TIMESTAMP
              WHERE agent_session_id = ? AND document_id = ? AND provider = ?",
        )
        .bind(new_provider_file_id)
        .bind(agent_session_id)
        .bind(document_id)
        .bind(&provider_str)
        .execute(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("refresh: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(AttachmentError::NotFound {
                document_id: document_id.to_string(),
            });
        }
        Ok(())
    }

    async fn list_for_session(
        &self,
        agent_session_id: &str,
    ) -> Result<Vec<ConversationAttachment>, AttachmentError> {
        let rows = sqlx::query(
            "SELECT agent_session_id, document_id, provider, provider_file_id,
                    mime_type, filename, size_bytes, label, description,
                    source_kind, source_value, registered_at, refreshed_at
             FROM conversation_attachments
             WHERE agent_session_id = ?
             ORDER BY registered_at ASC",
        )
        .bind(agent_session_id)
        .fetch_all(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("list: {}", e)))?;

        rows.iter().map(row_to_attachment).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn make_registry() -> SqliteAttachmentRegistry {
        // In-memory; isolated per test.
        SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn upsert_then_lookup_roundtrip() {
        let reg = make_registry().await;
        let input = UpsertAttachmentInput {
            agent_session_id: "s1".to_string(),
            document_id: "doc-1".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_bytes: Some(1024),
            label: Some("X".to_string()),
            description: Some("desc".to_string()),
            source: AttachmentSource::Path("/tmp/x.pdf".to_string()),
        };
        reg.upsert(input).await.unwrap();
        let got = reg
            .lookup("s1", "doc-1", ProviderKind::OpenAi)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.provider_file_id, "pf-1");
        assert_eq!(got.label, Some("X".to_string()));
        assert_eq!(got.source, AttachmentSource::Path("/tmp/x.pdf".to_string()));
    }

    #[tokio::test]
    async fn list_returns_only_session_rows_ordered() {
        let reg = make_registry().await;
        for (sid, doc) in [("s1", "a"), ("s1", "b"), ("s2", "c")] {
            reg.upsert(UpsertAttachmentInput {
                agent_session_id: sid.to_string(),
                document_id: doc.to_string(),
                provider: ProviderKind::OpenAi,
                provider_file_id: format!("pf-{}", doc),
                mime_type: "text/plain".to_string(),
                filename: format!("{}.txt", doc),
                size_bytes: None,
                label: None,
                description: None,
                source: AttachmentSource::Inline,
            })
            .await
            .unwrap();
        }
        let rows = reg.list_for_session("s1").await.unwrap();
        let ids: Vec<_> = rows.iter().map(|r| r.document_id.clone()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn refresh_updates_provider_file_id() {
        let reg = make_registry().await;
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: "s1".to_string(),
            document_id: "d".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-old".to_string(),
            mime_type: "x".to_string(),
            filename: "x".to_string(),
            size_bytes: None,
            label: None,
            description: None,
            source: AttachmentSource::Inline,
        })
        .await
        .unwrap();
        reg.refresh_provider_file_id("s1", "d", ProviderKind::OpenAi, "pf-new")
            .await
            .unwrap();
        let r = reg.lookup("s1", "d", ProviderKind::OpenAi).await.unwrap().unwrap();
        assert_eq!(r.provider_file_id, "pf-new");
    }

    #[tokio::test]
    async fn refresh_returns_not_found_for_missing_row() {
        let reg = make_registry().await;
        let err = reg
            .refresh_provider_file_id("nope", "nope", ProviderKind::OpenAi, "x")
            .await
            .unwrap_err();
        assert!(matches!(err, AttachmentError::NotFound { .. }));
    }
}
