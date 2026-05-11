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

        // Diagnostic: confirm the row is visible from the SAME pool right after insert.
        let visible: Option<(String,)> = sqlx::query_as(
            "SELECT hash_key FROM secure_value_mappings WHERE session_id = $1 AND hash_key = $2",
        )
        .bind(session_id)
        .bind(hash_key)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();
        tracing::info!(
            target: "colmena::secure_value_repo",
            session_id,
            hash_key,
            visible_after_insert = visible.is_some(),
            "postgres_persist: post-insert visibility probe"
        );

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
                UPDATE secure_value_mappings
                SET expires_at = NOW() + INTERVAL '24 hours'
                WHERE agent_session_id = $2
                  AND hash_key = $3
                  AND expires_at > NOW()
                RETURNING pgp_sym_decrypt(encrypted_value, $1)::text AS decrypted
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
                UPDATE secure_value_mappings
                SET expires_at = NOW() + INTERVAL '24 hours'
                WHERE session_id = $2
                  AND hash_key = $3
                  AND expires_at > NOW()
                RETURNING pgp_sym_decrypt(encrypted_value, $1)::text AS decrypted
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
                "SELECT EXISTS(SELECT 1 FROM secure_value_mappings WHERE agent_session_id = $1 AND hash_key = $2 AND expires_at > NOW())"
            )
            .bind(agent)
            .bind(hash_key)
            .fetch_one(&self.pool)
            .await
        } else {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM secure_value_mappings WHERE session_id = $1 AND hash_key = $2 AND expires_at > NOW())"
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

    async fn cleanup_expired_for_run(
        &self,
        session_id: &str,
        agent_session_id: Option<&str>,
    ) -> Result<u64, DagError> {
        let result = sqlx::query(
            r#"
            DELETE FROM secure_value_mappings
            WHERE expires_at < NOW()
              AND (
                    session_id = $1
                    OR ($2::text IS NOT NULL AND agent_session_id = $2)
                  )
            "#,
        )
        .bind(session_id)
        .bind(agent_session_id)
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("cleanup_expired_for_run failed: {}", e)))?;
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
        repo.persist(
            &session1,
            Some(agent_a),
            "setup_node",
            "<sv_xs>",
            "the-real-value",
            "secret",
        )
        .await
        .unwrap();

        // Decrypt from session2 (DIFFERENT) but same agent_a → must find.
        let v = repo
            .decrypt(&session2, Some(agent_a), "<sv_xs>")
            .await
            .unwrap();
        assert_eq!(v.as_deref(), Some("the-real-value"));

        // Decrypt with same session2 but different agent_b → None.
        let v = repo
            .decrypt(&session2, Some(agent_b), "<sv_xs>")
            .await
            .unwrap();
        assert!(v.is_none());

        // exists also works
        assert!(repo
            .exists(&session2, Some(agent_a), "<sv_xs>")
            .await
            .unwrap());
        assert!(!repo
            .exists(&session2, Some(agent_b), "<sv_xs>")
            .await
            .unwrap());

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
        repo.persist(
            &session1,
            None,
            "setup_node",
            "<sv_legacy>",
            "ephem-only",
            "secret",
        )
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

    #[tokio::test]
    #[ignore = "requires DATABASE_URL"]
    async fn decrypt_extends_expires_at() {
        dotenvy::dotenv().ok();
        let url = std::env::var("DATABASE_URL").unwrap();
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let repo = PostgresSecureValueRepository::new(pool.clone());

        let session = format!("ttl_test_{}", uuid::Uuid::new_v4());
        repo.persist(
            &session,
            None,
            "test_node",
            "<sv_short>",
            "alice123",
            "secret",
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE secure_value_mappings SET expires_at = NOW() + INTERVAL '10 seconds' WHERE session_id = $1",
        )
        .bind(&session)
        .execute(&pool)
        .await
        .unwrap();

        let pre: (chrono::DateTime<chrono::Utc>,) =
            sqlx::query_as("SELECT expires_at FROM secure_value_mappings WHERE session_id = $1")
                .bind(&session)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(pre.0 < chrono::Utc::now() + chrono::Duration::minutes(1));

        let value = repo.decrypt(&session, None, "<sv_short>").await.unwrap();
        assert_eq!(value, Some("alice123".to_string()));

        let post: (chrono::DateTime<chrono::Utc>,) =
            sqlx::query_as("SELECT expires_at FROM secure_value_mappings WHERE session_id = $1")
                .bind(&session)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            post.0 > chrono::Utc::now() + chrono::Duration::hours(23),
            "expires_at should be > now+23h, got {}",
            post.0
        );

        sqlx::query("DELETE FROM secure_value_mappings WHERE session_id = $1")
            .bind(&session)
            .execute(&pool)
            .await
            .ok();
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL"]
    async fn exists_returns_false_for_expired_row() {
        dotenvy::dotenv().ok();
        let url = std::env::var("DATABASE_URL").unwrap();
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let repo = PostgresSecureValueRepository::new(pool.clone());

        let session = format!("exists_expired_{}", uuid::Uuid::new_v4());
        repo.persist(&session, None, "test_node", "<sv_expired>", "alice123", "secret")
            .await
            .unwrap();
        sqlx::query(
            "UPDATE secure_value_mappings SET expires_at = NOW() - INTERVAL '1 second' WHERE session_id = $1",
        )
        .bind(&session)
        .execute(&pool)
        .await
        .unwrap();

        let exists = repo.exists(&session, None, "<sv_expired>").await.unwrap();
        assert!(!exists, "expired row should not be reported as existing");

        sqlx::query("DELETE FROM secure_value_mappings WHERE session_id = $1")
            .bind(&session)
            .execute(&pool)
            .await
            .ok();
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL"]
    async fn decrypt_returns_none_for_expired_row() {
        dotenvy::dotenv().ok();
        let url = std::env::var("DATABASE_URL").unwrap();
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let repo = PostgresSecureValueRepository::new(pool.clone());

        let session = format!("ttl_expired_{}", uuid::Uuid::new_v4());
        repo.persist(
            &session,
            None,
            "test_node",
            "<sv_expired>",
            "alice123",
            "secret",
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE secure_value_mappings SET expires_at = NOW() - INTERVAL '1 second' WHERE session_id = $1",
        )
        .bind(&session)
        .execute(&pool)
        .await
        .unwrap();

        let value = repo.decrypt(&session, None, "<sv_expired>").await.unwrap();
        assert!(value.is_none(), "expired row should not decrypt");

        sqlx::query("DELETE FROM secure_value_mappings WHERE session_id = $1")
            .bind(&session)
            .execute(&pool)
            .await
            .ok();
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL"]
    async fn cleanup_expired_for_run_deletes_only_expired_in_scope() {
        dotenvy::dotenv().ok();
        let url = std::env::var("DATABASE_URL").unwrap();
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let repo = PostgresSecureValueRepository::new(pool.clone());

        let my_session = format!("sweep_a_{}", uuid::Uuid::new_v4());
        let other_session = format!("sweep_b_{}", uuid::Uuid::new_v4());

        // (a) expired, this session
        repo.persist(&my_session, None, "n", "<sv_a>", "alice123", "secret").await.unwrap();
        sqlx::query("UPDATE secure_value_mappings SET expires_at = NOW() - INTERVAL '1 second' WHERE session_id = $1 AND hash_key = '<sv_a>'")
            .bind(&my_session).execute(&pool).await.unwrap();
        // (b) expired, OTHER session
        repo.persist(&other_session, None, "n", "<sv_b>", "alice123", "secret").await.unwrap();
        sqlx::query("UPDATE secure_value_mappings SET expires_at = NOW() - INTERVAL '1 second' WHERE session_id = $1 AND hash_key = '<sv_b>'")
            .bind(&other_session).execute(&pool).await.unwrap();
        // (c) not expired, this session
        repo.persist(&my_session, None, "n", "<sv_c>", "alice123", "secret").await.unwrap();

        let deleted = repo.cleanup_expired_for_run(&my_session, None).await.unwrap();
        assert_eq!(deleted, 1, "should delete exactly the (a) row");

        let a_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM secure_value_mappings WHERE session_id=$1 AND hash_key='<sv_a>')")
            .bind(&my_session).fetch_one(&pool).await.unwrap();
        let b_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM secure_value_mappings WHERE session_id=$1 AND hash_key='<sv_b>')")
            .bind(&other_session).fetch_one(&pool).await.unwrap();
        let c_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM secure_value_mappings WHERE session_id=$1 AND hash_key='<sv_c>')")
            .bind(&my_session).fetch_one(&pool).await.unwrap();
        assert!(!a_exists);
        assert!(b_exists, "row in other session must survive");
        assert!(c_exists, "non-expired row in same session must survive");

        sqlx::query("DELETE FROM secure_value_mappings WHERE session_id IN ($1, $2)")
            .bind(&my_session).bind(&other_session).execute(&pool).await.ok();
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL"]
    async fn cleanup_expired_for_run_respects_agent_session_id() {
        dotenvy::dotenv().ok();
        let url = std::env::var("DATABASE_URL").unwrap();
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let repo = PostgresSecureValueRepository::new(pool.clone());

        let my_agent = format!("agent_sweep_{}", uuid::Uuid::new_v4());
        let other_agent = format!("agent_other_{}", uuid::Uuid::new_v4());
        let session_a = format!("s_a_{}", uuid::Uuid::new_v4());
        let session_b = format!("s_b_{}", uuid::Uuid::new_v4());

        repo.persist(&session_a, Some(&my_agent), "n", "<sv_mine>", "alice123", "secret").await.unwrap();
        repo.persist(&session_b, Some(&other_agent), "n", "<sv_other>", "alice123", "secret").await.unwrap();
        sqlx::query("UPDATE secure_value_mappings SET expires_at = NOW() - INTERVAL '1 second' WHERE hash_key IN ('<sv_mine>','<sv_other>')")
            .execute(&pool).await.unwrap();

        // Pass an unrelated session_id to prove agent_session_id is the key.
        let unrelated_session = format!("unrelated_{}", uuid::Uuid::new_v4());
        let deleted = repo.cleanup_expired_for_run(&unrelated_session, Some(&my_agent)).await.unwrap();
        assert_eq!(deleted, 1);

        let mine_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM secure_value_mappings WHERE hash_key='<sv_mine>')")
            .fetch_one(&pool).await.unwrap();
        let other_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM secure_value_mappings WHERE hash_key='<sv_other>')")
            .fetch_one(&pool).await.unwrap();
        assert!(!mine_exists);
        assert!(other_exists);

        sqlx::query("DELETE FROM secure_value_mappings WHERE agent_session_id IN ($1, $2)")
            .bind(&my_agent).bind(&other_agent).execute(&pool).await.ok();
    }
}
