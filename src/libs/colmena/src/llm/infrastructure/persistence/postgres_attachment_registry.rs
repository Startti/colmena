use crate::dag_engine::infrastructure::pool_registry::PgPoolRegistry;
use crate::llm::domain::{
    AttachmentError, AttachmentRegistry, AttachmentSource, ConversationAttachment, ProviderKind,
    UpsertAttachmentInput,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use std::str::FromStr;
use std::sync::Arc;

pub struct PostgresAttachmentRegistry {
    pool: Arc<PgPool>,
}

impl PostgresAttachmentRegistry {
    pub async fn new(
        registry: Arc<PgPoolRegistry>,
        database_url: &str,
    ) -> Result<Self, AttachmentError> {
        let pool = registry
            .get_or_create(database_url)
            .await
            .map_err(|e| AttachmentError::RepositoryFailed(format!("pool init failed: {}", e)))?;
        Ok(Self { pool })
    }
}

fn row_to_attachment(
    row: &sqlx::postgres::PgRow,
) -> Result<ConversationAttachment, AttachmentError> {
    let provider_db: String = row
        .try_get("provider")
        .map_err(|e| AttachmentError::RepositoryFailed(format!("provider column read: {}", e)))?;
    let provider = ProviderKind::from_str(&provider_db).map_err(|e| {
        AttachmentError::RepositoryFailed(format!("corrupted provider '{}': {}", provider_db, e))
    })?;

    let source_kind: String = row
        .try_get("source_kind")
        .map_err(|e| AttachmentError::RepositoryFailed(format!("source_kind: {}", e)))?;
    let source_value: Option<String> = row
        .try_get("source_value")
        .map_err(|e| AttachmentError::RepositoryFailed(format!("source_value: {}", e)))?;
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

    let size_bytes_db: Option<i64> = row.try_get("size_bytes").ok().flatten();
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
        registered_at: row
            .try_get::<DateTime<Utc>, _>("registered_at")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("registered_at: {}", e)))?,
        refreshed_at: row
            .try_get::<DateTime<Utc>, _>("refreshed_at")
            .map_err(|e| AttachmentError::RepositoryFailed(format!("refreshed_at: {}", e)))?,
    })
}

#[async_trait]
impl AttachmentRegistry for PostgresAttachmentRegistry {
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
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11, NOW(), NOW())
            ON CONFLICT (agent_session_id, document_id, provider) DO UPDATE
            SET provider_file_id = EXCLUDED.provider_file_id,
                mime_type        = EXCLUDED.mime_type,
                filename         = EXCLUDED.filename,
                size_bytes       = EXCLUDED.size_bytes,
                label            = EXCLUDED.label,
                description      = EXCLUDED.description,
                source_kind      = EXCLUDED.source_kind,
                source_value     = EXCLUDED.source_value,
                refreshed_at     = NOW()",
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
             WHERE agent_session_id = $1 AND document_id = $2 AND provider = $3",
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
                SET provider_file_id = $4, refreshed_at = NOW()
              WHERE agent_session_id = $1 AND document_id = $2 AND provider = $3",
        )
        .bind(agent_session_id)
        .bind(document_id)
        .bind(&provider_str)
        .bind(new_provider_file_id)
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
             WHERE agent_session_id = $1
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
    use crate::dag_engine::infrastructure::pool_registry::PoolConfig;

    async fn make_registry() -> PostgresAttachmentRegistry {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        let registry = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
        let pool = registry.get_or_create(&url).await.unwrap();
        sqlx::migrate!("migrations/postgres")
            .set_ignore_missing(true)
            .run(&*pool)
            .await
            .unwrap();
        PostgresAttachmentRegistry::new(registry, &url)
            .await
            .unwrap()
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn upsert_then_lookup_roundtrip() {
        let reg = make_registry().await;
        let sid = format!("test_sess_{}", uuid::Uuid::new_v4());
        let input = UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: "doc-1".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_bytes: Some(1024),
            label: Some("X".to_string()),
            description: None,
            source: AttachmentSource::SignedUrl("https://u".to_string()),
        };
        reg.upsert(input.clone()).await.unwrap();
        let got = reg
            .lookup(&sid, "doc-1", ProviderKind::OpenAi)
            .await
            .unwrap()
            .expect("row present");
        assert_eq!(got.provider_file_id, "pf-1");
        assert_eq!(
            got.source,
            AttachmentSource::SignedUrl("https://u".to_string())
        );
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn upsert_is_idempotent_and_overwrites() {
        let reg = make_registry().await;
        let sid = format!("test_sess_{}", uuid::Uuid::new_v4());
        let mk = |pf: &str| UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: "doc-1".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: pf.to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_bytes: None,
            label: None,
            description: None,
            source: AttachmentSource::Inline,
        };
        reg.upsert(mk("pf-1")).await.unwrap();
        reg.upsert(mk("pf-2")).await.unwrap();
        let got = reg
            .lookup(&sid, "doc-1", ProviderKind::OpenAi)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.provider_file_id, "pf-2");
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn refresh_returns_not_found_when_missing() {
        let reg = make_registry().await;
        let err = reg
            .refresh_provider_file_id("missing_sess", "missing_doc", ProviderKind::OpenAi, "pf-x")
            .await
            .unwrap_err();
        assert!(matches!(err, AttachmentError::NotFound { .. }));
    }
}
