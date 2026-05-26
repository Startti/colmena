use crate::llm::domain::{
    AttachmentError, AttachmentRegistry, AttachmentSource, ConversationAttachment, ProviderKind,
    StaleAttachmentQuery, UpsertAttachmentInput,
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
        let pool = SqlitePool::connect(database_url)
            .await
            .map_err(|e| AttachmentError::RepositoryFailed(format!("sqlite connect: {}", e)))?;
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

/// Tolerant timestamp parser for nullable columns (`last_used_at`).
/// Accepts the SQLite default format and ISO-8601/RFC3339 strings; returns
/// `None` if the column is NULL or unparseable rather than failing the row.
fn parse_ts_opt(s: Option<String>) -> Option<DateTime<Utc>> {
    let raw = s?;
    if let Ok(n) = NaiveDateTime::parse_from_str(&raw, "%Y-%m-%d %H:%M:%S") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(n, Utc));
    }
    DateTime::parse_from_rfc3339(&raw)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn row_to_attachment(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ConversationAttachment, AttachmentError> {
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

    let last_used_at_str: Option<String> = row
        .try_get::<Option<String>, _>("last_used_at")
        .ok()
        .flatten();

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
        // Plan A — additive nullable columns.
        storage_key: row
            .try_get::<Option<String>, _>("storage_key")
            .ok()
            .flatten(),
        origin: row.try_get::<Option<String>, _>("origin").ok().flatten(),
        last_used_at: parse_ts_opt(last_used_at_str),
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
                source_kind, source_value, registered_at, refreshed_at,
                storage_key, origin
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?, ?)
            ON CONFLICT (agent_session_id, document_id, provider) DO UPDATE
            SET provider_file_id = excluded.provider_file_id,
                mime_type        = excluded.mime_type,
                filename         = excluded.filename,
                size_bytes       = excluded.size_bytes,
                label            = COALESCE(excluded.label, conversation_attachments.label),
                description      = COALESCE(excluded.description, conversation_attachments.description),
                source_kind      = excluded.source_kind,
                source_value     = excluded.source_value,
                storage_key      = COALESCE(excluded.storage_key, conversation_attachments.storage_key),
                origin           = COALESCE(excluded.origin, conversation_attachments.origin),
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
                    source_kind, source_value, registered_at, refreshed_at,
                    storage_key, origin, last_used_at
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
                SET description = ?
              WHERE agent_session_id = ? AND document_id = ? AND provider = ?",
        )
        .bind(description)
        .bind(agent_session_id)
        .bind(document_id)
        .bind(&provider_str)
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
    /// deterministic winner when multiple rows share the same `refreshed_at` (SQLite stores
    /// timestamps at second resolution, so ties are likely in fast-running tests).
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
             WHERE agent_session_id = ? AND document_id = ?
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
                SET last_used_at = CURRENT_TIMESTAMP
              WHERE agent_session_id = ? AND document_id = ?",
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
        // SQLite stores timestamps as TEXT in `YYYY-MM-DD HH:MM:SS` form (UTC).
        // chrono's `DateTime<Utc>::to_string()` produces an ISO-8601 form which
        // SQLite's string comparison cannot meaningfully order against the stored
        // format. Format the cutoff explicitly to match the column format.
        let cutoff_str = query.cutoff.format("%Y-%m-%d %H:%M:%S").to_string();
        let rows = sqlx::query(
            "SELECT agent_session_id, document_id, provider, provider_file_id,
                    mime_type, filename, size_bytes, label, description,
                    source_kind, source_value, registered_at, refreshed_at,
                    storage_key, origin, last_used_at
             FROM conversation_attachments
             WHERE COALESCE(last_used_at, registered_at) < ?
             ORDER BY COALESCE(last_used_at, registered_at) ASC
             LIMIT ?",
        )
        .bind(cutoff_str)
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
              WHERE agent_session_id = ? AND document_id = ?",
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
            storage_key: None,
            origin: None,
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
                storage_key: None,
                origin: None,
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
            storage_key: None,
            origin: None,
        })
        .await
        .unwrap();
        reg.refresh_provider_file_id("s1", "d", ProviderKind::OpenAi, "pf-new")
            .await
            .unwrap();
        let r = reg
            .lookup("s1", "d", ProviderKind::OpenAi)
            .await
            .unwrap()
            .unwrap();
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

    #[tokio::test]
    async fn update_description_persists_value() {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        // Upsert a row with description=None
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: "s1".into(),
            document_id: "doc-1".into(),
            provider: ProviderKind::Mock,
            provider_file_id: "pf-1".into(),
            mime_type: "application/pdf".into(),
            filename: "a.pdf".into(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key: None,
            origin: None,
        })
        .await
        .unwrap();

        reg.update_description("s1", "doc-1", ProviderKind::Mock, "Q3 financials")
            .await
            .unwrap();

        let row = reg
            .lookup("s1", "doc-1", ProviderKind::Mock)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.description.as_deref(), Some("Q3 financials"));
    }

    #[tokio::test]
    async fn update_description_missing_row_returns_not_found() {
        let reg = SqliteAttachmentRegistry::new("sqlite::memory:")
            .await
            .unwrap();
        let r = reg
            .update_description("s1", "missing", ProviderKind::Mock, "x")
            .await;
        assert!(matches!(r, Err(AttachmentError::NotFound { .. })));
    }

    #[tokio::test]
    async fn upsert_persists_storage_key_and_origin() {
        let reg = make_registry().await;
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: "s1".to_string(),
            document_id: "doc-1".to_string(),
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

        let row = reg
            .lookup("s1", "doc-1", ProviderKind::OpenAi)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.storage_key.as_deref(), Some("sk-1"));
        assert_eq!(row.origin.as_deref(), Some("user_upload"));
        assert!(row.last_used_at.is_none());
    }

    #[tokio::test]
    async fn upsert_preserves_storage_key_when_subsequent_call_omits_it() {
        let reg = make_registry().await;
        // First upsert: provide storage_key.
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: "s1".to_string(),
            document_id: "doc-1".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "a.pdf".to_string(),
            size_bytes: Some(100),
            label: Some("Q3 report".to_string()),
            description: Some("desc".to_string()),
            source: AttachmentSource::Inline,
            storage_key: Some("sk-1".to_string()),
            origin: Some("user_upload".to_string()),
        })
        .await
        .unwrap();

        // Second upsert: same key but storage_key/origin/label/description omitted.
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: "s1".to_string(),
            document_id: "doc-1".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-2".to_string(),
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

        let row = reg
            .lookup("s1", "doc-1", ProviderKind::OpenAi)
            .await
            .unwrap()
            .unwrap();
        // COALESCE keeps previous values when EXCLUDED is NULL.
        assert_eq!(row.storage_key.as_deref(), Some("sk-1"));
        assert_eq!(row.origin.as_deref(), Some("user_upload"));
        assert_eq!(row.label.as_deref(), Some("Q3 report"));
        assert_eq!(row.description.as_deref(), Some("desc"));
        // But provider_file_id was updated.
        assert_eq!(row.provider_file_id, "pf-2");
    }

    #[tokio::test]
    async fn lookup_by_document_id_returns_row_when_present() {
        let reg = make_registry().await;
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: "s1".to_string(),
            document_id: "doc-1".to_string(),
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

        let got = reg.lookup_by_document_id("s1", "doc-1").await.unwrap();
        assert!(got.is_some());
        let row = got.unwrap();
        assert_eq!(row.storage_key.as_deref(), Some("sk-1"));
        assert_eq!(row.origin.as_deref(), Some("user_upload"));
    }

    #[tokio::test]
    async fn lookup_by_document_id_returns_none_when_absent() {
        let reg = make_registry().await;
        let got = reg.lookup_by_document_id("s1", "missing").await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn lookup_by_document_id_returns_most_recent_when_multiple_providers() {
        let reg = make_registry().await;
        // Two rows with same (session, doc) but different providers, inserted back-to-back.
        // SQLite stores `refreshed_at` at second resolution, so both rows are very likely to
        // share the same timestamp — the deterministic secondary sort (`provider ASC`) must
        // pick the alphabetically-first provider (`Google` < `OpenAi`).
        for (provider, pf) in [
            (ProviderKind::OpenAi, "pf-openai"),
            (ProviderKind::Google, "pf-google"),
        ] {
            reg.upsert(UpsertAttachmentInput {
                agent_session_id: "s1".to_string(),
                document_id: "doc-1".to_string(),
                provider,
                provider_file_id: pf.to_string(),
                mime_type: "application/pdf".to_string(),
                filename: "a.pdf".to_string(),
                size_bytes: Some(100),
                label: None,
                description: None,
                source: AttachmentSource::Inline,
                storage_key: Some(format!("sk-{}", pf)),
                origin: Some("user_upload".to_string()),
            })
            .await
            .unwrap();
        }

        let got = reg
            .lookup_by_document_id("s1", "doc-1")
            .await
            .unwrap()
            .expect("row present");
        // With ties on `refreshed_at`, the secondary `provider ASC` key wins deterministically.
        assert_eq!(got.provider, ProviderKind::Google);
    }

    #[tokio::test]
    async fn touch_last_used_updates_timestamp() {
        let reg = make_registry().await;
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: "s1".to_string(),
            document_id: "doc-1".to_string(),
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
            .lookup_by_document_id("s1", "doc-1")
            .await
            .unwrap()
            .unwrap();
        assert!(before.last_used_at.is_none());

        reg.touch_last_used("s1", "doc-1").await.unwrap();

        let after = reg
            .lookup_by_document_id("s1", "doc-1")
            .await
            .unwrap()
            .unwrap();
        assert!(after.last_used_at.is_some());
    }

    #[tokio::test]
    async fn touch_last_used_is_noop_when_missing() {
        let reg = make_registry().await;
        // No row inserted — must not error.
        reg.touch_last_used("s1", "missing").await.unwrap();
    }

    // ----- Plan C: find_stale_attachments + delete_attachment -----

    #[tokio::test]
    async fn find_stale_attachments_returns_rows_older_than_cutoff_sqlite() {
        use crate::llm::domain::StaleAttachmentQuery;
        let reg = make_registry().await;

        reg.upsert(UpsertAttachmentInput {
            agent_session_id: "s1".to_string(),
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
            agent_session_id: "s1".to_string(),
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

        // Backdate the stale row.
        sqlx::query(
            "UPDATE conversation_attachments
                SET registered_at = datetime('now', '-8 days'), last_used_at = NULL
              WHERE document_id = ? AND agent_session_id = ?",
        )
        .bind("stale-doc")
        .bind("s1")
        .execute(&*reg.pool)
        .await
        .unwrap();

        let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
        let stale = reg
            .find_stale_attachments(StaleAttachmentQuery { cutoff, limit: 100 })
            .await
            .unwrap();

        let ids: Vec<&str> = stale.iter().map(|r| r.document_id.as_str()).collect();
        assert!(
            ids.contains(&"stale-doc"),
            "stale-doc should be in stale list, got {:?}",
            ids
        );
        assert!(
            !ids.contains(&"fresh-doc"),
            "fresh-doc should NOT be in stale list, got {:?}",
            ids
        );
    }

    #[tokio::test]
    async fn find_stale_attachments_respects_limit_sqlite() {
        use crate::llm::domain::StaleAttachmentQuery;
        let reg = make_registry().await;

        for i in 0..5 {
            reg.upsert(UpsertAttachmentInput {
                agent_session_id: "s1".to_string(),
                document_id: format!("doc-{}", i),
                provider: ProviderKind::OpenAi,
                provider_file_id: format!("pf-{}", i),
                mime_type: "application/pdf".to_string(),
                filename: "x.pdf".to_string(),
                size_bytes: Some(100),
                label: None,
                description: None,
                source: AttachmentSource::Inline,
                storage_key: Some(format!("sk-{}", i)),
                origin: Some("user_upload".to_string()),
            })
            .await
            .unwrap();
        }

        // Backdate them all.
        sqlx::query(
            "UPDATE conversation_attachments
                SET registered_at = datetime('now', '-8 days'), last_used_at = NULL",
        )
        .execute(&*reg.pool)
        .await
        .unwrap();

        let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
        let batch = reg
            .find_stale_attachments(StaleAttachmentQuery { cutoff, limit: 2 })
            .await
            .unwrap();
        assert_eq!(batch.len(), 2, "limit must cap returned rows");
    }

    #[tokio::test]
    async fn delete_attachment_removes_row_and_is_idempotent_sqlite() {
        let reg = make_registry().await;

        reg.upsert(UpsertAttachmentInput {
            agent_session_id: "s1".to_string(),
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
            .lookup_by_document_id("s1", "to-delete")
            .await
            .unwrap()
            .is_some());

        reg.delete_attachment("s1", "to-delete").await.unwrap();
        assert!(reg
            .lookup_by_document_id("s1", "to-delete")
            .await
            .unwrap()
            .is_none());

        // Idempotent — second call must not error.
        reg.delete_attachment("s1", "to-delete").await.unwrap();
    }
}
