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
        agent_session_id: Option<&str>,
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
                (session_id, agent_session_id, source_node_id, hash_key, encrypted_value, field_name)
            VALUES ($1, $2, $3, $4, pgp_sym_encrypt($5::text, $6), $7)
            ON CONFLICT (session_id, hash_key) DO UPDATE SET
                encrypted_value = EXCLUDED.encrypted_value,
                agent_session_id = EXCLUDED.agent_session_id,
                expires_at = NOW() + INTERVAL '1 hour'
            "#,
        )
        .bind(session_id)
        .bind(agent_session_id)
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

    async fn decrypt(
        &self,
        session_id: &str,
        agent_session_id: Option<&str>,
        hash_key: &str,
    ) -> Result<Option<String>, DagError> {
        let encryption_key =
            std::env::var("SECURE_VALUES_KEY").unwrap_or_else(|_| "default-key".to_string());

        let row = if let Some(agent) = agent_session_id {
            sqlx::query(
                r#"
                SELECT pgp_sym_decrypt(encrypted_value, $1) as decrypted
                FROM secure_value_mappings
                WHERE agent_session_id = $2 AND hash_key = $3
                "#,
            )
            .bind(&encryption_key)
            .bind(agent)
            .bind(hash_key)
            .fetch_optional(&self.pool)
            .await
        } else {
            sqlx::query(
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
        }
        .map_err(|e| DagError::StateError(format!("Failed to decrypt value: {}", e)))?;

        Ok(row.map(|r| r.get::<String, _>("decrypted")))
    }

    async fn exists(
        &self,
        session_id: &str,
        agent_session_id: Option<&str>,
        hash_key: &str,
    ) -> Result<bool, DagError> {
        let exists: bool = if let Some(agent) = agent_session_id {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM secure_value_mappings WHERE agent_session_id = $1 AND hash_key = $2)"
            )
            .bind(agent)
            .bind(hash_key)
            .fetch_one(&self.pool)
            .await
        } else {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM secure_value_mappings WHERE session_id = $1 AND hash_key = $2)"
            )
            .bind(session_id)
            .bind(hash_key)
            .fetch_one(&self.pool)
            .await
        }
        .map_err(|e| {
            DagError::StateError(format!("secure_value_mappings exists query failed: {e}"))
        })?;
        Ok(exists)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn test_postgres_exists_returns_false_for_unknown_key() {
        use sqlx::postgres::PgPoolOptions;
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPoolOptions::new().connect(&url).await.unwrap();
        let repo = PostgresSecureValueRepository::new(pool);
        let exists = repo
            .exists("nonexistent_session_xyz", None, "<sv_nope>")
            .await
            .unwrap();
        assert!(!exists);
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn test_postgres_exists_returns_true_after_persist() {
        use sqlx::postgres::PgPoolOptions;
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPoolOptions::new().connect(&url).await.unwrap();
        let repo = PostgresSecureValueRepository::new(pool);
        let unique_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let session = format!("test_session_{unique_id}");
        repo.persist(&session, None, "node1", "<sv_x>", "secret_value", "test")
            .await
            .unwrap();
        assert!(repo.exists(&session, None, "<sv_x>").await.unwrap());
        repo.cleanup(&session).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn cross_session_lookup_via_agent_id() {
        use sqlx::postgres::PgPoolOptions;
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPoolOptions::new().connect(&url).await.unwrap();
        let repo = PostgresSecureValueRepository::new(pool);

        let session1 = format!(
            "xs_run1_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let session2 = format!(
            "xs_run2_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let agent_a = "agent_A1";
        let agent_b = "agent_A2";

        // Persist with session1 + agent_a.
        repo.persist(&session1, Some(agent_a), "setup_node", "<sv_xs>", "the-real-value", "secret")
            .await
            .unwrap();

        // Decrypt from session2 (DIFFERENT) but same agent_a → must find.
        let v = repo.decrypt(&session2, Some(agent_a), "<sv_xs>").await.unwrap();
        assert_eq!(v.as_deref(), Some("the-real-value"));

        // Decrypt with same session2 but different agent_b → None.
        let v = repo.decrypt(&session2, Some(agent_b), "<sv_xs>").await.unwrap();
        assert!(v.is_none());

        // exists also works
        assert!(repo.exists(&session2, Some(agent_a), "<sv_xs>").await.unwrap());
        assert!(!repo.exists(&session2, Some(agent_b), "<sv_xs>").await.unwrap());

        repo.cleanup(&session1).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn legacy_session_only_lookup_still_works() {
        use sqlx::postgres::PgPoolOptions;
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPoolOptions::new().connect(&url).await.unwrap();
        let repo = PostgresSecureValueRepository::new(pool);

        let session1 = format!(
            "legacy_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        // Persist WITHOUT agent_session_id.
        repo.persist(&session1, None, "setup_node", "<sv_legacy>", "ephem-only", "secret")
            .await
            .unwrap();

        // Decrypt with same session1, no agent → finds.
        let v = repo.decrypt(&session1, None, "<sv_legacy>").await.unwrap();
        assert_eq!(v.as_deref(), Some("ephem-only"));

        // Decrypt with different session, no agent → None.
        let session2 = format!(
            "legacy2_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let v = repo.decrypt(&session2, None, "<sv_legacy>").await.unwrap();
        assert!(v.is_none());

        repo.cleanup(&session1).await.unwrap();
    }
}
