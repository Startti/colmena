use crate::dag_engine::domain::{error::DagError, secure_value_repository::SecureValueRepository};
use async_trait::async_trait;
use sqlx::{PgPool, Row};

/// PostgreSQL implementation using pgcrypto for AES-256 encryption
pub struct PostgresSecureValueRepository {
    pool: PgPool,
}

impl PostgresSecureValueRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Ensure pgcrypto extension is available (safety net for environments where
    /// the migration ran without superuser). The table itself is created by
    /// migrations/postgres/20260425000002_secure_value_mappings.sql.
    pub async fn migrate(&self) -> Result<(), DagError> {
        sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
            .execute(&self.pool)
            .await
            .map_err(|e| DagError::StateError(format!("Failed to enable pgcrypto: {}", e)))?;

        Ok(())
    }
}

#[async_trait]
impl SecureValueRepository for PostgresSecureValueRepository {
    async fn persist(
        &self,
        session_id: &str,
        source_node_id: &str,
        hash_key: &str,
        real_value: &str,
        field_name: &str,
    ) -> Result<(), DagError> {
        let encryption_key =
            std::env::var("SECURE_VALUES_KEY").unwrap_or_else(|_| "default-key".to_string());

        sqlx::query(
            r#"
            INSERT INTO secure_value_mappings
                (session_id, source_node_id, hash_key, encrypted_value, field_name)
            VALUES ($1, $2, $3, pgp_sym_encrypt($4::text, $5), $6)
            ON CONFLICT (session_id, hash_key) DO UPDATE SET
                encrypted_value = EXCLUDED.encrypted_value,
                expires_at = NOW() + INTERVAL '1 hour'
            "#,
        )
        .bind(session_id)
        .bind(source_node_id)
        .bind(hash_key)
        .bind(real_value)
        .bind(&encryption_key)
        .bind(field_name)
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Failed to persist secure value: {}", e)))?;

        Ok(())
    }

    async fn decrypt(&self, session_id: &str, hash_key: &str) -> Result<Option<String>, DagError> {
        let encryption_key =
            std::env::var("SECURE_VALUES_KEY").unwrap_or_else(|_| "default-key".to_string());

        let row = sqlx::query(
            r#"
            SELECT pgp_sym_decrypt(encrypted_value, $1) as decrypted
            FROM secure_value_mappings
            WHERE session_id = $2 AND hash_key = $3
            "#,
        )
        .bind(&encryption_key)
        .bind(session_id)
        .bind(hash_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Failed to decrypt value: {}", e)))?;

        Ok(row.map(|r| r.get::<String, _>("decrypted")))
    }

    async fn cleanup(&self, session_id: &str) -> Result<(), DagError> {
        sqlx::query("DELETE FROM secure_value_mappings WHERE session_id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DagError::StateError(format!("Cleanup failed: {}", e)))?;

        Ok(())
    }

    async fn cleanup_expired(&self) -> Result<u64, DagError> {
        let result = sqlx::query("DELETE FROM secure_value_mappings WHERE expires_at < NOW()")
            .execute(&self.pool)
            .await
            .map_err(|e| DagError::StateError(format!("Cleanup expired failed: {}", e)))?;

        Ok(result.rows_affected())
    }
}
