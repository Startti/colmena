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
    assert_eq!(
        row.created_at, later,
        "a new preparation starts the TTL clock again"
    );
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

/// The first layout (format 1, manifests without `in_memory_bytes`) is gone:
/// a table prepared under it is claimed again by the layout this code writes,
/// and its old objects stay tracked so cleanup can still reach them.
pub(crate) async fn a_ready_row_of_the_first_layout_is_claimable_by_the_current_one<
    R: PreparationRegistry,
>(
    r: &R,
) {
    let key = fresh_key();
    let mut first = claim_req(&key, "old", t0());
    first.format_version = 1;
    r.claim(first).await.unwrap().unwrap();
    r.complete(&key, "old", ready_info(), t0()).await.unwrap();
    assert_eq!(get_row(r, &key).await.format_version, 1);
    let later = t0() + Duration::days(1);
    let claim = r.claim(claim_req(&key, "new", later)).await.unwrap();
    assert_eq!(claim, Some(Claim { attempts: 1 }));
    let row = get_row(r, &key).await;
    assert_eq!(row.status, PrepareStatus::Running);
    assert_eq!(row.format_version, FORMAT_VERSION);
    assert_eq!(row.blob_keys, ready_info().blob_keys);
    // Once ready under the current layout it is not claimed again.
    r.complete(&key, "new", ready_info(), later).await.unwrap();
    assert_eq!(
        r.claim(claim_req(&key, "third", later + Duration::days(30)))
            .await
            .unwrap(),
        None
    );
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
        row.manifest_key.as_deref(),
        Some(ready_info().manifest_key.as_str()),
        "the old manifest stays until complete supersedes it, so cleanup can reach it"
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
    async fn begin_delete(
        &self,
        row: &PreparedRow,
        owner: &str,
        lease: Duration,
        now: DateTime<Utc>,
    ) -> Result<bool, RegistryError> {
        self.counts
            .other_writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.begin_delete(row, owner, lease, now).await
    }

    async fn finish_delete(&self, k: &str, owner: &str) -> Result<bool, RegistryError> {
        self.counts
            .other_writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.finish_delete(k, owner).await
    }
    async fn delete_if_unchanged(&self, row: &PreparedRow) -> Result<bool, RegistryError> {
        self.counts
            .other_writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.delete_if_unchanged(row).await
    }
    async fn mark_manifest_missing(
        &self,
        row: &PreparedRow,
        n: DateTime<Utc>,
    ) -> Result<bool, RegistryError> {
        self.counts
            .other_writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.mark_manifest_missing(row, n).await
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

pub(crate) async fn gc_claims_a_row_before_deleting_it<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    let old = t0() - Duration::days(10);
    r.claim(claim_req(&key, "A", old)).await.unwrap().unwrap();
    r.fail(&key, "A", "time", "x", old).await.unwrap();
    let observed = get_row(r, &key).await;

    assert!(r
        .begin_delete(&observed, "gc-1", Duration::minutes(10), t0())
        .await
        .unwrap());
    let row = get_row(r, &key).await;
    assert_eq!(row.status, PrepareStatus::Deleting);
    assert_eq!(row.lease_owner.as_deref(), Some("gc-1"));
    assert_eq!(row.lease_until, Some(t0() + Duration::minutes(10)));
    // A preparation cannot claim a row that is being deleted, however old,
    // and a second collector cannot take it while the lease is live.
    assert_eq!(r.claim(claim_req(&key, "B", t0())).await.unwrap(), None);
    assert!(!r
        .begin_delete(&row, "gc-2", Duration::minutes(10), t0())
        .await
        .unwrap());
    // Only the owner finishes it.
    assert!(!r.finish_delete(&key, "gc-2").await.unwrap());
    assert!(r.finish_delete(&key, "gc-1").await.unwrap());
    assert_eq!(r.get(&key).await.unwrap(), None);
}

pub(crate) async fn gc_cannot_claim_a_row_that_changed_since_it_was_read<R: PreparationRegistry>(
    r: &R,
) {
    let key = fresh_key();
    let old = t0() - Duration::days(10);
    r.claim(claim_req(&key, "A", old)).await.unwrap().unwrap();
    r.fail(&key, "A", "time", "x", old).await.unwrap();
    let observed = get_row(r, &key).await;
    // A worker re-claims it between GC's read and GC's claim.
    r.claim(claim_req(&key, "worker", t0()))
        .await
        .unwrap()
        .unwrap();
    assert!(!r
        .begin_delete(&observed, "gc", Duration::minutes(10), t0())
        .await
        .unwrap());
    let row = get_row(r, &key).await;
    assert_eq!(
        row.status,
        PrepareStatus::Running,
        "the live preparation is untouched"
    );
    assert_eq!(row.lease_owner.as_deref(), Some("worker"));
    // Even with the fresh observation, a live lease is never claimed.
    assert!(!r
        .begin_delete(&row, "gc", Duration::minutes(10), t0())
        .await
        .unwrap());
}

pub(crate) async fn gc_cannot_claim_a_row_that_was_used_since_it_was_read<
    R: PreparationRegistry,
>(
    r: &R,
) {
    let key = fresh_key();
    let old = t0() - Duration::days(10);
    r.claim(claim_req(&key, "A", old)).await.unwrap().unwrap();
    r.complete(&key, "A", ready_info(), old).await.unwrap();
    let observed = get_row(r, &key).await;
    assert_eq!(observed.last_used_at, None, "read as stale: never used");
    // A table is handed out after GC read the row: only `last_used_at` moves.
    assert!(r
        .touch_last_used(&key, t0(), Duration::hours(1))
        .await
        .unwrap());
    assert!(!r
        .begin_delete(&observed, "gc", Duration::minutes(10), t0())
        .await
        .unwrap());
    let row = get_row(r, &key).await;
    assert_eq!(
        row.status,
        PrepareStatus::Ready,
        "the used table is untouched"
    );
    // Read again (now with a last_used_at), the same row can be claimed when
    // nobody touches it in between, whatever the observed value was.
    assert!(r
        .begin_delete(&row, "gc", Duration::minutes(10), t0())
        .await
        .unwrap());
}

pub(crate) async fn an_abandoned_deleting_row_is_found_and_taken_over<R: PreparationRegistry>(
    r: &R,
) {
    let key = fresh_key();
    // Fresh row (not past the TTL) whose collector died mid-delete.
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    r.fail(&key, "A", "time", "x", t0()).await.unwrap();
    let observed = get_row(r, &key).await;
    assert!(r
        .begin_delete(&observed, "gc-dead", Duration::minutes(10), t0())
        .await
        .unwrap());
    let cutoff = t0() - Duration::days(7);
    let live = t0() + Duration::minutes(5);
    let ids = |rows: Vec<PreparedRow>| -> Vec<String> {
        rows.into_iter().map(|row| row.source_storage_key).collect()
    };
    let while_live = ids(r.find_stale(cutoff, live, None, 10_000).await.unwrap());
    assert!(!while_live.contains(&key), "lease still live");
    let expired = t0() + Duration::minutes(11);
    let found = r.find_stale(cutoff, expired, None, 10_000).await.unwrap();
    let row = found
        .into_iter()
        .find(|row| row.source_storage_key == key)
        .expect("an expired deleting row is picked up whatever its age");
    assert!(r
        .begin_delete(&row, "gc-2", Duration::minutes(10), expired)
        .await
        .unwrap());
    assert!(r.finish_delete(&key, "gc-2").await.unwrap());
}

pub(crate) async fn delete_if_unchanged_only_deletes_the_row_that_was_observed<
    R: PreparationRegistry,
>(
    r: &R,
) {
    let key = fresh_key();
    r.claim(claim_req(&key, "job", t0()))
        .await
        .unwrap()
        .unwrap();
    let observed = get_row(r, &key).await;
    // The job completes after the cleanup read the row: the row changed.
    r.complete(&key, "job", ready_info(), t0() + Duration::seconds(5))
        .await
        .unwrap();
    assert!(!r.delete_if_unchanged(&observed).await.unwrap());
    assert_eq!(
        get_row(r, &key).await.status,
        PrepareStatus::Ready,
        "the finished table is not lost"
    );
    // Observed again, it can be deleted; a second delete finds nothing.
    let fresh = get_row(r, &key).await;
    assert!(r.delete_if_unchanged(&fresh).await.unwrap());
    assert_eq!(r.get(&key).await.unwrap(), None);
    assert!(!r.delete_if_unchanged(&fresh).await.unwrap());

    // A row owned by another job than the observed one is not deleted either.
    let key2 = fresh_key();
    r.claim(claim_req(&key2, "A", t0())).await.unwrap().unwrap();
    let seen = get_row(r, &key2).await;
    r.claim(claim_req(&key2, "B", t0() + Duration::seconds(400)))
        .await
        .unwrap()
        .unwrap();
    assert!(!r.delete_if_unchanged(&seen).await.unwrap());
    assert_eq!(get_row(r, &key2).await.lease_owner.as_deref(), Some("B"));
}

pub(crate) async fn a_deleting_row_is_never_taken_by_an_older_format_claim<
    R: PreparationRegistry,
>(
    r: &R,
) {
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    r.fail(&key, "A", "time", "x", t0()).await.unwrap();
    let observed = get_row(r, &key).await;
    assert!(r
        .begin_delete(&observed, "gc", Duration::minutes(10), t0())
        .await
        .unwrap());
    // A newer format must not take a row the cleanup pass holds with a live lease.
    let mut req = claim_req(&key, "v2", t0() + Duration::minutes(1));
    req.format_version = FORMAT_VERSION + 1;
    assert_eq!(r.claim(req).await.unwrap(), None);
    assert_eq!(get_row(r, &key).await.status, PrepareStatus::Deleting);
}

pub(crate) async fn an_expired_deleting_lease_can_be_claimed_with_a_fresh_start<
    R: PreparationRegistry,
>(
    r: &R,
) {
    let key = fresh_key();
    let old = t0() - Duration::days(10);
    // A final-failed old row that the cleanup pass started deleting and never finished.
    for attempt in 1..=MAX_ATTEMPTS {
        r.claim(claim_req(&key, &format!("j{attempt}"), old))
            .await
            .unwrap()
            .unwrap();
        r.fail(&key, &format!("j{attempt}"), "time", "x", old)
            .await
            .unwrap();
    }
    let observed = get_row(r, &key).await;
    assert!(r
        .begin_delete(&observed, "gc-dead", Duration::minutes(10), t0())
        .await
        .unwrap());
    // Live lease: refused. Expired lease: taken, as a new life of the row.
    assert_eq!(
        r.claim(claim_req(&key, "B", t0() + Duration::minutes(5)))
            .await
            .unwrap(),
        None
    );
    let later = t0() + Duration::minutes(11);
    assert_eq!(
        r.claim(claim_req(&key, "B", later)).await.unwrap(),
        Some(Claim { attempts: 1 }),
        "attempts restart: the old life was being deleted"
    );
    let row = get_row(r, &key).await;
    assert_eq!(row.status, PrepareStatus::Running);
    assert_eq!(
        row.created_at, later,
        "no longer looks stale to the TTL pass"
    );
    assert_eq!(row.last_used_at, None);
}

pub(crate) async fn an_older_format_claim_keeps_the_old_manifest_tracked<R: PreparationRegistry>(
    r: &R,
) {
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    let mut info = ready_info();
    info.manifest_key = "old/manifest.json".to_string();
    r.complete(&key, "A", info, t0()).await.unwrap();
    let mut req = claim_req(&key, "B", t0());
    req.format_version = FORMAT_VERSION + 1;
    r.claim(req).await.unwrap().unwrap();
    let mut next = ready_info();
    next.manifest_key = "new/manifest.json".to_string();
    next.blob_keys = vec!["new/part".to_string()];
    r.complete(&key, "B", next, t0()).await.unwrap();
    let row = get_row(r, &key).await;
    assert_eq!(row.manifest_key.as_deref(), Some("new/manifest.json"));
    assert!(
        row.blob_keys.contains(&"old/manifest.json".to_string()),
        "the superseded manifest stays tracked for cleanup: {:?}",
        row.blob_keys
    );
}

pub(crate) async fn a_ready_row_with_a_missing_manifest_becomes_claimable<
    R: PreparationRegistry,
>(
    r: &R,
) {
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    r.complete(&key, "A", ready_info(), t0()).await.unwrap();
    assert_eq!(r.claim(claim_req(&key, "B", t0())).await.unwrap(), None);

    let at = t0() + Duration::hours(1);
    let observed = get_row(r, &key).await;
    assert!(r.mark_manifest_missing(&observed, at).await.unwrap());
    let row = get_row(r, &key).await;
    assert_eq!(row.status, PrepareStatus::Failed);
    assert_eq!(row.error_code.as_deref(), Some("manifest_missing"));
    assert_eq!(
        row.attempts, 0,
        "a completed preparation cleared the attempts"
    );
    assert_eq!(
        row.blob_keys,
        ready_info().blob_keys,
        "leftover blobs stay listed"
    );
    assert_eq!(
        r.claim(claim_req(&key, "B", at)).await.unwrap(),
        Some(Claim { attempts: 1 })
    );
}

pub(crate) async fn mark_manifest_missing_only_touches_ready_rows<R: PreparationRegistry>(r: &R) {
    let missing = fresh_key();
    let mut ghost = {
        let k = fresh_key();
        r.claim(claim_req(&k, "A", t0())).await.unwrap().unwrap();
        r.complete(&k, "A", ready_info(), t0()).await.unwrap();
        let row = get_row(r, &k).await;
        r.delete(&k).await.unwrap();
        row
    };
    ghost.source_storage_key = missing.clone();
    assert!(!r.mark_manifest_missing(&ghost, t0()).await.unwrap());
    assert_eq!(r.get(&missing).await.unwrap(), None);

    let running = fresh_key();
    r.claim(claim_req(&running, "A", t0()))
        .await
        .unwrap()
        .unwrap();
    let observed = get_row(r, &running).await;
    assert!(!r.mark_manifest_missing(&observed, t0()).await.unwrap());
    assert_eq!(get_row(r, &running).await.status, PrepareStatus::Running);
}

/// The check that found a manifest missing is a snapshot: a table that was
/// prepared again since must not be demoted by it.
pub(crate) async fn mark_manifest_missing_refuses_a_row_changed_since_the_check<
    R: PreparationRegistry,
>(
    r: &R,
) {
    let key = fresh_key();
    r.claim(claim_req(&key, "A", t0())).await.unwrap().unwrap();
    r.complete(&key, "A", ready_info(), t0()).await.unwrap();
    let observed = get_row(r, &key).await;
    // Meanwhile the table is demoted and prepared again (a fresh manifest).
    assert!(r.mark_manifest_missing(&observed, t0()).await.unwrap());
    r.claim(claim_req(&key, "B", t0() + Duration::minutes(1)))
        .await
        .unwrap()
        .unwrap();
    r.complete(&key, "B", ready_info(), t0() + Duration::minutes(2))
        .await
        .unwrap();
    // The stale check's late mark is refused and the fresh table stays ready.
    assert!(!r
        .mark_manifest_missing(&observed, t0() + Duration::minutes(3))
        .await
        .unwrap());
    assert_eq!(get_row(r, &key).await.status, PrepareStatus::Ready);
}

/// The attempt cap bounds CONSECUTIVE failures: a preparation that completes
/// clears the count, so a table that needed retries, or that lost its manifest
/// more than once, is never a permanent failure for that reason alone.
pub(crate) async fn a_completed_preparation_resets_the_attempts<R: PreparationRegistry>(r: &R) {
    let key = fresh_key();
    let mut now = t0();
    // Two failures, then a success on the third attempt.
    for attempt in 1..=MAX_ATTEMPTS - 1 {
        let claim = r.claim(claim_req(&key, "job", now)).await.unwrap();
        assert_eq!(claim, Some(Claim { attempts: attempt }));
        r.fail(&key, "job", "time", "x", now).await.unwrap();
    }
    assert_eq!(
        r.claim(claim_req(&key, "job", now)).await.unwrap(),
        Some(Claim {
            attempts: MAX_ATTEMPTS
        })
    );
    r.complete(&key, "job", ready_info(), now).await.unwrap();
    assert_eq!(
        get_row(r, &key).await.attempts,
        0,
        "completed: count cleared"
    );
    // Its manifest is lost, again and again: each time it is claimable.
    for _ in 0..MAX_ATTEMPTS + 2 {
        let observed = get_row(r, &key).await;
        assert!(r.mark_manifest_missing(&observed, now).await.unwrap());
        now += Duration::minutes(1);
        assert_eq!(
            r.claim(claim_req(&key, "job", now)).await.unwrap(),
            Some(Claim { attempts: 1 }),
            "never a final failure for repeated manifest loss alone"
        );
        r.complete(&key, "job", ready_info(), now).await.unwrap();
    }
}

/// A table that is prepared again (after a failure, a format change or a lost
/// manifest) starts its TTL clock at that preparation: it must not be seen as
/// stale before its first use because the row was created long ago.
pub(crate) async fn a_re_prepared_table_is_not_stale_before_its_first_use<
    R: PreparationRegistry,
>(
    r: &R,
) {
    let old = t0() - Duration::days(10);
    let cutoff = t0() - Duration::days(7);
    let is_stale =
        |rows: Vec<PreparedRow>, key: &str| rows.iter().any(|row| row.source_storage_key == key);

    // 1. a failed retry that succeeds
    let retry = fresh_key();
    r.claim(claim_req(&retry, "A", old)).await.unwrap().unwrap();
    r.fail(&retry, "A", "time", "x", old).await.unwrap();
    r.claim(claim_req(&retry, "B", t0()))
        .await
        .unwrap()
        .unwrap();
    r.complete(&retry, "B", ready_info(), t0()).await.unwrap();

    // 2. an older-format row prepared again
    let reformat = fresh_key();
    r.claim(claim_req(&reformat, "A", old))
        .await
        .unwrap()
        .unwrap();
    r.complete(&reformat, "A", ready_info(), old).await.unwrap();
    let mut req = claim_req(&reformat, "B", t0());
    req.format_version = FORMAT_VERSION + 1;
    r.claim(req).await.unwrap().unwrap();
    r.complete(&reformat, "B", ready_info(), t0())
        .await
        .unwrap();

    // 3. a lost manifest, prepared again
    let lost = fresh_key();
    r.claim(claim_req(&lost, "A", old)).await.unwrap().unwrap();
    r.complete(&lost, "A", ready_info(), old).await.unwrap();
    let observed = get_row(r, &lost).await;
    r.mark_manifest_missing(&observed, t0()).await.unwrap();
    r.claim(claim_req(&lost, "B", t0())).await.unwrap().unwrap();
    r.complete(&lost, "B", ready_info(), t0()).await.unwrap();

    let stale = r.find_stale(cutoff, t0(), None, 10_000).await.unwrap();
    for key in [&retry, &reformat, &lost] {
        assert!(
            !is_stale(stale.clone(), key),
            "{key} would be deleted before first use"
        );
    }
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
    async fn tabular_prepare_a_ready_row_of_the_first_layout_is_claimable_by_the_current_one() {
        let f = fixture().await;
        a_ready_row_of_the_first_layout_is_claimable_by_the_current_one(&*f.registry).await;
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

    #[tokio::test]
    async fn tabular_prepare_gc_claims_a_row_before_deleting_it() {
        let f = fixture().await;
        gc_claims_a_row_before_deleting_it(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_gc_cannot_claim_a_row_that_changed_since_it_was_read() {
        let f = fixture().await;
        gc_cannot_claim_a_row_that_changed_since_it_was_read(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_gc_cannot_claim_a_row_that_was_used_since_it_was_read() {
        let f = fixture().await;
        gc_cannot_claim_a_row_that_was_used_since_it_was_read(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_an_abandoned_deleting_row_is_found_and_taken_over() {
        let f = fixture().await;
        an_abandoned_deleting_row_is_found_and_taken_over(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_delete_if_unchanged_only_deletes_the_row_that_was_observed() {
        let f = fixture().await;
        delete_if_unchanged_only_deletes_the_row_that_was_observed(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_a_deleting_row_is_never_taken_by_an_older_format_claim() {
        let f = fixture().await;
        a_deleting_row_is_never_taken_by_an_older_format_claim(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_an_expired_deleting_lease_can_be_claimed_with_a_fresh_start() {
        let f = fixture().await;
        an_expired_deleting_lease_can_be_claimed_with_a_fresh_start(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_an_older_format_claim_keeps_the_old_manifest_tracked() {
        let f = fixture().await;
        an_older_format_claim_keeps_the_old_manifest_tracked(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_a_ready_row_with_a_missing_manifest_becomes_claimable() {
        let f = fixture().await;
        a_ready_row_with_a_missing_manifest_becomes_claimable(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_mark_manifest_missing_only_touches_ready_rows() {
        let f = fixture().await;
        mark_manifest_missing_only_touches_ready_rows(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_mark_manifest_missing_refuses_a_row_changed_since_the_check() {
        let f = fixture().await;
        mark_manifest_missing_refuses_a_row_changed_since_the_check(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_a_completed_preparation_resets_the_attempts() {
        let f = fixture().await;
        a_completed_preparation_resets_the_attempts(&*f.registry).await;
    }

    #[tokio::test]
    async fn tabular_prepare_a_re_prepared_table_is_not_stale_before_its_first_use() {
        let f = fixture().await;
        a_re_prepared_table_is_not_stale_before_its_first_use(&*f.registry).await;
    }
}
