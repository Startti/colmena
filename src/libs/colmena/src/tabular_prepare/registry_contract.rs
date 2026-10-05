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

pub(crate) async fn concurrent_claims_have_exactly_one_winner<R: PreparationRegistry + 'static>(
    r: Arc<R>,
) {
    let key = fresh_key();
    let mut tasks = Vec::new();
    for i in 0..8 {
        let r = r.clone();
        let key = key.clone();
        tasks.push(tokio::spawn(async move {
            r.claim(claim_req(&key, &format!("racer-{i}"), t0()))
                .await
                .unwrap()
        }));
    }
    let mut winners = 0;
    for t in tasks {
        if t.await.unwrap().is_some() {
            winners += 1;
        }
    }
    assert_eq!(winners, 1);
    assert_eq!(get_row(&*r, &key).await.attempts, 1);
}

pub(crate) async fn a_dead_job_is_retried_only_up_to_the_attempt_cap<R: PreparationRegistry>(
    r: &R,
) {
    let key = fresh_key();
    let mut now = t0();
    // Jobs that die without writing `failed`: each takeover happens after the
    // lease expired and counts as an attempt.
    for attempt in 1..=MAX_ATTEMPTS {
        let owner = format!("dead-{attempt}");
        let claim = r.claim(claim_req(&key, &owner, now)).await.unwrap();
        assert_eq!(
            claim,
            Some(Claim { attempts: attempt }),
            "attempt {attempt}"
        );
        now += Duration::seconds(361);
    }
    // The lease of the third job is expired too, but the cap holds.
    assert_eq!(r.claim(claim_req(&key, "late", now)).await.unwrap(), None);
    let row = get_row(r, &key).await;
    assert_eq!(row.attempts, MAX_ATTEMPTS);
    assert_eq!(row.lease_owner.as_deref(), Some("dead-3"));
    // A newer format still gets a fresh budget.
    let mut req = claim_req(&key, "v2", now);
    req.format_version = FORMAT_VERSION + 1;
    assert_eq!(r.claim(req).await.unwrap(), Some(Claim { attempts: 1 }));
}

/// The claim compares timestamps as text on SQLite: whole-second and
/// fractional values must still order correctly around the lease boundary.
pub(crate) async fn the_lease_boundary_holds_with_sub_second_timestamps<R: PreparationRegistry>(
    r: &R,
) {
    let ms = Duration::milliseconds;
    // Lease granted at a fractional instant, probed with whole-second and
    // fractional clocks.
    let key = fresh_key();
    let granted = t0() + ms(123);
    r.claim(claim_req(&key, "A", granted))
        .await
        .unwrap()
        .unwrap();
    let expiry = granted + lease();
    for probe in [expiry - ms(1), expiry - ms(123), expiry] {
        assert_eq!(
            r.claim(claim_req(&key, "B", probe)).await.unwrap(),
            None,
            "{probe}"
        );
    }
    assert!(r
        .claim(claim_req(&key, "B", expiry + ms(1)))
        .await
        .unwrap()
        .is_some());

    // Lease granted on a whole second, probed a millisecond either side.
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    let expiry = t0() + lease();
    assert_eq!(
        r.claim(claim_req(&key, "B", expiry - ms(1))).await.unwrap(),
        None
    );
    assert_eq!(r.claim(claim_req(&key, "B", expiry)).await.unwrap(), None);
    assert!(r
        .claim(claim_req(&key, "B", expiry + ms(1)))
        .await
        .unwrap()
        .is_some());
}

fn ready_info() -> ReadyInfo {
    ReadyInfo {
        manifest_key: "chat-attachments/u/s/prepared/f1/manifest.json".to_string(),
        blob_keys: vec![
            "chat-attachments/u/s/prepared/f1/t0/part-00000.parquet".to_string(),
            "chat-attachments/u/s/prepared/f1/t0/part-00001.parquet".to_string(),
        ],
        tables_json: r#"[{"name":"data","rows":3,"columns":[{"name":"a","type":"int64"}]}]"#
            .to_string(),
        prepared_bytes: 12_345,
    }
}

pub(crate) async fn complete_marks_ready_for_the_lease_owner<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    let done = t0() + Duration::seconds(90);
    let out = r.complete(&key, "A", ready_info(), done).await.unwrap();
    assert_eq!(out, TerminalOutcome::Written);
    let row = get_row(r, &key).await;
    assert_eq!(row.status, PrepareStatus::Ready);
    assert_eq!(row.manifest_key, Some(ready_info().manifest_key));
    assert_eq!(row.blob_keys, ready_info().blob_keys);
    assert_eq!(row.tables_json, Some(ready_info().tables_json));
    assert_eq!(row.prepared_bytes, Some(12_345));
    assert_eq!(row.lease_owner, None);
    assert_eq!(row.lease_until, None);
    assert_eq!(row.updated_at, done);
}

pub(crate) async fn complete_by_a_non_owner_is_cancelled_and_never_ready<R: PreparationRegistry>(
    r: &R,
) {
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    // B was never the owner.
    let out = r.complete(&key, "B", ready_info(), t0()).await.unwrap();
    assert_eq!(out, TerminalOutcome::Cancelled);
    assert_eq!(get_row(r, &key).await.status, PrepareStatus::Running);
    // A's lease expires and B takes over; A's late completion is cancelled.
    let later = t0() + Duration::seconds(400);
    r.claim(claim_req(&key, "B", later)).await.unwrap().unwrap();
    let late = r.complete(&key, "A", ready_info(), later).await.unwrap();
    assert_eq!(late, TerminalOutcome::Cancelled);
    let row = get_row(r, &key).await;
    assert_eq!(row.status, PrepareStatus::Running);
    assert_eq!(row.lease_owner.as_deref(), Some("B"));
    assert_eq!(
        r.complete(&key, "B", ready_info(), later).await.unwrap(),
        TerminalOutcome::Written
    );
}

pub(crate) async fn fail_records_the_reason_and_keeps_the_attempt<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    let at = t0() + Duration::seconds(5);
    let out = r
        .fail(&key, "A", "time", "ran past 300 s", at)
        .await
        .unwrap();
    assert_eq!(out, TerminalOutcome::Written);
    let row = get_row(r, &key).await;
    assert_eq!(row.status, PrepareStatus::Failed);
    assert_eq!(row.error_code.as_deref(), Some("time"));
    assert_eq!(row.error_detail.as_deref(), Some("ran past 300 s"));
    assert_eq!(row.attempts, 1);
    assert_eq!(row.lease_owner, None);
    assert_eq!(row.updated_at, at);
}

pub(crate) async fn fail_never_writes_for_a_missing_row_or_another_owner<R: PreparationRegistry>(
    r: &R,
) {
    let missing = fresh_key();
    let out = r.fail(&missing, "A", "time", "x", t0()).await.unwrap();
    assert_eq!(out, TerminalOutcome::Cancelled);
    assert_eq!(
        r.get(&missing).await.unwrap(),
        None,
        "no failed row appears"
    );

    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    let out = r.fail(&key, "B", "time", "x", t0()).await.unwrap();
    assert_eq!(out, TerminalOutcome::Cancelled);
    assert_eq!(get_row(r, &key).await.status, PrepareStatus::Running);
}

pub(crate) async fn claim_retries_a_failed_row_until_the_third_attempt<R: PreparationRegistry>(
    r: &R,
) {
    let key = fresh_key();
    let mut now = t0();
    for attempt in 1..=MAX_ATTEMPTS {
        let owner = format!("owner-{attempt}");
        let claim = r.claim(claim_req(&key, &owner, now)).await.unwrap();
        assert_eq!(
            claim,
            Some(Claim { attempts: attempt }),
            "attempt {attempt}"
        );
        now += Duration::seconds(10);
        let out = r.fail(&key, &owner, "time", "too slow", now).await.unwrap();
        assert_eq!(out, TerminalOutcome::Written);
    }
    // The third failure is final.
    let again = r.claim(claim_req(&key, "late", now)).await.unwrap();
    assert_eq!(again, None);
    let row = get_row(r, &key).await;
    assert_eq!(row.status, PrepareStatus::Failed);
    assert_eq!(row.attempts, MAX_ATTEMPTS);
}

pub(crate) async fn claim_takes_a_row_written_by_an_older_format<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    // A source that exhausted its attempts under format 1 ...
    let mut now = t0();
    for attempt in 1..=MAX_ATTEMPTS {
        let owner = format!("v1-{attempt}");
        r.claim(claim_req(&key, &owner, now))
            .await
            .unwrap()
            .unwrap();
        now += Duration::seconds(1);
        r.fail(&key, &owner, "parse", "bad", now).await.unwrap();
    }
    assert_eq!(
        r.claim(claim_req(&key, "v1-late", now)).await.unwrap(),
        None
    );
    // ... is claimable again under format 2, with a fresh attempt budget.
    let mut req = claim_req(&key, "v2", now);
    req.format_version = FORMAT_VERSION + 1;
    assert_eq!(r.claim(req).await.unwrap(), Some(Claim { attempts: 1 }));
    let row = get_row(r, &key).await;
    assert_eq!(row.status, PrepareStatus::Running);
    assert_eq!(row.format_version, FORMAT_VERSION + 1);
    assert_eq!(row.error_code, None);
}

pub(crate) async fn a_ready_row_is_claimable_only_by_a_newer_format<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    r.complete(&key, "A", ready_info(), t0()).await.unwrap();
    let much_later = t0() + Duration::days(30);
    assert_eq!(
        r.claim(claim_req(&key, "B", much_later)).await.unwrap(),
        None
    );
    let mut req = claim_req(&key, "B", much_later);
    req.format_version = FORMAT_VERSION + 1;
    assert_eq!(r.claim(req).await.unwrap(), Some(Claim { attempts: 1 }));
    let row = get_row(r, &key).await;
    assert_eq!(row.status, PrepareStatus::Running);
    assert_eq!(
        row.manifest_key, None,
        "the old manifest is not carried over"
    );
    assert_eq!(
        row.blob_keys,
        ready_info().blob_keys,
        "old blobs stay listed for cleanup"
    );
}

pub(crate) async fn complete_and_fail_keep_every_blob_ever_recorded<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    let s = |v: &[&str]| v.iter().map(|k| k.to_string()).collect::<Vec<_>>();
    // A first attempt fails with two blobs still on storage ...
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    r.fail_with_blobs(&key, "A", "time", "x", &s(&["a", "b"]), t0())
        .await
        .unwrap();
    assert_eq!(get_row(r, &key).await.blob_keys, s(&["a", "b"]));
    // ... a second one fails again, naming one of them and a new one ...
    r.claim(claim_req(&key, "B", t0())).await.unwrap().unwrap();
    r.fail_with_blobs(&key, "B", "time", "x", &s(&["b", "c"]), t0())
        .await
        .unwrap();
    assert_eq!(
        get_row(r, &key).await.blob_keys,
        s(&["a", "b", "c"]),
        "union"
    );
    // ... and the third completes: its own keys are added, nothing dropped.
    r.claim(claim_req(&key, "C", t0())).await.unwrap().unwrap();
    let mut info = ready_info();
    info.blob_keys = s(&["c", "d"]);
    r.complete(&key, "C", info, t0()).await.unwrap();
    assert_eq!(
        get_row(r, &key).await.blob_keys,
        s(&["a", "b", "c", "d"]),
        "complete keeps what earlier attempts left"
    );
}

pub(crate) async fn a_non_owner_terminal_write_records_no_blobs<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    let out = r
        .fail_with_blobs(&key, "B", "time", "x", &["z".to_string()], t0())
        .await
        .unwrap();
    assert_eq!(out, TerminalOutcome::Cancelled);
    assert!(get_row(r, &key).await.blob_keys.is_empty());
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn tabular_prepare_concurrent_claims_have_exactly_one_winner() {
        let f = fixture().await;
        concurrent_claims_have_exactly_one_winner(f.registry.clone()).await;
    }

    #[tokio::test]
    async fn tabular_prepare_a_dead_job_is_retried_only_up_to_the_attempt_cap() {
        let f = fixture().await;
        a_dead_job_is_retried_only_up_to_the_attempt_cap(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_the_lease_boundary_holds_with_sub_second_timestamps() {
        let f = fixture().await;
        the_lease_boundary_holds_with_sub_second_timestamps(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_complete_marks_ready_for_the_lease_owner() {
        let f = fixture().await;
        complete_marks_ready_for_the_lease_owner(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_complete_by_a_non_owner_is_cancelled_and_never_ready() {
        let f = fixture().await;
        complete_by_a_non_owner_is_cancelled_and_never_ready(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_fail_records_the_reason_and_keeps_the_attempt() {
        let f = fixture().await;
        fail_records_the_reason_and_keeps_the_attempt(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_fail_never_writes_for_a_missing_row_or_another_owner() {
        let f = fixture().await;
        fail_never_writes_for_a_missing_row_or_another_owner(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_claim_retries_a_failed_row_until_the_third_attempt() {
        let f = fixture().await;
        claim_retries_a_failed_row_until_the_third_attempt(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_claim_takes_a_row_written_by_an_older_format() {
        let f = fixture().await;
        claim_takes_a_row_written_by_an_older_format(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_a_ready_row_is_claimable_only_by_a_newer_format() {
        let f = fixture().await;
        a_ready_row_is_claimable_only_by_a_newer_format(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_complete_and_fail_keep_every_blob_ever_recorded() {
        let f = fixture().await;
        complete_and_fail_keep_every_blob_ever_recorded(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_a_non_owner_terminal_write_records_no_blobs() {
        let f = fixture().await;
        a_non_owner_terminal_write_records_no_blobs(&*f.registry).await;
    }
}
