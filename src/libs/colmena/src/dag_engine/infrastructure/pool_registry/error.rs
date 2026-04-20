use thiserror::Error;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("failed to create Postgres pool for URL: {message}")]
    PoolCreation { message: String },
    #[error("registry is closed")]
    Closed,
}

impl From<sqlx::Error> for RegistryError {
    fn from(e: sqlx::Error) -> Self {
        RegistryError::PoolCreation {
            message: e.to_string(),
        }
    }
}
