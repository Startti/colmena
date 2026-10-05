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

/// Counts what a job does to the registry. Only `claim` and the terminal
/// writes may happen during a preparation (TP-5).
#[derive(Default)]
pub(crate) struct Counts {
    pub claims: std::sync::atomic::AtomicUsize,
    pub terminals: std::sync::atomic::AtomicUsize,
    pub other_writes: std::sync::atomic::AtomicUsize,
    pub reads: std::sync::atomic::AtomicUsize,
}

pub(crate) struct CountingRegistry<R> {
    pub inner: R,
    pub counts: Counts,
}

impl<R> CountingRegistry<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            counts: Counts::default(),
        }
    }
    pub fn snapshot(&self) -> (usize, usize, usize, usize) {
        use std::sync::atomic::Ordering::SeqCst;
        (
            self.counts.claims.load(SeqCst),
            self.counts.terminals.load(SeqCst),
            self.counts.other_writes.load(SeqCst),
            self.counts.reads.load(SeqCst),
        )
    }
}

#[async_trait::async_trait]
impl<R: PreparationRegistry> PreparationRegistry for CountingRegistry<R> {
    async fn claim(&self, req: ClaimRequest) -> Result<Option<Claim>, RegistryError> {
        self.counts
            .claims
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.claim(req).await
    }
    async fn complete(
        &self,
        k: &str,
        o: &str,
        i: ReadyInfo,
        n: DateTime<Utc>,
    ) -> Result<TerminalOutcome, RegistryError> {
        self.counts
            .terminals
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.complete(k, o, i, n).await
    }
    async fn fail_with_blobs(
        &self,
        k: &str,
        o: &str,
        c: &str,
        d: &str,
        b: &[String],
        n: DateTime<Utc>,
    ) -> Result<TerminalOutcome, RegistryError> {
        self.counts
            .terminals
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.fail_with_blobs(k, o, c, d, b, n).await
    }
    async fn still_owned(&self, k: &str, o: &str) -> Result<bool, RegistryError> {
        self.counts
            .reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.still_owned(k, o).await
    }
    async fn release(&self, k: &str, o: &str) -> Result<bool, RegistryError> {
        self.counts
            .other_writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.release(k, o).await
    }
    async fn delete(&self, k: &str) -> Result<bool, RegistryError> {
        self.counts
            .other_writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.delete(k).await
    }
    async fn get(&self, k: &str) -> Result<Option<PreparedRow>, RegistryError> {
        self.counts
            .reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.get(k).await
    }
    async fn touch_last_used(
        &self,
        k: &str,
        n: DateTime<Utc>,
        i: Duration,
    ) -> Result<bool, RegistryError> {
        self.counts
            .other_writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.touch_last_used(k, n, i).await
    }
    async fn find_stale(
        &self,
        cutoff: DateTime<Utc>,
        now: DateTime<Utc>,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<PreparedRow>, RegistryError> {
        self.counts
            .reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.find_stale(cutoff, now, after, limit).await
    }

    async fn list_ready_after(
        &self,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<PreparedRow>, RegistryError> {
        self.counts
            .reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.list_ready_after(after, limit).await
    }
}

pub(crate) async fn a_preparation_writes_only_on_claim_and_terminal<R: PreparationRegistry>(r: R) {
    let counting = CountingRegistry::new(r);
    // A successful job, then a failing one on another source.
    let ok = fresh_key();
    counting
        .claim(claim_req(&ok, "A", t0()))
        .await
        .unwrap()
        .unwrap();
    counting
        .complete(&ok, "A", ready_info(), t0())
        .await
        .unwrap();
    assert_eq!(counting.snapshot(), (1, 1, 0, 0));
    let bad = fresh_key();
    counting
        .claim(claim_req(&bad, "A", t0()))
        .await
        .unwrap()
        .unwrap();
    counting.fail(&bad, "A", "time", "x", t0()).await.unwrap();
    assert_eq!(counting.snapshot(), (2, 2, 0, 0));
}

pub(crate) async fn complete_after_the_row_is_deleted_is_cancelled<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    // The source was deleted: the API removed the row (done here by SQL-free
    // means is not possible, so the trait's `delete` stands in for it).
    assert!(r.delete(&key).await.unwrap());
    let out = r.complete(&key, "A", ready_info(), t0()).await.unwrap();
    assert_eq!(out, TerminalOutcome::Cancelled);
    assert_eq!(r.get(&key).await.unwrap(), None, "no row, never ready");
}

pub(crate) async fn still_owned_tells_the_owner_from_everyone_else<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    assert!(!r.still_owned(&key, "A").await.unwrap(), "no row");
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    assert!(r.still_owned(&key, "A").await.unwrap());
    assert!(!r.still_owned(&key, "B").await.unwrap(), "another owner");
    // Takeover after expiry moves ownership.
    r.claim(claim_req(&key, "B", t0() + Duration::seconds(400)))
        .await
        .unwrap()
        .unwrap();
    assert!(!r.still_owned(&key, "A").await.unwrap());
    assert!(r.still_owned(&key, "B").await.unwrap());
    // Deleting the row is the cancellation signal.
    assert!(r.delete(&key).await.unwrap());
    assert!(!r.still_owned(&key, "B").await.unwrap());
    assert!(
        !r.delete(&key).await.unwrap(),
        "second delete finds nothing"
    );
}

pub(crate) async fn the_cancellation_check_is_a_read_and_the_lease_is_not_renewed<
    R: PreparationRegistry,
>(
    r: R,
) {
    let counting = CountingRegistry::new(r);
    let key = fresh_key();
    counting
        .claim(claim_req(&key, "A", t0()))
        .await
        .unwrap()
        .unwrap();
    let before = get_row(&counting, &key).await;
    for _ in 0..20 {
        assert!(counting.still_owned(&key, "A").await.unwrap());
    }
    let after = get_row(&counting, &key).await;
    assert_eq!(
        after, before,
        "checking ownership changes nothing, lease included"
    );
    assert_eq!(counting.snapshot(), (1, 0, 0, 22));
}

pub(crate) async fn a_duplicate_trigger_after_delete_leaves_no_row<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    // The source file is deleted while A runs: the row goes first.
    assert!(r.delete(&key).await.unwrap());
    assert!(
        !r.still_owned(&key, "A").await.unwrap(),
        "A sees it and stops"
    );
    // A duplicate trigger arrives afterwards and re-claims a fresh row ...
    let later = t0() + Duration::seconds(30);
    let claim = r.claim(claim_req(&key, "B", later)).await.unwrap();
    assert_eq!(claim, Some(Claim { attempts: 1 }));
    // ... finds the source missing and removes its own row: nothing is left,
    // in particular no `failed` row.
    assert!(
        !r.release(&key, "A").await.unwrap(),
        "a stale owner removes nothing"
    );
    assert_eq!(get_row(r, &key).await.lease_owner.as_deref(), Some("B"));
    assert!(r.release(&key, "B").await.unwrap());
    assert_eq!(r.get(&key).await.unwrap(), None);
}

pub(crate) async fn touch_last_used_marks_use_at_most_once_per_interval<R: PreparationRegistry>(
    r: &R,
) {
    let key = fresh_key();
    let interval = Duration::hours(1);
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    // A row that is not ready is never touched.
    assert!(!r.touch_last_used(&key, t0(), interval).await.unwrap());
    r.complete(&key, "A", ready_info(), t0()).await.unwrap();

    assert!(r.touch_last_used(&key, t0(), interval).await.unwrap());
    assert_eq!(get_row(r, &key).await.last_used_at, Some(t0()));
    let soon = t0() + Duration::minutes(30);
    assert!(
        !r.touch_last_used(&key, soon, interval).await.unwrap(),
        "throttled"
    );
    assert_eq!(get_row(r, &key).await.last_used_at, Some(t0()));
    let later = t0() + Duration::minutes(61);
    assert!(r.touch_last_used(&key, later, interval).await.unwrap());
    assert_eq!(get_row(r, &key).await.last_used_at, Some(later));
    assert!(
        !r.touch_last_used(&fresh_key(), later, interval)
            .await
            .unwrap(),
        "no row"
    );
}

/// A job run for real through `ensure_prepared`: the registry sees one claim,
/// one terminal write and, when the table is handed out, one throttled
/// `last_used_at` touch; the cancellation checks between parts are reads.
pub(crate) struct PartsJob<R> {
    pub registry: Arc<CountingRegistry<R>>,
    pub parts: usize,
}

#[async_trait::async_trait]
impl<R: PreparationRegistry + 'static> crate::tabular_prepare::ports::PrepareRunner
    for PartsJob<R>
{
    async fn run(&self, req: crate::tabular_prepare::ports::PrepareRequest) {
        let claim = ClaimRequest {
            source_key: req.source_key.clone(),
            source_bytes: req.size_bytes as i64,
            format_version: FORMAT_VERSION,
            owner: "job".to_string(),
            lease: lease(),
            now: Utc::now(),
        };
        self.registry.claim(claim).await.unwrap().expect("claimed");
        for _ in 0..self.parts {
            assert!(self
                .registry
                .still_owned(&req.source_key, "job")
                .await
                .unwrap());
        }
        let out = self
            .registry
            .complete(&req.source_key, "job", ready_info(), Utc::now())
            .await
            .unwrap();
        assert_eq!(out, TerminalOutcome::Written);
    }
}

pub(crate) async fn a_preparation_through_ensure_prepared_writes_only_on_claim_and_terminal<
    R: PreparationRegistry + 'static,
>(
    r: R,
) {
    use crate::tabular_prepare::ports::{InlineTrigger, PrepareConfig, PrepareRequest};
    use crate::tabular_prepare::{EnsureOutcome, TabularPrepare};
    let counting = Arc::new(CountingRegistry::new(r));
    let config = PrepareConfig {
        large_tabular: true,
        trigger: Arc::new(InlineTrigger::new(Arc::new(PartsJob {
            registry: counting.clone(),
            parts: 5,
        }))),
        ..PrepareConfig::default()
    };
    let prepare = TabularPrepare::new(config, counting.clone())
        .with_poll_interval(std::time::Duration::from_millis(20));
    let key = fresh_key();
    let req = PrepareRequest {
        source_key: key.clone(),
        mime_type: "text/csv".to_string(),
        filename: "big.csv".to_string(),
        size_bytes: 60_000_000,
    };
    let out = prepare
        .ensure_prepared(&req, std::time::Duration::from_secs(20))
        .await
        .unwrap();
    assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
    let (claims, terminals, other_writes, reads) = counting.snapshot();
    assert_eq!((claims, terminals), (1, 1), "claim and terminal only");
    assert_eq!(other_writes, 1, "only the throttled last_used_at touch");
    assert!(
        reads >= 5 + 2,
        "5 ownership checks and the polling reads, got {reads}"
    );
    assert!(get_row(&*counting, &key).await.last_used_at.is_some());
}

pub(crate) async fn find_stale_selects_old_rows_and_spares_a_live_preparation<
    R: PreparationRegistry,
>(
    r: &R,
) {
    let old = t0() - Duration::days(10);
    let prefix = format!("stale-{}", Uuid::new_v4());
    let key = |n: &str| format!("{prefix}/{n}");
    // An old failed row and an old ready row are stale.
    r.claim(claim_req(&key("failed"), "A", old))
        .await
        .unwrap()
        .unwrap();
    r.fail(&key("failed"), "A", "time", "x", old).await.unwrap();
    r.claim(claim_req(&key("ready"), "A", old))
        .await
        .unwrap()
        .unwrap();
    r.complete(&key("ready"), "A", ready_info(), old)
        .await
        .unwrap();
    // A fresh row is not.
    r.claim(claim_req(&key("fresh"), "A", t0()))
        .await
        .unwrap()
        .unwrap();
    // An old row that was claimed again and holds a live lease is not: it is
    // being prepared right now (the claim keeps the original creation time).
    let mut live = claim_req(&key("live"), "A", old);
    live.lease = Duration::days(11);
    r.claim(live).await.unwrap().unwrap();

    let cutoff = t0() - Duration::days(7);
    let page = r.find_stale(cutoff, t0(), None, 1000).await.unwrap();
    let mut found: Vec<String> = page
        .into_iter()
        .map(|row| row.source_storage_key)
        .filter(|k| k.starts_with(&prefix))
        .collect();
    found.sort();
    assert_eq!(found, vec![key("failed"), key("ready")]);
}

pub(crate) async fn find_stale_honours_the_limit<R: PreparationRegistry>(r: &R) {
    let old = t0() - Duration::days(10);
    for _ in 0..3 {
        let k = fresh_key();
        r.claim(claim_req(&k, "A", old)).await.unwrap().unwrap();
        r.fail(&k, "A", "time", "x", old).await.unwrap();
    }
    let page = r
        .find_stale(t0() - Duration::days(7), t0(), None, 2)
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
}

pub(crate) async fn list_ready_after_pages_through_ready_rows_only<R: PreparationRegistry>(r: &R) {
    let prefix = format!("ready-{}", Uuid::new_v4());
    let key = |n: &str| format!("{prefix}/{n}");
    for n in ["c", "a", "b"] {
        r.claim(claim_req(&key(n), "A", t0()))
            .await
            .unwrap()
            .unwrap();
        r.complete(&key(n), "A", ready_info(), t0()).await.unwrap();
    }
    r.claim(claim_req(&key("d-running"), "A", t0()))
        .await
        .unwrap()
        .unwrap();
    let mut seen = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = r.list_ready_after(after.as_deref(), 2).await.unwrap();
        if page.is_empty() {
            break;
        }
        assert!(page.iter().all(|row| row.status == PrepareStatus::Ready));
        after = Some(page.last().unwrap().source_storage_key.clone());
        seen.extend(
            page.into_iter()
                .map(|row| row.source_storage_key)
                .filter(|k| k.starts_with(&prefix)),
        );
    }
    assert_eq!(
        seen,
        vec![key("a"), key("b"), key("c")],
        "sorted, each once"
    );
}

pub(crate) async fn a_table_in_use_is_not_stale<R: PreparationRegistry>(r: &R) {
    let old = t0() - Duration::days(10);
    let (used, idle) = (fresh_key(), fresh_key());
    for key in [&used, &idle] {
        r.claim(claim_req(key, "A", old)).await.unwrap().unwrap();
        r.complete(key, "A", ready_info(), old).await.unwrap();
    }
    r.touch_last_used(&used, t0() - Duration::days(1), Duration::hours(1))
        .await
        .unwrap();
    let stale: Vec<String> = r
        .find_stale(t0() - Duration::days(7), t0(), None, 10_000)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.source_storage_key)
        .collect();
    assert!(stale.contains(&idle), "created long ago and never used");
    assert!(
        !stale.contains(&used),
        "created long ago but used yesterday"
    );
}

pub(crate) async fn find_stale_pages_with_a_keyset_cursor<R: PreparationRegistry>(r: &R) {
    let old = t0() - Duration::days(10);
    let prefix = format!("page-{}", Uuid::new_v4());
    let keys: Vec<String> = (0..5).map(|i| format!("{prefix}/{i}")).collect();
    for k in &keys {
        r.claim(claim_req(k, "A", old)).await.unwrap().unwrap();
        r.fail(k, "A", "time", "x", old).await.unwrap();
    }
    let cutoff = t0() - Duration::days(7);
    let mut seen = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = r
            .find_stale(cutoff, t0(), after.as_deref(), 2)
            .await
            .unwrap();
        if page.is_empty() {
            break;
        }
        after = Some(page.last().unwrap().source_storage_key.clone());
        seen.extend(
            page.into_iter()
                .map(|row| row.source_storage_key)
                .filter(|k| k.starts_with(&prefix)),
        );
    }
    assert_eq!(seen, keys, "every row once, in key order, none skipped");
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

    #[tokio::test]
    async fn tabular_prepare_a_preparation_writes_only_on_claim_and_terminal() {
        let f = fixture().await;
        let inner = SqlitePreparationRegistry::from_pool(f.registry.pool());
        a_preparation_writes_only_on_claim_and_terminal(inner).await;
    }

    #[tokio::test]
    async fn tabular_prepare_complete_after_the_row_is_deleted_is_cancelled() {
        let f = fixture().await;
        complete_after_the_row_is_deleted_is_cancelled(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_still_owned_tells_the_owner_from_everyone_else() {
        let f = fixture().await;
        still_owned_tells_the_owner_from_everyone_else(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_the_cancellation_check_is_a_read_and_the_lease_is_not_renewed() {
        let f = fixture().await;
        let inner = SqlitePreparationRegistry::from_pool(f.registry.pool());
        the_cancellation_check_is_a_read_and_the_lease_is_not_renewed(inner).await;
    }

    #[tokio::test]
    async fn tabular_prepare_a_duplicate_trigger_after_delete_leaves_no_row() {
        let f = fixture().await;
        a_duplicate_trigger_after_delete_leaves_no_row(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_touch_last_used_marks_use_at_most_once_per_interval() {
        let f = fixture().await;
        touch_last_used_marks_use_at_most_once_per_interval(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_a_preparation_through_ensure_prepared_writes_only_on_claim_and_terminal(
    ) {
        let f = fixture().await;
        let inner = SqlitePreparationRegistry::from_pool(f.registry.pool());
        a_preparation_through_ensure_prepared_writes_only_on_claim_and_terminal(inner).await;
    }

    #[tokio::test]
    async fn tabular_prepare_find_stale_selects_old_rows_and_spares_a_live_preparation() {
        let f = fixture().await;
        find_stale_selects_old_rows_and_spares_a_live_preparation(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_find_stale_honours_the_limit() {
        let f = fixture().await;
        find_stale_honours_the_limit(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_list_ready_after_pages_through_ready_rows_only() {
        let f = fixture().await;
        list_ready_after_pages_through_ready_rows_only(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_a_table_in_use_is_not_stale() {
        let f = fixture().await;
        a_table_in_use_is_not_stale(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_find_stale_pages_with_a_keyset_cursor() {
        let f = fixture().await;
        find_stale_pages_with_a_keyset_cursor(&*f.registry).await;
    }
}
