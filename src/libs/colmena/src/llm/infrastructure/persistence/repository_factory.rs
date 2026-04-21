use crate::dag_engine::infrastructure::pool_registry::PgPoolRegistry;
use crate::llm::domain::{ConversationRepository, LlmError};
use crate::llm::infrastructure::persistence::{
    PostgresConversationRepository, SqliteConversationRepository,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Factory that returns `ConversationRepository` instances keyed by connection URL.
///
/// For Postgres URLs the pool is obtained from the shared `PgPoolRegistry` — so
/// all LLM memory operations share pools with state persistence, secure values,
/// and SQL nodes pointing at the same URL. SQLite repositories are still owned
/// per-URL by this factory (SQLite has no central pool concept).
#[derive(Clone)]
pub struct ConversationRepositoryFactory {
    registry: Arc<PgPoolRegistry>,
    repositories: Arc<Mutex<HashMap<String, Arc<dyn ConversationRepository>>>>,
}

impl ConversationRepositoryFactory {
    pub fn new(registry: Arc<PgPoolRegistry>) -> Self {
        Self {
            registry,
            repositories: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get_repository(
        &self,
        connection_url: &str,
    ) -> Result<Arc<dyn ConversationRepository>, LlmError> {
        let mut repos = self.repositories.lock().await;
        if let Some(repo) = repos.get(connection_url) {
            return Ok(repo.clone());
        }

        let repo: Arc<dyn ConversationRepository> = if connection_url.starts_with("postgres://")
            || connection_url.starts_with("postgresql://")
        {
            let pool_arc = self
                .registry
                .get_or_create(connection_url)
                .await
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("Failed to get Postgres pool: {}", e),
                })?;

            // Run migrations (ignore missing: the DB may have old migrations
            // that no longer exist on disk from previous schema consolidations).
            let mut migrator = sqlx::migrate!("migrations/postgres");
            migrator.set_ignore_missing(true);
            migrator
                .run(&*pool_arc)
                .await
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("Migration failed: {}", e),
                })?;

            Arc::new(PostgresConversationRepository::new((*pool_arc).clone()))
        } else if connection_url.starts_with("sqlite://") {
            let options = SqliteConnectOptions::from_str(connection_url)
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("Invalid SQLite URL: {}", e),
                })?
                .create_if_missing(true);

            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("Failed to connect to SQLite: {}", e),
                })?;

            let mut migrator = sqlx::migrate!("migrations/sqlite");
            migrator.set_ignore_missing(true);
            migrator
                .run(&pool)
                .await
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("Migration failed: {}", e),
                })?;

            Arc::new(SqliteConversationRepository::new(pool))
        } else {
            return Err(LlmError::RequestFailed {
                message: format!("Unsupported database protocol in URL: {}", connection_url),
            });
        };

        repos.insert(connection_url.to_string(), repo.clone());
        Ok(repo)
    }
}
