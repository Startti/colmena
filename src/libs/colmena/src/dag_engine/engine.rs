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
use crate::llm::domain::AttachmentRegistry;
use crate::llm::infrastructure::persistence::repository_factory::ConversationRepositoryFactory;
use crate::llm::infrastructure::persistence::PostgresAttachmentRegistry;
use crate::storage::domain::OutputStorageRepository;
use crate::storage::infrastructure::{
    HttpCallbackStorageAdapter, LocalCacheStorageAdapter, LocalHttpStorageAdapter,
};

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
    /// Adapter used by media-generation nodes (image, audio) to persist their
    /// output. Defaults to [`LocalCacheStorageAdapter`] when no callback env
    /// vars are set.
    pub storage: Arc<dyn OutputStorageRepository>,
    /// Shared attachment registry. When present, media-generation nodes
    /// register their outputs here so `load_attachment` can later resolve them
    /// (capability 1 — agent opens its own generation). Always present when
    /// built via `from_env` because the engine requires `DATABASE_URL`.
    pub attachment_registry: Option<Arc<dyn AttachmentRegistry>>,
}

/// Parse a string env value as a boolean. Returns `Some(true)` for
/// "true"/"1"/"yes" (case-insensitive), `Some(false)` for "false"/"0"/"no",
/// and `None` for anything else (including unset).
fn parse_bool_env(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .and_then(|v| match v.trim().to_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        })
}

impl EngineConfig {
    /// Build the engine config from environment variables. **Async** because
    /// `LocalHttpStorageAdapter` (dev mode with disk + HTTP server) needs to
    /// bind a TCP listener during construction.
    ///
    /// ## Storage adapter selection
    ///
    /// The explicit guard rail is `COLMENA_LOCAL` (true/false):
    ///
    /// | `COLMENA_LOCAL` | Behavior |
    /// |---|---|
    /// | `true`  | Force [`LocalHttpStorageAdapter`]. Defaults: `dir=/tmp/colmena-out`, `port=8765`. |
    /// | `false` | Force [`HttpCallbackStorageAdapter`]. Hard-fail if `COLMENA_STORAGE_CALLBACK_URL` or `..._SECRET` are missing — prevents a prod deploy from silently falling back to dev/in-memory storage. |
    /// | unset   | Implicit fallback (back-compat): use callback if both vars set, else local-http if dir set, else in-memory. |
    ///
    /// Sub-env vars:
    /// - `COLMENA_STORAGE_CALLBACK_URL` + `COLMENA_STORAGE_CALLBACK_SECRET` (prod)
    /// - `COLMENA_LOCAL_STORAGE_DIR` (default `/tmp/colmena-out` when `COLMENA_LOCAL=true`)
    /// - `COLMENA_LOCAL_STORAGE_PORT` (default `8765`; use `0` to let the OS pick)
    pub async fn from_env() -> Result<Self, EngineError> {
        let internal_database_url = std::env::var("DATABASE_URL").map_err(|_| {
            EngineError::Other("DATABASE_URL must be set to build ColmenaEngine".to_string())
        })?;
        let pool_config = PoolConfig::from_env()?;

        let local_flag = parse_bool_env("COLMENA_LOCAL");

        let storage: Arc<dyn OutputStorageRepository> = match local_flag {
            // --- Explicit local: force LocalHttpStorageAdapter with defaults ---
            Some(true) => {
                let dir = std::env::var("COLMENA_LOCAL_STORAGE_DIR")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "/tmp/colmena-out".to_string());
                let port = std::env::var("COLMENA_LOCAL_STORAGE_PORT")
                    .ok()
                    .and_then(|p| p.parse::<u16>().ok())
                    .unwrap_or(8765);
                let adapter = LocalHttpStorageAdapter::new(std::path::PathBuf::from(&dir), port)
                    .await
                    .map_err(|e| EngineError::Other(format!("LocalHttpStorageAdapter: {}", e)))?;
                tracing::info!(
                    target: "colmena::engine",
                    mode = "local",
                    adapter = "LocalHttpStorageAdapter",
                    dir = %dir,
                    port = adapter.port(),
                    "storage_mode_selected (COLMENA_LOCAL=true)"
                );
                Arc::new(adapter)
            }
            // --- Explicit prod: REQUIRE callback vars; loud failure if missing ---
            Some(false) => {
                let url = std::env::var("COLMENA_STORAGE_CALLBACK_URL")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        EngineError::Other(
                            "COLMENA_LOCAL=false (prod mode) requires \
                             COLMENA_STORAGE_CALLBACK_URL to be set"
                                .to_string(),
                        )
                    })?;
                let secret = std::env::var("COLMENA_STORAGE_CALLBACK_SECRET")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        EngineError::Other(
                            "COLMENA_LOCAL=false (prod mode) requires \
                             COLMENA_STORAGE_CALLBACK_SECRET to be set"
                                .to_string(),
                        )
                    })?;
                tracing::info!(
                    target: "colmena::engine",
                    mode = "prod",
                    adapter = "HttpCallbackStorageAdapter",
                    callback_url = %url,
                    "storage_mode_selected (COLMENA_LOCAL=false)"
                );
                Arc::new(HttpCallbackStorageAdapter::new(url, secret))
            }
            // --- Unset: implicit back-compat fallback ---
            None => match (
                std::env::var("COLMENA_STORAGE_CALLBACK_URL").ok(),
                std::env::var("COLMENA_STORAGE_CALLBACK_SECRET").ok(),
            ) {
                (Some(url), Some(secret)) if !url.is_empty() && !secret.is_empty() => {
                    tracing::info!(
                        target: "colmena::engine",
                        mode = "implicit-prod",
                        adapter = "HttpCallbackStorageAdapter",
                        "storage_mode_selected (callback vars set; consider setting COLMENA_LOCAL=false explicitly)"
                    );
                    Arc::new(HttpCallbackStorageAdapter::new(url, secret))
                }
                _ => match std::env::var("COLMENA_LOCAL_STORAGE_DIR").ok() {
                    Some(dir) if !dir.is_empty() => {
                        let port = std::env::var("COLMENA_LOCAL_STORAGE_PORT")
                            .ok()
                            .and_then(|p| p.parse::<u16>().ok())
                            .unwrap_or(8765);
                        let adapter =
                            LocalHttpStorageAdapter::new(std::path::PathBuf::from(&dir), port)
                                .await
                                .map_err(|e| {
                                    EngineError::Other(format!("LocalHttpStorageAdapter: {}", e))
                                })?;
                        tracing::info!(
                            target: "colmena::engine",
                            mode = "implicit-local",
                            adapter = "LocalHttpStorageAdapter",
                            dir = %dir,
                            port = adapter.port(),
                            "storage_mode_selected (LOCAL_STORAGE_DIR set; consider setting COLMENA_LOCAL=true explicitly)"
                        );
                        Arc::new(adapter)
                    }
                    _ => {
                        tracing::info!(
                            target: "colmena::engine",
                            mode = "implicit-in-memory",
                            adapter = "LocalCacheStorageAdapter",
                            "storage_mode_selected (no storage env vars; in-memory bytes lost on shutdown)"
                        );
                        Arc::new(LocalCacheStorageAdapter::new())
                    }
                },
            },
        };

        Ok(Self {
            internal_database_url,
            pool_config,
            storage,
            // attachment_registry is built lazily inside ColmenaEngine::new
            // (after the pool is pinned), so leave it None here.
            attachment_registry: None,
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

        // Run all schema migrations on the internal pool before any repo touches
        // the database. This ensures tables exist before ALTER TABLE / CREATE TABLE
        // safety-net calls in repo migrate() methods run.
        let mut migrator = sqlx::migrate!("migrations/postgres");
        migrator.set_ignore_missing(true);
        migrator
            .run(&*internal_pool)
            .await
            .map_err(|e| EngineError::Migration(format!("Schema migration failed: {}", e)))?;

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

        // Build the attachment registry from the same internal pool, so media
        // nodes can register their outputs and `load_attachment` can later
        // resolve them. If the explicit `config.attachment_registry` was
        // provided (e.g. by tests), use that; otherwise build the canonical
        // Postgres one.
        let attachment_registry: Arc<dyn AttachmentRegistry> = if let Some(reg) =
            config.attachment_registry.clone()
        {
            reg
        } else {
            let reg =
                PostgresAttachmentRegistry::new(registry.clone(), &config.internal_database_url)
                    .await
                    .map_err(|e| EngineError::Other(format!("attachment registry init: {}", e)))?;
            Arc::new(reg)
        };

        let node_registry = HashMapNodeRegistry::new_with_secure_values(
            conversation_factory,
            sql_port_factory,
            Some(state_repo.clone() as Arc<dyn DagTaskMemoryRepository>),
            Some(secure_value_service.clone()),
            Some(config.storage.clone()),
            Some(attachment_registry.clone()),
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
        _agent_session_id: Option<String>,
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
        path_prefix: Option<String>,
        agent_session_id: Option<String>,
    ) -> impl Stream<Item = Result<DagExecutionEvent, DagError>> + Send + '_ {
        (*self.use_case).clone().execute_stream(
            graph,
            resume_session_id,
            resume_answer,
            include_extra_info,
            path_prefix,
            agent_session_id,
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

#[cfg(test)]
mod env_guard_rail_tests {
    //! Tests for the `COLMENA_LOCAL` env guard rail. These tests mutate
    //! process-wide environment variables and therefore MUST run serially.
    use super::parse_bool_env;
    use serial_test::serial;

    /// Helper: clear all storage-relevant env vars before each test so we
    /// start from a known state.
    fn clean_env() {
        for v in [
            "COLMENA_LOCAL",
            "COLMENA_STORAGE_CALLBACK_URL",
            "COLMENA_STORAGE_CALLBACK_SECRET",
            "COLMENA_LOCAL_STORAGE_DIR",
            "COLMENA_LOCAL_STORAGE_PORT",
        ] {
            std::env::remove_var(v);
        }
    }

    #[test]
    #[serial]
    fn parse_bool_env_recognizes_truthy_values() {
        clean_env();
        for val in ["true", "TRUE", "1", "yes", "on", "  True  "] {
            std::env::set_var("COLMENA_LOCAL", val);
            assert_eq!(
                parse_bool_env("COLMENA_LOCAL"),
                Some(true),
                "value '{val}' should parse as true"
            );
        }
        clean_env();
    }

    #[test]
    #[serial]
    fn parse_bool_env_recognizes_falsy_values() {
        clean_env();
        for val in ["false", "FALSE", "0", "no", "off"] {
            std::env::set_var("COLMENA_LOCAL", val);
            assert_eq!(
                parse_bool_env("COLMENA_LOCAL"),
                Some(false),
                "value '{val}' should parse as false"
            );
        }
        clean_env();
    }

    #[test]
    #[serial]
    fn parse_bool_env_returns_none_for_unset() {
        clean_env();
        assert_eq!(parse_bool_env("COLMENA_LOCAL"), None);
    }

    #[test]
    #[serial]
    fn parse_bool_env_returns_none_for_garbage() {
        clean_env();
        std::env::set_var("COLMENA_LOCAL", "maybe");
        assert_eq!(parse_bool_env("COLMENA_LOCAL"), None);
        clean_env();
    }
}
