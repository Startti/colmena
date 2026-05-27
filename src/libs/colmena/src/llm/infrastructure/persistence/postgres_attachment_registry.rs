use crate::dag_engine::infrastructure::pool_registry::PgPoolRegistry;
use crate::llm::domain::{
    AttachmentError, AttachmentRegistry, AttachmentSource, ConversationAttachment, ProviderKind,
    StaleAttachmentQuery, UpsertAttachmentInput,
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
        // Plan A — optional columns, may be NULL on legacy rows.
        storage_key: row
            .try_get::<Option<String>, _>("storage_key")
            .ok()
            .flatten(),
        origin: row.try_get::<Option<String>, _>("origin").ok().flatten(),
        last_used_at: row
            .try_get::<Option<DateTime<Utc>>, _>("last_used_at")
            .ok()
            .flatten(),
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
                source_kind, source_value, registered_at, refreshed_at,
                storage_key, origin
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11, NOW(), NOW(), $12, $13)
            ON CONFLICT (agent_session_id, document_id, provider) DO UPDATE
            SET provider_file_id = EXCLUDED.provider_file_id,
                mime_type        = EXCLUDED.mime_type,
                filename         = EXCLUDED.filename,
                size_bytes       = EXCLUDED.size_bytes,
                label            = COALESCE(EXCLUDED.label, conversation_attachments.label),
                description      = COALESCE(EXCLUDED.description, conversation_attachments.description),
                source_kind      = EXCLUDED.source_kind,
                source_value     = EXCLUDED.source_value,
                storage_key      = COALESCE(EXCLUDED.storage_key, conversation_attachments.storage_key),
                origin           = COALESCE(EXCLUDED.origin, conversation_attachments.origin),
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
        .bind(&input.storage_key)
        .bind(&input.origin)
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
                    source_kind, source_value, registered_at, refreshed_at,
                    storage_key, origin, last_used_at
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
                    source_kind, source_value, registered_at, refreshed_at,
                    storage_key, origin, last_used_at
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

    async fn update_description(
        &self,
        agent_session_id: &str,
        document_id: &str,
        provider: ProviderKind,
        description: &str,
    ) -> Result<(), AttachmentError> {
        let provider_str = provider.to_string();
        let res = sqlx::query(
            "UPDATE conversation_attachments
                SET description = $4
              WHERE agent_session_id = $1 AND document_id = $2 AND provider = $3",
        )
        .bind(agent_session_id)
        .bind(document_id)
        .bind(&provider_str)
        .bind(description)
        .execute(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("update_description: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(AttachmentError::NotFound {
                document_id: document_id.to_string(),
            });
        }
        Ok(())
    }

    /// Provider is intentionally ignored — returns the most recently refreshed row across all
    /// providers for the (session, document) pair. Secondary sort by `provider` ASC ensures a
    /// deterministic winner when multiple rows share the same `refreshed_at`.
    async fn lookup_by_document_id(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<Option<ConversationAttachment>, AttachmentError> {
        let row = sqlx::query(
            "SELECT agent_session_id, document_id, provider, provider_file_id,
                    mime_type, filename, size_bytes, label, description,
                    source_kind, source_value, registered_at, refreshed_at,
                    storage_key, origin, last_used_at
             FROM conversation_attachments
             WHERE agent_session_id = $1 AND document_id = $2
             ORDER BY refreshed_at DESC, provider ASC
             LIMIT 1",
        )
        .bind(agent_session_id)
        .bind(document_id)
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("lookup_by_doc: {}", e)))?;

        match row {
            None => Ok(None),
            Some(r) => Ok(Some(row_to_attachment(&r)?)),
        }
    }

    async fn touch_last_used(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<(), AttachmentError> {
        sqlx::query(
            "UPDATE conversation_attachments
                SET last_used_at = NOW()
              WHERE agent_session_id = $1 AND document_id = $2",
        )
        .bind(agent_session_id)
        .bind(document_id)
        .execute(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("touch_last_used: {}", e)))?;
        Ok(())
    }

    async fn find_stale_attachments(
        &self,
        query: StaleAttachmentQuery,
    ) -> Result<Vec<ConversationAttachment>, AttachmentError> {
        let rows = sqlx::query(
            "SELECT agent_session_id, document_id, provider, provider_file_id,
                    mime_type, filename, size_bytes, label, description,
                    source_kind, source_value, registered_at, refreshed_at,
                    storage_key, origin, last_used_at
             FROM conversation_attachments
             WHERE COALESCE(last_used_at, registered_at) < $1
             ORDER BY COALESCE(last_used_at, registered_at) ASC
             LIMIT $2",
        )
        .bind(query.cutoff)
        .bind(query.limit as i64)
        .fetch_all(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("find_stale: {}", e)))?;

        rows.iter().map(row_to_attachment).collect()
    }

    async fn delete_attachment(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<(), AttachmentError> {
        sqlx::query(
            "DELETE FROM conversation_attachments
              WHERE agent_session_id = $1 AND document_id = $2",
        )
        .bind(agent_session_id)
        .bind(document_id)
        .execute(&*self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(format!("delete_attachment: {}", e)))?;
        Ok(())
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
            storage_key: None,
            origin: None,
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
            storage_key: None,
            origin: None,
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

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn update_description_persists_value_pg() {
        let reg = make_registry().await;
        let sid = format!("test_sess_{}", uuid::Uuid::new_v4());
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: "doc-1".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "a.pdf".to_string(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key: None,
            origin: None,
        })
        .await
        .unwrap();

        reg.update_description(&sid, "doc-1", ProviderKind::OpenAi, "Q3 financials")
            .await
            .unwrap();

        let row = reg
            .lookup(&sid, "doc-1", ProviderKind::OpenAi)
            .await
            .unwrap()
            .expect("row present");
        assert_eq!(row.description.as_deref(), Some("Q3 financials"));
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn update_description_missing_row_returns_not_found_pg() {
        let reg = make_registry().await;
        let err = reg
            .update_description("missing_sess", "missing_doc", ProviderKind::OpenAi, "x")
            .await
            .unwrap_err();
        assert!(matches!(err, AttachmentError::NotFound { .. }));
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn lookup_by_document_id_returns_row_when_present_pg() {
        let reg = make_registry().await;
        let sid = format!("test_sess_{}", uuid::Uuid::new_v4());
        let did = format!("doc_{}", uuid::Uuid::new_v4());

        reg.upsert(UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: did.clone(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "a.pdf".to_string(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key: Some("sk-1".to_string()),
            origin: Some("user_upload".to_string()),
        })
        .await
        .unwrap();

        let got = reg.lookup_by_document_id(&sid, &did).await.unwrap();
        assert!(got.is_some(), "expected row to be returned");
        let row = got.unwrap();
        assert_eq!(row.storage_key.as_deref(), Some("sk-1"));
        assert_eq!(row.origin.as_deref(), Some("user_upload"));
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn lookup_by_document_id_returns_none_when_absent_pg() {
        let reg = make_registry().await;
        let sid = format!("test_sess_{}", uuid::Uuid::new_v4());
        let got = reg.lookup_by_document_id(&sid, "missing").await.unwrap();
        assert!(got.is_none());
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn touch_last_used_updates_timestamp_pg() {
        let reg = make_registry().await;
        let sid = format!("test_sess_{}", uuid::Uuid::new_v4());
        let did = format!("doc_{}", uuid::Uuid::new_v4());

        reg.upsert(UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: did.clone(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "a.pdf".to_string(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key: Some("sk-1".to_string()),
            origin: Some("user_upload".to_string()),
        })
        .await
        .unwrap();

        let before = reg
            .lookup_by_document_id(&sid, &did)
            .await
            .unwrap()
            .unwrap();
        assert!(before.last_used_at.is_none());

        reg.touch_last_used(&sid, &did).await.unwrap();

        let after = reg
            .lookup_by_document_id(&sid, &did)
            .await
            .unwrap()
            .unwrap();
        assert!(after.last_used_at.is_some());
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn touch_last_used_is_noop_when_missing_pg() {
        let reg = make_registry().await;
        // Should not error even when no row exists.
        reg.touch_last_used("missing_sess", "missing_doc")
            .await
            .unwrap();
    }

    // ----- Plan C: find_stale_attachments + delete_attachment -----

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn find_stale_attachments_returns_rows_older_than_cutoff() {
        use crate::llm::domain::StaleAttachmentQuery;
        let reg = make_registry().await;
        let sid = format!("test_sess_{}", uuid::Uuid::new_v4());

        // Insert two rows; we'll backdate one to look stale.
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: "stale-doc".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-stale".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "old.pdf".to_string(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key: Some("sk-stale".to_string()),
            origin: Some("user_upload".to_string()),
        })
        .await
        .unwrap();

        reg.upsert(UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: "fresh-doc".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-fresh".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "new.pdf".to_string(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key: Some("sk-fresh".to_string()),
            origin: Some("user_upload".to_string()),
        })
        .await
        .unwrap();

        // Force the first row to look stale by backdating registered_at + nulling last_used_at.
        sqlx::query(
            "UPDATE conversation_attachments
                SET registered_at = NOW() - INTERVAL '8 days', last_used_at = NULL
              WHERE document_id = $1 AND agent_session_id = $2",
        )
        .bind("stale-doc")
        .bind(&sid)
        .execute(&*reg.pool)
        .await
        .unwrap();

        let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
        let stale = reg
            .find_stale_attachments(StaleAttachmentQuery { cutoff, limit: 100 })
            .await
            .unwrap();

        // Filter to our session — other concurrent tests may have stale rows too.
        let stale_for_us: Vec<&str> = stale
            .iter()
            .filter(|r| r.agent_session_id == sid)
            .map(|r| r.document_id.as_str())
            .collect();
        assert!(
            stale_for_us.contains(&"stale-doc"),
            "stale-doc should be in stale list, got {:?}",
            stale_for_us
        );
        assert!(
            !stale_for_us.contains(&"fresh-doc"),
            "fresh-doc should NOT be in stale list, got {:?}",
            stale_for_us
        );
    }

    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn delete_attachment_removes_row_and_is_idempotent_pg() {
        let reg = make_registry().await;
        let sid = format!("test_sess_{}", uuid::Uuid::new_v4());

        reg.upsert(UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: "to-delete".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-del".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key: Some("sk-del".to_string()),
            origin: Some("user_upload".to_string()),
        })
        .await
        .unwrap();

        assert!(reg
            .lookup_by_document_id(&sid, "to-delete")
            .await
            .unwrap()
            .is_some());

        reg.delete_attachment(&sid, "to-delete").await.unwrap();
        assert!(reg
            .lookup_by_document_id(&sid, "to-delete")
            .await
            .unwrap()
            .is_none());

        // Idempotent — deleting again must not error.
        reg.delete_attachment(&sid, "to-delete").await.unwrap();
    }
}
