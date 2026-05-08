use crate::dag_engine::domain::error::DagError;
use async_trait::async_trait;

/// Repository trait for managing secure values (encrypted storage)
/// Implementations handle AES-256 encryption/decryption
#[async_trait]
pub trait SecureValueRepository: Send + Sync {
    /// Store a sensitive value with encryption
    ///
    /// # Arguments
    /// * `session_id` - DAG execution session ID
    /// * `source_node_id` - ID of the HTTP node that generated this value
    /// * `hash_key` - Placeholder identifier (e.g., "<token_1>")
    /// * `real_value` - The actual sensitive value to encrypt
    /// * `field_name` - Human-readable field name for auditing
    async fn persist(
        &self,
        session_id: &str,
        source_node_id: &str,
        hash_key: &str,
        real_value: &str,
        field_name: &str,
    ) -> Result<(), DagError>;

    /// Retrieve and decrypt a value by its hash key
    /// Returns None if the hash key doesn't exist
    async fn decrypt(&self, session_id: &str, hash_key: &str) -> Result<Option<String>, DagError>;

    /// Check whether a hash_key already exists in this session.
    ///
    /// Production impls should override with a direct existence check (e.g. SQL
    /// `EXISTS`) so that callers checking only for presence do not pay
    /// decryption cost or transiently materialize the secret in memory. The
    /// default implementation calls `decrypt` and discards the value — correct
    /// but suboptimal for that goal.
    async fn exists(&self, session_id: &str, hash_key: &str) -> Result<bool, DagError> {
        Ok(self.decrypt(session_id, hash_key).await?.is_some())
    }

    /// Delete all secure values for a session (cleanup after DAG)
    async fn cleanup(&self, session_id: &str) -> Result<(), DagError>;

    /// Delete expired values (safety net, called periodically)
    async fn cleanup_expired(&self) -> Result<u64, DagError>;
}
