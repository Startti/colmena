//! Factory that builds `PgPoolAdapter` instances on top of shared registry pools.
//!
//! Each adapter keeps its own `statement_timeout_ms` / `work_mem_mb` (applied
//! per-query via `SET LOCAL`), so multiple nodes hitting the same URL with
//! different runtime limits do not interfere with each other.

use crate::dag_engine::domain::sql_errors::SqlNodeError;
use crate::dag_engine::infrastructure::pool_registry::PgPoolRegistry;
use crate::dag_engine::infrastructure::sql_pool_adapter::PgPoolAdapter;
use std::sync::Arc;

pub struct SqlPortFactory {
    registry: Arc<PgPoolRegistry>,
}

impl SqlPortFactory {
    pub fn new(registry: Arc<PgPoolRegistry>) -> Self {
        Self { registry }
    }

    /// Obtain a `PgPoolAdapter` wrapping the shared registry pool for `url`.
    pub async fn get_adapter(
        &self,
        url: &str,
        statement_timeout_ms: u64,
        work_mem_mb: u64,
    ) -> Result<Arc<PgPoolAdapter>, SqlNodeError> {
        let pool = self.registry.get_or_create(url).await.map_err(|e| {
            SqlNodeError::ConnectionError(format!("pool registry: {}", e))
        })?;
        Ok(Arc::new(PgPoolAdapter::new(
            pool,
            statement_timeout_ms,
            work_mem_mb,
        )))
    }
}
