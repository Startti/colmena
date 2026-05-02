//! Implementación Postgres de FileCacheRepository.
//! Usa el `PgPoolRegistry` compartido — la conexión viene siempre
//! de DATABASE_URL (env), independiente de la `connection_url` por nodo
//! que usa el backend de memoria.

use crate::dag_engine::infrastructure::pool_registry::PgPoolRegistry;
use crate::llm::domain::{CachedFileEntry, FileCacheRepository, LlmError, ProviderKind};
use async_trait::async_trait;
use sqlx::PgPool;
use std::str::FromStr;
use std::sync::Arc;

pub struct PostgresFileCache {
    pool: Arc<PgPool>,
}

impl PostgresFileCache {
    pub async fn new(
        registry: Arc<PgPoolRegistry>,
        database_url: &str,
    ) -> Result<Self, LlmError> {
        let pool = registry
            .get_or_create(database_url)
            .await
            .map_err(|e| LlmError::RequestFailed {
                message: format!("Failed to get Postgres pool: {}", e),
            })?;

        Ok(Self { pool })
    }
}

#[async_trait]
impl FileCacheRepository for PostgresFileCache {
    async fn lookup(
        &self,
        document_id: &str,
        provider: ProviderKind,
    ) -> Result<Option<CachedFileEntry>, LlmError> {
        let provider_str = provider.to_string();
        let row = sqlx::query!(
            r#"
            SELECT document_id, provider, provider_file_id, mime_type, filename,
                   size_bytes, uploaded_at, expires_at, last_used_at
              FROM provider_file_cache
             WHERE document_id = $1 AND provider = $2
            "#,
            document_id,
            provider_str
        )
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed {
            message: format!("provider_file_cache lookup failed: {}", e),
        })?;

        Ok(row.map(|r| {
            let provider = ProviderKind::from_str(&r.provider).unwrap_or_else(|_| {
                eprintln!(
                    "PostgresFileCache: unknown provider '{}' in row for document_id={}, falling back to Mock",
                    r.provider, r.document_id
                );
                ProviderKind::Mock
            });
            CachedFileEntry {
                document_id: r.document_id,
                provider,
                provider_file_id: r.provider_file_id,
                mime_type: r.mime_type,
                filename: r.filename,
                size_bytes: r.size_bytes,
                uploaded_at: r.uploaded_at,
                expires_at: r.expires_at,
                last_used_at: r.last_used_at,
            }
        }))
    }

    async fn upsert(&self, entry: &CachedFileEntry) -> Result<(), LlmError> {
        let provider_str = entry.provider.to_string();
        sqlx::query!(
            r#"
            INSERT INTO provider_file_cache
                (document_id, provider, provider_file_id, mime_type, filename,
                 size_bytes, uploaded_at, expires_at, last_used_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (document_id, provider) DO UPDATE SET
                provider_file_id = EXCLUDED.provider_file_id,
                mime_type        = EXCLUDED.mime_type,
                filename         = EXCLUDED.filename,
                size_bytes       = EXCLUDED.size_bytes,
                uploaded_at      = EXCLUDED.uploaded_at,
                expires_at       = EXCLUDED.expires_at,
                last_used_at     = NOW()
            "#,
            entry.document_id,
            provider_str,
            entry.provider_file_id,
            entry.mime_type,
            entry.filename,
            entry.size_bytes,
            entry.uploaded_at,
            entry.expires_at,
            entry.last_used_at,
        )
        .execute(&*self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed {
            message: format!("provider_file_cache upsert failed: {}", e),
        })?;
        Ok(())
    }

    async fn invalidate(
        &self,
        document_id: &str,
        provider: ProviderKind,
    ) -> Result<(), LlmError> {
        let provider_str = provider.to_string();
        sqlx::query!(
            r#"DELETE FROM provider_file_cache
                WHERE document_id = $1 AND provider = $2"#,
            document_id,
            provider_str
        )
        .execute(&*self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed {
            message: format!("provider_file_cache invalidate failed: {}", e),
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::infrastructure::pool_registry::PoolConfig;
    use chrono::Utc;

    /// Helper: crea una instancia con PG real. Requiere TEST_DATABASE_URL.
    /// Skip si no está set.
    async fn with_cache<F, Fut>(f: F)
    where
        F: FnOnce(PostgresFileCache) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let url = match std::env::var("TEST_DATABASE_URL") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("skipping: TEST_DATABASE_URL not set");
                return;
            }
        };
        let registry = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
        // Run migration explicitly.
        let pool = registry.get_or_create(&url).await.unwrap();
        sqlx::migrate!("migrations/postgres")
            .set_ignore_missing(true)
            .run(&*pool)
            .await
            .unwrap();
        let cache = PostgresFileCache::new(registry, &url).await.unwrap();
        // Clean state.
        sqlx::query!("DELETE FROM provider_file_cache WHERE document_id LIKE 'test-%'")
            .execute(&*pool)
            .await
            .unwrap();
        f(cache).await;
    }

    fn fixture(doc_id: &str) -> CachedFileEntry {
        let now = Utc::now();
        CachedFileEntry {
            document_id: doc_id.into(),
            provider: ProviderKind::Anthropic,
            provider_file_id: "file_abc".into(),
            mime_type: "application/pdf".into(),
            filename: "report.pdf".into(),
            size_bytes: Some(2_000_000),
            uploaded_at: now,
            expires_at: None,
            last_used_at: now,
        }
    }

    #[tokio::test]
    async fn lookup_miss_returns_none() {
        with_cache(|cache| async move {
            let r = cache
                .lookup("test-not-exist", ProviderKind::Anthropic)
                .await
                .unwrap();
            assert!(r.is_none());
        })
        .await;
    }

    #[tokio::test]
    async fn upsert_then_lookup_returns_entry() {
        with_cache(|cache| async move {
            let entry = fixture("test-1");
            cache.upsert(&entry).await.unwrap();
            let got = cache
                .lookup("test-1", ProviderKind::Anthropic)
                .await
                .unwrap();
            assert!(got.is_some());
            assert_eq!(got.unwrap().provider_file_id, "file_abc");
        })
        .await;
    }

    #[tokio::test]
    async fn upsert_twice_updates() {
        with_cache(|cache| async move {
            let mut entry = fixture("test-2");
            cache.upsert(&entry).await.unwrap();
            entry.provider_file_id = "file_xyz".into();
            cache.upsert(&entry).await.unwrap();
            let got = cache
                .lookup("test-2", ProviderKind::Anthropic)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(got.provider_file_id, "file_xyz");
        })
        .await;
    }

    #[tokio::test]
    async fn invalidate_removes() {
        with_cache(|cache| async move {
            let entry = fixture("test-3");
            cache.upsert(&entry).await.unwrap();
            cache
                .invalidate("test-3", ProviderKind::Anthropic)
                .await
                .unwrap();
            let got = cache
                .lookup("test-3", ProviderKind::Anthropic)
                .await
                .unwrap();
            assert!(got.is_none());
        })
        .await;
    }
}
