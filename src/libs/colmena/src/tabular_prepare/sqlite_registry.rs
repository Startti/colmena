//! SQLite implementation of [`PreparationRegistry`].

use super::registry::*;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::sync::Arc;

/// Timestamps are bound as `DateTime<Utc>`, which SQLx stores as RFC 3339
/// text; the claim rule compares them as text, which orders correctly because
/// every value shares one format and offset.
pub struct SqlitePreparationRegistry {
    pool: Arc<SqlitePool>,
}

impl SqlitePreparationRegistry {
    pub fn from_pool(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    #[cfg(test)]
    pub(crate) fn pool(&self) -> Arc<SqlitePool> {
        self.pool.clone()
    }
}

impl SqlitePreparationRegistry {
    /// Blob keys the row tracks now (empty when there is no row).
    async fn tracked_keys(&self, source_key: &str) -> Result<Vec<String>, RegistryError> {
        Ok(self
            .get(source_key)
            .await?
            .map(|row| row.blob_keys)
            .unwrap_or_default())
    }
}

#[async_trait]
impl PreparationRegistry for SqlitePreparationRegistry {
    async fn claim(&self, req: ClaimRequest) -> Result<Option<Claim>, RegistryError> {
        let row = sqlx::query_scalar::<_, i32>(&for_sqlite(CLAIM_SQL))
            .bind(&req.source_key)
            .bind(req.format_version)
            .bind(req.source_bytes)
            .bind(&req.owner)
            .bind(req.now + req.lease)
            .bind(req.now)
            .bind(MAX_ATTEMPTS)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("claim", e))?;
        Ok(row.map(|attempts| Claim { attempts }))
    }

    async fn complete(
        &self,
        source_key: &str,
        owner: &str,
        info: ReadyInfo,
        now: DateTime<Utc>,
    ) -> Result<TerminalOutcome, RegistryError> {
        let tracked = self.tracked_keys(source_key).await?;
        let row = sqlx::query_scalar::<_, String>(&for_sqlite(COMPLETE_SQL))
            .bind(source_key)
            .bind(owner)
            .bind(&info.manifest_key)
            .bind(blob_keys_to_json(&merge_keys(&tracked, &info.blob_keys))?)
            .bind(&info.tables_json)
            .bind(info.prepared_bytes)
            .bind(now)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("complete", e))?;
        Ok(match row {
            Some(_) => TerminalOutcome::Written,
            None => TerminalOutcome::Cancelled,
        })
    }

    async fn fail_with_blobs(
        &self,
        source_key: &str,
        owner: &str,
        error_code: &str,
        error_detail: &str,
        blob_keys: &[String],
        now: DateTime<Utc>,
    ) -> Result<TerminalOutcome, RegistryError> {
        let tracked = self.tracked_keys(source_key).await?;
        let row = sqlx::query_scalar::<_, String>(&for_sqlite(FAIL_SQL))
            .bind(source_key)
            .bind(owner)
            .bind(error_code)
            .bind(error_detail)
            .bind(blob_keys_to_json(&merge_keys(&tracked, blob_keys))?)
            .bind(now)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("fail", e))?;
        Ok(match row {
            Some(_) => TerminalOutcome::Written,
            None => TerminalOutcome::Cancelled,
        })
    }

    async fn still_owned(&self, source_key: &str, owner: &str) -> Result<bool, RegistryError> {
        let row = sqlx::query_scalar::<_, i32>(&for_sqlite(STILL_OWNED_SQL))
            .bind(source_key)
            .bind(owner)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("still_owned", e))?;
        Ok(row.is_some())
    }

    async fn release(&self, source_key: &str, owner: &str) -> Result<bool, RegistryError> {
        let done = sqlx::query(&for_sqlite(RELEASE_SQL))
            .bind(source_key)
            .bind(owner)
            .execute(&*self.pool)
            .await
            .map_err(|e| backend_err("release", e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn delete(&self, source_key: &str) -> Result<bool, RegistryError> {
        let done = sqlx::query(&for_sqlite(DELETE_SQL))
            .bind(source_key)
            .execute(&*self.pool)
            .await
            .map_err(|e| backend_err("delete", e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn touch_last_used(
        &self,
        source_key: &str,
        now: DateTime<Utc>,
        min_interval: chrono::Duration,
    ) -> Result<bool, RegistryError> {
        let row = sqlx::query_scalar::<_, String>(&for_sqlite(TOUCH_LAST_USED_SQL))
            .bind(source_key)
            .bind(now)
            .bind(now - min_interval)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("touch_last_used", e))?;
        Ok(row.is_some())
    }

    async fn get(&self, source_key: &str) -> Result<Option<PreparedRow>, RegistryError> {
        let row = sqlx::query(&for_sqlite(GET_SQL))
            .bind(source_key)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("get", e))?;
        row.map(|row| Ok(prepared_row_from!(&row))).transpose()
    }
}
