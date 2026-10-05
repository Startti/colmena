//! SQLite implementation of [`PreparationRegistry`].

use super::registry::*;
use async_trait::async_trait;
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

    async fn get(&self, source_key: &str) -> Result<Option<PreparedRow>, RegistryError> {
        let row = sqlx::query(&for_sqlite(GET_SQL))
            .bind(source_key)
            .fetch_optional(&*self.pool)
            .await
            .map_err(|e| backend_err("get", e))?;
        row.map(|row| Ok(prepared_row_from!(&row))).transpose()
    }
}
