//! `ColmenaEngine`: process-wide entry point for DAG execution.
//!
//! Owns the shared `PgPoolRegistry`, the pinned internal-DB pool, state + secure
//! value repositories, the node registry, and the `DagRunUseCase`. Consumers
//! (CLI, HTTP worker, `run_dag`/`serve_dag`) build one per process.

use crate::dag_engine::application::run_use_case::DagRunUseCase;
use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::error::DagError;
use crate::dag_engine::domain::events::DagExecutionEvent;
use crate::dag_engine::domain::graph::Graph;
use crate::dag_engine::domain::state::DagTaskMemoryRepository;
use crate::dag_engine::infrastructure::persistence::postgres_dag_state_repository::PostgresDagStateRepository;
use crate::dag_engine::infrastructure::persistence::PostgresSecureValueRepository;
use crate::dag_engine::infrastructure::pool_registry::{
    ConfigError, PgPoolRegistry, PoolConfig, RegistryError, RegistryMetrics,
};
use crate::dag_engine::infrastructure::registry::HashMapNodeRegistry;
use crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;
use crate::llm::infrastructure::persistence::repository_factory::ConversationRepositoryFactory;

use futures::Stream;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("config error: {0}")]
    Config(#[from] ConfigError),
    #[error("registry error: {0}")]
    Registry(#[from] RegistryError),
    #[error("migration failed: {0}")]
    Migration(String),
    #[error("{0}")]
    Other(String),
}

pub struct EngineConfig {
    pub internal_database_url: String,
    pub pool_config: PoolConfig,
}

impl EngineConfig {
    pub fn from_env() -> Result<Self, EngineError> {
        let internal_database_url = std::env::var("DATABASE_URL").map_err(|_| {
            EngineError::Other("DATABASE_URL must be set to build ColmenaEngine".to_string())
        })?;
        let pool_config = PoolConfig::from_env()?;
        Ok(Self {
            internal_database_url,
            pool_config,
        })
    }
}

pub struct ColmenaEngine {
    registry: Arc<PgPoolRegistry>,
    use_case: Arc<DagRunUseCase>,
    closed: AtomicBool,
}

impl ColmenaEngine {
    /// Build the engine: pin the internal pool, migrate state + secure-values
    /// schemas on it, build the node registry, and wire the `DagRunUseCase`.
    pub async fn new(config: EngineConfig) -> Result<Self, EngineError> {
        let registry = Arc::new(PgPoolRegistry::new(config.pool_config));

        // Pin the internal DB. The returned Arc<PgPool> is the sole Postgres
        // connection pool used by state + secure-value repositories, and is
        // shared with any graph node that happens to reference the same URL.
        let internal_pool = registry.pin(&config.internal_database_url).await?;

        let state_repo = Arc::new(PostgresDagStateRepository::new((*internal_pool).clone()));
        state_repo
            .migrate()
            .await
            .map_err(|e| EngineError::Migration(e.to_string()))?;

        let secure_value_repo =
            Arc::new(PostgresSecureValueRepository::new((*internal_pool).clone()));
        secure_value_repo
            .migrate()
            .await
            .map_err(|e| EngineError::Migration(e.to_string()))?;

        let secure_value_service = Arc::new(SecureValueService::new(secure_value_repo));

        let conversation_factory = Arc::new(ConversationRepositoryFactory::new(registry.clone()));
        let sql_port_factory = Arc::new(SqlPortFactory::new(registry.clone()));

        let node_registry = HashMapNodeRegistry::new_with_secure_values(
            conversation_factory,
            sql_port_factory,
            Some(state_repo.clone() as Arc<dyn DagTaskMemoryRepository>),
            Some(secure_value_service.clone()),
        );

        let use_case = Arc::new(DagRunUseCase::with_secure_values_and_service(
            node_registry.clone(),
            Some(state_repo.clone()),
            secure_value_service,
        ));
        node_registry.set_subgraph_executor(use_case.clone());

        tracing::info!(
            target = "colmena::engine",
            pinned_pool_count = 1,
            "engine_started"
        );

        Ok(Self {
            registry,
            use_case,
            closed: AtomicBool::new(false),
        })
    }

    pub async fn run_dag(
        &self,
        graph: Graph,
        resume_session_id: Option<String>,
        resume_answer: Option<String>,
        include_extra_info: bool,
    ) -> Result<Value, DagError> {
        self.use_case
            .execute(graph, resume_session_id, resume_answer, include_extra_info)
            .await
    }

    pub fn execute_stream(
        &self,
        graph: Graph,
        resume_session_id: Option<String>,
        resume_answer: Option<String>,
        include_extra_info: bool,
    ) -> impl Stream<Item = Result<DagExecutionEvent, DagError>> + Send + '_ {
        (*self.use_case).clone().execute_stream(
            graph,
            resume_session_id,
            resume_answer,
            include_extra_info,
        )
    }

    pub fn registry_metrics(&self) -> RegistryMetrics {
        self.registry.snapshot_metrics()
    }

    /// Close every pool in the registry. Idempotent.
    pub async fn shutdown(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let start = std::time::Instant::now();
        let pool_count = self.registry.snapshot_metrics().cached_pools;
        self.registry.close_all().await;
        tracing::info!(
            target = "colmena::engine",
            pools_closed = pool_count,
            duration_ms = start.elapsed().as_millis() as u64,
            "engine_shutdown"
        );
    }
}

impl Drop for ColmenaEngine {
    fn drop(&mut self) {
        if !self.closed.load(Ordering::SeqCst) {
            tracing::warn!(
                target = "colmena::engine",
                "engine_dropped_without_shutdown"
            );
        }
    }
}
