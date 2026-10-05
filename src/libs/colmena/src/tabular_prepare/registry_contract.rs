//! Backend-independent behaviour of [`PreparationRegistry`], written once and
//! run against SQLite here and against Postgres in the ignored tests of
//! `postgres_registry` (`DATABASE_URL` required).

use super::registry::*;
use chrono::{DateTime, Duration, TimeZone, Utc};
use std::sync::Arc;
use uuid::Uuid;

pub(crate) fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()
}

pub(crate) fn fresh_key() -> String {
    format!("chat-attachments/u/s/{}.csv", Uuid::new_v4())
}

/// Lease used by the cases: a 300 s time budget plus the 60 s grace.
fn lease() -> Duration {
    lease_for(Duration::seconds(300))
}

fn claim_req(key: &str, owner: &str, now: DateTime<Utc>) -> ClaimRequest {
    ClaimRequest {
        source_key: key.to_string(),
        source_bytes: 60_000_000,
        format_version: FORMAT_VERSION,
        owner: owner.to_string(),
        lease: lease(),
        now,
    }
}

async fn get_row<R: PreparationRegistry>(r: &R, key: &str) -> PreparedRow {
    r.get(key).await.unwrap().expect("row present")
}

pub(crate) async fn claim_creates_a_running_row_when_there_is_none<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    let claim = r.claim(claim_req(&key, "A", t0())).await.unwrap();
    assert_eq!(claim, Some(Claim { attempts: 1 }));
    let row = get_row(r, &key).await;
    assert_eq!(row.status, PrepareStatus::Running);
    assert_eq!(row.lease_owner.as_deref(), Some("A"));
    assert_eq!(row.lease_until, Some(t0() + Duration::seconds(360)));
    assert_eq!(row.attempts, 1);
    assert_eq!(row.format_version, FORMAT_VERSION);
    assert_eq!(row.source_bytes, 60_000_000);
    assert!(row.blob_keys.is_empty());
    assert_eq!(row.created_at, t0());
    assert_eq!(row.updated_at, t0());
}

pub(crate) async fn claim_is_refused_while_a_lease_is_live<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    // One second before expiry, and exactly at expiry: still A's.
    for offset in [359, 360] {
        let now = t0() + Duration::seconds(offset);
        assert_eq!(r.claim(claim_req(&key, "B", now)).await.unwrap(), None);
    }
    let row = get_row(r, &key).await;
    assert_eq!(row.lease_owner.as_deref(), Some("A"));
    assert_eq!(row.attempts, 1);
}

pub(crate) async fn claim_takes_over_an_expired_lease<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    let later = t0() + Duration::seconds(361);
    let claim = r.claim(claim_req(&key, "B", later)).await.unwrap();
    assert_eq!(claim, Some(Claim { attempts: 2 }));
    let row = get_row(r, &key).await;
    assert_eq!(row.lease_owner.as_deref(), Some("B"));
    assert_eq!(row.lease_until, Some(later + Duration::seconds(360)));
    assert_eq!(row.created_at, t0(), "the original creation time is kept");
}

#[cfg(test)]
mod sqlite {
    use super::*;
    use crate::tabular_prepare::sqlite_registry::SqlitePreparationRegistry;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::time::Duration as StdDuration;

    /// A file-backed database so several pooled connections share it (an
    /// in-memory database is private to one connection); the temp dir lives
    /// as long as the fixture.
    struct Fixture {
        registry: Arc<SqlitePreparationRegistry>,
        _dir: tempfile::TempDir,
    }

    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(dir.path().join("registry.db"))
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(StdDuration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate!("migrations/sqlite")
            .run(&pool)
            .await
            .unwrap();
        Fixture {
            registry: Arc::new(SqlitePreparationRegistry::from_pool(Arc::new(pool))),
            _dir: dir,
        }
    }

    #[tokio::test]
    async fn tabular_prepare_claim_creates_a_running_row_when_there_is_none() {
        let f = fixture().await;
        claim_creates_a_running_row_when_there_is_none(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_claim_is_refused_while_a_lease_is_live() {
        let f = fixture().await;
        claim_is_refused_while_a_lease_is_live(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_claim_takes_over_an_expired_lease() {
        let f = fixture().await;
        claim_takes_over_an_expired_lease(&*f.registry).await;
    }
}
