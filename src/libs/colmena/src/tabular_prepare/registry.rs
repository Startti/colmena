//! Preparation registry: which large tabular sources are being prepared,
//! are ready, or failed. See the module docs in [`super`].
//!
//! The registry sits behind the [`PreparationRegistry`] trait so the host can
//! choose where rows live and who may delete one (open question O8): ADP may
//! delete a row directly or through an internal Colmena endpoint, and either
//! fits without touching the callers.
//!
//! Writes happen only on state change: one claim and one terminal write per
//! attempt. Progress never touches the registry and the lease is a single
//! fixed value with no renewal, so a long preparation costs two writes.

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use thiserror::Error;

/// Version of the prepared layout. A row written with an older value is
/// claimable again so the file is prepared in the new layout.
pub const FORMAT_VERSION: i32 = 1;

/// A source that failed this many times is final: no further claim.
pub const MAX_ATTEMPTS: i32 = 3;

/// Extra time on top of the preparation time budget before a lease expires.
pub const LEASE_GRACE: Duration = Duration::seconds(60);

/// Fixed lease for a claim: the job's own time budget plus [`LEASE_GRACE`].
pub fn lease_for(prep_time: Duration) -> Duration {
    prep_time + LEASE_GRACE
}

/// State of a registry row. A row is created at claim, so it starts at
/// `Running` (the narrow reading of `pending`, open question O9: a pending
/// item lives in the queue, not here). Stored as TEXT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareStatus {
    Running,
    Ready,
    Failed,
    /// Being removed by the cleanup pass, which holds a lease on it. A
    /// preparation never claims such a row.
    Deleting,
}

impl PrepareStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PrepareStatus::Running => "running",
            PrepareStatus::Ready => "ready",
            PrepareStatus::Failed => "failed",
            PrepareStatus::Deleting => "deleting",
        }
    }

    pub fn parse(value: &str) -> Result<Self, RegistryError> {
        match value {
            "running" => Ok(PrepareStatus::Running),
            "ready" => Ok(PrepareStatus::Ready),
            "failed" => Ok(PrepareStatus::Failed),
            "deleting" => Ok(PrepareStatus::Deleting),
            other => Err(RegistryError::Backend(format!(
                "unknown preparation status '{other}'"
            ))),
        }
    }
}

/// One `attachment_prepared` row.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedRow {
    pub source_storage_key: String,
    pub status: PrepareStatus,
    pub format_version: i32,
    pub manifest_key: Option<String>,
    /// Every blob written for this source, for cleanup.
    pub blob_keys: Vec<String>,
    pub tables_json: Option<String>,
    pub source_bytes: i64,
    pub prepared_bytes: Option<i64>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_until: Option<DateTime<Utc>>,
    pub attempts: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// Input of [`PreparationRegistry::claim`]. `now` is passed in (never read
/// from the database clock) so both dialects compare against the same value
/// and tests control time.
#[derive(Debug, Clone)]
pub struct ClaimRequest {
    pub source_key: String,
    pub source_bytes: i64,
    pub format_version: i32,
    pub owner: String,
    pub lease: Duration,
    pub now: DateTime<Utc>,
}

/// A won claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// Attempt number this claim represents (1 for a fresh claim).
    pub attempts: i32,
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("preparation registry backend error: {0}")]
    Backend(String),
}

#[async_trait]
pub trait PreparationRegistry: Send + Sync {
    /// Try to take the preparation of a source. One statement, atomic on both
    /// dialects. Claimable when there is no row, the row is `failed` with
    /// fewer than [`MAX_ATTEMPTS`] attempts, it is `running` with an expired
    /// lease, or its `format_version` is older. `None` means someone else
    /// holds it, it is final, or it is already ready at this version.
    async fn claim(&self, req: ClaimRequest) -> Result<Option<Claim>, RegistryError>;

    async fn get(&self, source_key: &str) -> Result<Option<PreparedRow>, RegistryError>;
}

// ---------------------------------------------------------------------------
// SQL shared by the SQLite and Postgres implementations.
//
// Written once with `$N` placeholders; SQLite runs it through
// [`for_sqlite`], which turns `$N` into `?N`. Every statement is valid on
// both dialects (upsert with `WHERE`, `RETURNING`).
// ---------------------------------------------------------------------------

pub(crate) fn for_sqlite(sql: &str) -> String {
    sql.replace('$', "?")
}

/// The claim rule as one atomic statement: `$1` key, `$2` format version,
/// `$3` source bytes, `$4` owner, `$5` lease end, `$6` now, `$7` max
/// attempts (a takeover of an expired lease counts as an attempt, so a job that
/// dies without writing `failed` is retried at most `$7` times). A claim over a row written by an older format restarts the
/// attempt count; any other claim adds one. `RETURNING` yields a row only
/// when the claim was won.
pub(crate) const CLAIM_SQL: &str = "\
INSERT INTO attachment_prepared
    (source_storage_key, status, format_version, blob_keys, source_bytes,
     lease_owner, lease_until, attempts, created_at, updated_at)
VALUES ($1, 'running', $2, '[]', $3, $4, $5, 1, $6, $6)
ON CONFLICT (source_storage_key) DO UPDATE SET
    status = 'running',
    attempts = CASE
        WHEN attachment_prepared.format_version < excluded.format_version THEN 1
        ELSE attachment_prepared.attempts + 1
    END,
    format_version = excluded.format_version,
    source_bytes = excluded.source_bytes,
    manifest_key = NULL,
    tables_json = NULL,
    prepared_bytes = NULL,
    error_code = NULL,
    error_detail = NULL,
    lease_owner = excluded.lease_owner,
    lease_until = excluded.lease_until,
    updated_at = excluded.updated_at
WHERE attachment_prepared.format_version < excluded.format_version
   OR (attachment_prepared.status = 'failed' AND attachment_prepared.attempts < $7)
   OR (attachment_prepared.status = 'running' AND attachment_prepared.attempts < $7
        AND attachment_prepared.lease_until < $6)
RETURNING attempts";

pub(crate) const GET_SQL: &str = "\
SELECT source_storage_key, status, format_version, manifest_key, blob_keys,
       tables_json, source_bytes, prepared_bytes, error_code, error_detail,
       lease_owner, lease_until, attempts, created_at, updated_at, last_used_at
  FROM attachment_prepared
 WHERE source_storage_key = $1";

pub(crate) fn backend_err(context: &str, e: impl std::fmt::Display) -> RegistryError {
    RegistryError::Backend(format!("{context}: {e}"))
}

pub(crate) fn blob_keys_from_json(raw: &str) -> Result<Vec<String>, RegistryError> {
    serde_json::from_str(raw).map_err(|e| backend_err("blob_keys", e))
}

/// Build a [`PreparedRow`] from a SQLx row of either dialect (both select the
/// columns of [`GET_SQL`]).
macro_rules! prepared_row_from {
    ($row:expr) => {{
        use sqlx::Row as _;
        let row = $row;
        let col = |name: &str| format!("column {name}");
        let status: String = row
            .try_get("status")
            .map_err(|e| $crate::tabular_prepare::registry::backend_err(&col("status"), e))?;
        let blob_keys: String = row
            .try_get("blob_keys")
            .map_err(|e| $crate::tabular_prepare::registry::backend_err(&col("blob_keys"), e))?;
        macro_rules! field {
            ($name:literal) => {
                row.try_get($name)
                    .map_err(|e| $crate::tabular_prepare::registry::backend_err(&col($name), e))?
            };
        }
        $crate::tabular_prepare::registry::PreparedRow {
            source_storage_key: field!("source_storage_key"),
            status: $crate::tabular_prepare::registry::PrepareStatus::parse(&status)?,
            format_version: field!("format_version"),
            manifest_key: field!("manifest_key"),
            blob_keys: $crate::tabular_prepare::registry::blob_keys_from_json(&blob_keys)?,
            tables_json: field!("tables_json"),
            source_bytes: field!("source_bytes"),
            prepared_bytes: field!("prepared_bytes"),
            error_code: field!("error_code"),
            error_detail: field!("error_detail"),
            lease_owner: field!("lease_owner"),
            lease_until: field!("lease_until"),
            attempts: field!("attempts"),
            created_at: field!("created_at"),
            updated_at: field!("updated_at"),
            last_used_at: field!("last_used_at"),
        }
    }};
}
pub(crate) use prepared_row_from;

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
