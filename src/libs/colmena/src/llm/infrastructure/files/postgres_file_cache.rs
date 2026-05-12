//! Implementación Postgres de FileCacheRepository.
//! Usa el `PgPoolRegistry` compartido — la conexión viene siempre
//! de DATABASE_URL (env), independiente de la `connection_url` por nodo
//! que usa el backend de memoria.

use crate::dag_engine::infrastructure::pool_registry::PgPoolRegistry;
use crate::llm::domain::{CachedFileEntry, FileCacheRepository, LlmError, ProviderKind};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use std::str::FromStr;
use std::sync::Arc;

pub struct PostgresFileCache {
    pool: Arc<PgPool>,
}

/// Convierte el string de la columna `provider` a `ProviderKind`. Fail-fast
/// si la fila tiene un valor que no mapea al enum — antes el fallback
/// silencioso a `ProviderKind::Mock` ocultaba la corrupción y producía
/// errores opacos al usar el `provider_file_id` con el kind equivocado.
fn parse_provider_from_row(provider_db: &str, document_id: &str) -> Result<ProviderKind, LlmError> {
    ProviderKind::from_str(provider_db).map_err(|e| {
        tracing::error!(
            provider = %provider_db,
            document_id = %document_id,
            "provider_file_cache: corrupted provider in cache row"
        );
        LlmError::RequestFailed {
            message: format!(
                "provider_file_cache: corrupted provider '{}' for document_id={}: {}",
                provider_db, document_id, e
            ),
        }
    })
}

impl PostgresFileCache {
    pub async fn new(registry: Arc<PgPoolRegistry>, database_url: &str) -> Result<Self, LlmError> {
        let pool =
            registry
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
        // `UPDATE ... RETURNING` toca `last_used_at` cada cache hit en una
        // sola query. Si la fila no existe, devuelve 0 rows → mismo resultado
        // que un SELECT MISS. Coste: row-lock en lugar de share-lock, pero la
        // concurrencia por `(document_id, provider)` es baja (un mismo doc
        // está en un solo request a la vez), así que no causa contención.
        let provider_str = provider.to_string();
        let row = sqlx::query(
            "UPDATE provider_file_cache \
                SET last_used_at = NOW() \
              WHERE document_id = $1 AND provider = $2 \
          RETURNING document_id, provider, provider_file_id, mime_type, filename, \
                    size_bytes, uploaded_at, expires_at, last_used_at",
        )
        .bind(document_id)
        .bind(&provider_str)
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed {
            message: format!("provider_file_cache lookup failed: {}", e),
        })?;

        let Some(row) = row else {
            return Ok(None);
        };

        let document_id: String =
            row.try_get::<String, _>("document_id")
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("provider_file_cache lookup decode failed: {}", e),
                })?;
        let provider_db: String =
            row.try_get::<String, _>("provider")
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("provider_file_cache lookup decode failed: {}", e),
                })?;
        let provider_file_id: String =
            row.try_get::<String, _>("provider_file_id")
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("provider_file_cache lookup decode failed: {}", e),
                })?;
        let mime_type: String =
            row.try_get::<String, _>("mime_type")
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("provider_file_cache lookup decode failed: {}", e),
                })?;
        let filename: String =
            row.try_get::<String, _>("filename")
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("provider_file_cache lookup decode failed: {}", e),
                })?;
        let size_bytes: Option<i64> =
            row.try_get::<Option<i64>, _>("size_bytes")
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("provider_file_cache lookup decode failed: {}", e),
                })?;
        let uploaded_at: DateTime<Utc> =
            row.try_get::<DateTime<Utc>, _>("uploaded_at")
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("provider_file_cache lookup decode failed: {}", e),
                })?;
        let expires_at: Option<DateTime<Utc>> = row
            .try_get::<Option<DateTime<Utc>>, _>("expires_at")
            .map_err(|e| LlmError::RequestFailed {
                message: format!("provider_file_cache lookup decode failed: {}", e),
            })?;
        let last_used_at: DateTime<Utc> =
            row.try_get::<DateTime<Utc>, _>("last_used_at")
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("provider_file_cache lookup decode failed: {}", e),
                })?;

        let provider = parse_provider_from_row(&provider_db, &document_id)?;

        Ok(Some(CachedFileEntry {
            document_id,
            provider,
            provider_file_id,
            mime_type,
            filename,
            size_bytes,
            uploaded_at,
            expires_at,
            last_used_at,
        }))
    }

    /// On INSERT, `last_used_at` from the entry is honored.
    /// On UPDATE (conflict on `(document_id, provider)`), `last_used_at` is set
    /// to `NOW()` regardless of the value passed in `entry.last_used_at`.
    /// This treats every upsert as a "touch" of the cache row.
    async fn upsert(&self, entry: &CachedFileEntry) -> Result<(), LlmError> {
        let provider_str = entry.provider.to_string();
        sqlx::query(
            "INSERT INTO provider_file_cache \
                (document_id, provider, provider_file_id, mime_type, filename, \
                 size_bytes, uploaded_at, expires_at, last_used_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (document_id, provider) DO UPDATE SET \
                provider_file_id = EXCLUDED.provider_file_id, \
                mime_type        = EXCLUDED.mime_type, \
                filename         = EXCLUDED.filename, \
                size_bytes       = EXCLUDED.size_bytes, \
                uploaded_at      = EXCLUDED.uploaded_at, \
                expires_at       = EXCLUDED.expires_at, \
                last_used_at     = NOW()",
        )
        .bind(&entry.document_id)
        .bind(&provider_str)
        .bind(&entry.provider_file_id)
        .bind(&entry.mime_type)
        .bind(&entry.filename)
        .bind(entry.size_bytes)
        .bind(entry.uploaded_at)
        .bind(entry.expires_at)
        .bind(entry.last_used_at)
        .execute(&*self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed {
            message: format!("provider_file_cache upsert failed: {}", e),
        })?;
        Ok(())
    }

    async fn invalidate(&self, document_id: &str, provider: ProviderKind) -> Result<(), LlmError> {
        let provider_str = provider.to_string();
        sqlx::query("DELETE FROM provider_file_cache WHERE document_id = $1 AND provider = $2")
            .bind(document_id)
            .bind(&provider_str)
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
    ///
    /// Each invocation generates a unique `test-<uuid>-` prefix so parallel
    /// test execution does not race on shared cleanup. The closure receives
    /// the cache and the prefix; cleanup deletes only rows under that prefix.
    async fn with_cache<F, Fut>(f: F)
    where
        F: FnOnce(PostgresFileCache, String) -> Fut,
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
        let prefix = format!("test-{}-", uuid::Uuid::new_v4());
        // Clean state for this prefix only (parallel-safe).
        sqlx::query("DELETE FROM provider_file_cache WHERE document_id LIKE $1")
            .bind(format!("{}%", prefix))
            .execute(&*pool)
            .await
            .unwrap();
        f(cache, prefix).await;
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
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn lookup_miss_returns_none() {
        with_cache(|cache, prefix| async move {
            let r = cache
                .lookup(&format!("{}missing", prefix), ProviderKind::Anthropic)
                .await
                .unwrap();
            assert!(r.is_none());
        })
        .await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn upsert_then_lookup_returns_entry() {
        with_cache(|cache, prefix| async move {
            let doc_id = format!("{}1", prefix);
            let entry = fixture(&doc_id);
            cache.upsert(&entry).await.unwrap();
            let got = cache
                .lookup(&doc_id, ProviderKind::Anthropic)
                .await
                .unwrap();
            assert!(got.is_some());
            assert_eq!(got.unwrap().provider_file_id, "file_abc");
        })
        .await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn upsert_twice_updates_and_touches_last_used_at() {
        with_cache(|cache, prefix| async move {
            let doc_id = format!("{}2", prefix);
            let mut entry = fixture(&doc_id);
            let original_last_used = entry.last_used_at;
            cache.upsert(&entry).await.unwrap();
            // Force a small delay so NOW() advances.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            entry.provider_file_id = "file_xyz".into();
            // Pass a stale last_used_at to confirm the DB ignores it.
            entry.last_used_at = original_last_used;
            cache.upsert(&entry).await.unwrap();
            let got = cache
                .lookup(&doc_id, ProviderKind::Anthropic)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(got.provider_file_id, "file_xyz");
            assert!(
                got.last_used_at > original_last_used,
                "last_used_at should be touched by NOW(), got {} vs original {}",
                got.last_used_at,
                original_last_used
            );
        })
        .await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn lookup_advances_last_used_at_on_cache_hit() {
        with_cache(|cache, prefix| async move {
            let doc_id = format!("{}touch", prefix);
            let entry = fixture(&doc_id);
            let original_last_used = entry.last_used_at;
            cache.upsert(&entry).await.unwrap();

            // Force NOW() to advance past the upsert timestamp.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            let got = cache
                .lookup(&doc_id, ProviderKind::Anthropic)
                .await
                .unwrap()
                .expect("should hit");

            assert!(
                got.last_used_at > original_last_used,
                "lookup should touch last_used_at via UPDATE...RETURNING; got {} vs original {}",
                got.last_used_at,
                original_last_used
            );
            // last_used_at must be strictly after uploaded_at — confirms the
            // UPDATE didn't accidentally rewrite uploaded_at.
            assert!(got.last_used_at > got.uploaded_at);
            assert_eq!(got.provider_file_id, "file_abc");
        })
        .await;
    }

    #[test]
    fn parse_provider_from_row_accepts_known_kinds() {
        for s in ["anthropic", "openai", "google", "mock"] {
            let parsed = parse_provider_from_row(s, "doc-1").unwrap();
            assert_eq!(parsed.to_string(), s);
        }
    }

    #[test]
    fn parse_provider_from_row_fails_on_corrupted_string() {
        // Antes: este caso devolvía silenciosamente `ProviderKind::Mock`. Ahora
        // propaga `LlmError::RequestFailed` con el provider y el document_id
        // en el mensaje, para que el operador pueda invalidar la fila a mano.
        let err = parse_provider_from_row("definitely-not-a-provider", "doc-corrupt")
            .expect_err("corrupted provider must fail-fast");
        match err {
            LlmError::RequestFailed { message } => {
                assert!(message.contains("corrupted provider"));
                assert!(message.contains("definitely-not-a-provider"));
                assert!(message.contains("doc-corrupt"));
            }
            other => panic!("expected RequestFailed, got {:?}", other),
        }
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn invalidate_removes() {
        with_cache(|cache, prefix| async move {
            let doc_id = format!("{}3", prefix);
            let entry = fixture(&doc_id);
            cache.upsert(&entry).await.unwrap();
            cache
                .invalidate(&doc_id, ProviderKind::Anthropic)
                .await
                .unwrap();
            let got = cache
                .lookup(&doc_id, ProviderKind::Anthropic)
                .await
                .unwrap();
            assert!(got.is_none());
        })
        .await;
    }
}
