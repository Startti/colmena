//! Preparation registry: which large tabular sources are being prepared, are
//! ready, or failed. This slice holds only the schema checks; the repository
//! trait and its implementations arrive in the following slices.

#[cfg(test)]
mod migration_tests {
    use sqlx::{Row, SqlitePool};

    async fn migrated_memory_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("migrations/sqlite")
            .run(&pool)
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn attachment_prepared_sqlite_migration_creates_the_registry_table() {
        let pool = migrated_memory_pool().await;
        let rows = sqlx::query("PRAGMA table_info(attachment_prepared)")
            .fetch_all(&pool)
            .await
            .unwrap();
        let cols: Vec<(String, String, bool, bool)> = rows
            .iter()
            .map(|r| {
                (
                    r.get::<String, _>("name"),
                    r.get::<String, _>("type"),
                    r.get::<i64, _>("notnull") == 1,
                    r.get::<i64, _>("pk") == 1,
                )
            })
            .collect();
        let names: Vec<&str> = cols.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "source_storage_key",
                "status",
                "format_version",
                "manifest_key",
                "blob_keys",
                "tables_json",
                "source_bytes",
                "prepared_bytes",
                "error_code",
                "error_detail",
                "lease_owner",
                "lease_until",
                "attempts",
                "created_at",
                "updated_at",
                "last_used_at",
            ]
        );
        let key = &cols[0];
        assert!(key.3, "source_storage_key must be the primary key");
        let by_name = |n: &str| cols.iter().find(|c| c.0 == n).unwrap().clone();
        assert!(by_name("status").2 && by_name("format_version").2);
        assert!(by_name("blob_keys").2 && by_name("source_bytes").2 && by_name("attempts").2);
        assert!(!by_name("manifest_key").2 && !by_name("lease_owner").2);
    }

    #[tokio::test]
    async fn attachment_prepared_defaults_apply_on_a_minimal_insert() {
        let pool = migrated_memory_pool().await;
        sqlx::query(
            "INSERT INTO attachment_prepared
               (source_storage_key, status, format_version, source_bytes, created_at, updated_at)
             VALUES ('k', 'running', 1, 10, '2026-10-04T00:00:00+00:00', '2026-10-04T00:00:00+00:00')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let row = sqlx::query("SELECT blob_keys, attempts FROM attachment_prepared")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("blob_keys"), "[]");
        assert_eq!(row.get::<i64, _>("attempts"), 0);
    }

    /// Same table on Postgres: applies every Postgres migration and reads the
    /// column list back from `information_schema`. Ignored like the other
    /// Postgres repository tests (needs `DATABASE_URL`).
    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    #[tokio::test]
    async fn attachment_prepared_postgres_migration_creates_the_registry_table() {
        use crate::dag_engine::infrastructure::pool_registry::{PgPoolRegistry, PoolConfig};
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        let registry = PgPoolRegistry::new(PoolConfig::defaults());
        let pool = registry.get_or_create(&url).await.unwrap();
        sqlx::migrate!("migrations/postgres")
            .set_ignore_missing(true)
            .run(&*pool)
            .await
            .unwrap();
        let rows = sqlx::query(
            "SELECT column_name, data_type, is_nullable FROM information_schema.columns
              WHERE table_name = 'attachment_prepared' ORDER BY ordinal_position",
        )
        .fetch_all(&*pool)
        .await
        .unwrap();
        let cols: Vec<(String, String, String)> = rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect();
        let find = |n: &str| cols.iter().find(|c| c.0 == n).unwrap().clone();
        assert_eq!(cols.len(), 16);
        assert_eq!(cols[0].0, "source_storage_key");
        assert_eq!(find("lease_until").1, "timestamp with time zone");
        assert_eq!(find("created_at").2, "NO");
        assert_eq!(find("manifest_key").2, "YES");
        assert_eq!(find("source_bytes").1, "bigint");
    }
}
