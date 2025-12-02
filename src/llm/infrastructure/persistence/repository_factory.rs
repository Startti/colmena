use crate::llm::domain::{ConversationRepository, LlmError};
use crate::llm::infrastructure::persistence::{
    PostgresConversationRepository, SqliteConversationRepository,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use std::collections::HashMap;
use std::str::FromStr;

use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct ConversationRepositoryFactory {
    // Cache de pools para no reconectar en cada llamada.
    // Usamos Mutex<HashMap> para acceso concurrente seguro.
    // Clave: Connection URL, Valor: Repository
    repositories: Arc<Mutex<HashMap<String, Arc<dyn ConversationRepository>>>>,
}

impl ConversationRepositoryFactory {
    pub fn new() -> Self {
        Self {
            repositories: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for ConversationRepositoryFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationRepositoryFactory {
    pub async fn get_repository(
        &self,
        connection_url: &str,
    ) -> Result<Arc<dyn ConversationRepository>, LlmError> {
        let mut repos = self.repositories.lock().await;

        if let Some(repo) = repos.get(connection_url) {
            return Ok(repo.clone());
        }

        println!("🔌 Conectando a nueva base de datos: {}", connection_url);

        let repo: Arc<dyn ConversationRepository> = if connection_url.starts_with("postgres://")
            || connection_url.starts_with("postgresql://")
        {
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(connection_url)
                .await
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("Failed to connect to Postgres: {}", e),
                })?;

            // Run migrations
            sqlx::migrate!("./migrations/postgres")
                .run(&pool)
                .await
                .map_err(|e| LlmError::RequestFailed {
                    message: format!("Migration failed: {}", e),
                })?;

            Arc::new(PostgresConversationRepository::new(pool))
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

            // Run migrations
            sqlx::migrate!("./migrations/sqlite")
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
