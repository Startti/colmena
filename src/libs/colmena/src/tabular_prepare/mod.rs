//! Large tabular preparation (dark behind `COLMENA_LARGE_TABULAR`).
//!
//! A large CSV/Excel attachment is prepared once into typed columnar tables,
//! tracked in a registry keyed by the source `storage_key`. This module holds
//! the registry ([`registry`], with SQLite and Postgres implementations), the
//! ports the host replaces ([`ports`]) and the `ensure_prepared` entry a tool
//! calls before it needs the tables ([`TabularPrepare::ensure_prepared`]); the
//! cleanup pass `attachment_gc` runs will follow. The CSV converter
//! ([`convert::convert_csv_table`]) is here too; nothing calls it yet.
//!
//! With the engine switch off (the default) none of it runs.
//! See `docs/developer_guide/54_tabular_prepare.md`.

use crate::tabular_prepare::ports::{
    PrepareConfig, PrepareProgressInfo, PrepareRequest, PrepareTriggerError, ProgressState,
};
use crate::tabular_prepare::registry::{
    PreparationRegistry, PrepareStatus, PreparedRow, RegistryError, FORMAT_VERSION, MAX_ATTEMPTS,
};
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::time::Instant;

pub mod convert;
pub mod csv;
pub mod driver;
pub mod gc;
pub mod infer;
pub mod manifest;
pub mod part_sink;
pub mod ports;
pub mod postgres_registry;
pub mod precheck;
pub mod prepare;
pub mod registry;
pub mod scan;
pub mod sqlite_registry;
pub mod writer;
pub mod xlsx_package;
pub mod xlsx_spool;
pub mod xlsx_styles;
pub mod xlsx_workbook;

#[cfg(test)]
mod registry_contract;
#[cfg(test)]
mod registry_faults;
#[cfg(test)]
mod xlsxfix;
#[cfg(test)]
mod zipfix;

/// How often a waiting caller re-reads the registry.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Minimum time any single registry or port call is given, whatever the wait.
const CALL_FLOOR: Duration = Duration::from_secs(2);

/// A table handed out is recorded as used at most once per this interval (a day).
const LAST_USED_INTERVAL: chrono::Duration = chrono::Duration::days(1);

/// Upper bound for a caller's wait, so `Instant + wait` cannot overflow.
const MAX_WAIT: Duration = Duration::from_secs(10 * 365 * 24 * 3600);

/// Result of [`TabularPrepare::ensure_prepared`]. There is deliberately no
/// variant that offers the whole file: a file that is not prepared is never
/// loaded whole into the sandbox.
#[derive(Debug, Clone, PartialEq)]
pub enum EnsureOutcome {
    /// The feature is off: nothing was read, requested or written.
    NotEnabled,
    Ready(Box<PreparedRow>),
    /// The wait ended before the preparation finished. Not an error: the
    /// caller tells the model to retry.
    StillPreparing {
        progress: Option<PrepareProgressInfo>,
    },
    Failed {
        error_code: String,
        error_detail: String,
        attempts: i32,
        /// `true` once the attempts are exhausted: no further request helps.
        final_failure: bool,
    },
}

impl EnsureOutcome {
    /// Text for the model or the user.
    pub fn message(&self) -> String {
        match self {
            EnsureOutcome::NotEnabled => {
                "large tabular preparation is not enabled in this environment".to_string()
            }
            EnsureOutcome::Ready(_) => "the file is prepared".to_string(),
            EnsureOutcome::StillPreparing { progress } => match progress {
                Some(PrepareProgressInfo {
                    done,
                    total: Some(total),
                    ..
                }) if *total > 0 => {
                    let percent = done.saturating_mul(100) / total;
                    format!("the file is still being prepared ({percent} %); retry shortly")
                }
                _ => "the file is still being prepared; retry shortly".to_string(),
            },
            EnsureOutcome::Failed {
                error_code,
                error_detail,
                attempts,
                final_failure,
            } => {
                if *final_failure {
                    format!(
                        "preparing this file failed after {attempts} attempts ({error_code}): \
                         {error_detail}. It will not be retried and the file cannot be analysed here"
                    )
                } else {
                    format!(
                        "preparing this file failed (attempt {attempts} of {MAX_ATTEMPTS}, \
                         {error_code}): {error_detail}. A new request will try again"
                    )
                }
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum PrepareError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Trigger(#[from] PrepareTriggerError),
}

/// Entry point of the lifecycle: wraps the registry and the ports.
pub struct TabularPrepare {
    config: PrepareConfig,
    registry: Arc<dyn PreparationRegistry>,
    clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    /// Sources this process is currently requesting or waiting on, so that
    /// concurrent callers share one trigger.
    requested: Mutex<HashSet<String>>,
    poll_interval: Duration,
}

/// Removes a source from the in-flight set when it is dropped. It is created
/// at the moment the key is inserted, so every exit path (an early return, an
/// error, a future dropped while awaiting a port) removes the key.
struct RequestedGuard<'a> {
    owner: &'a TabularPrepare,
    key: String,
}

impl Drop for RequestedGuard<'_> {
    fn drop(&mut self) {
        self.owner.requested.lock().unwrap().remove(&self.key);
    }
}

/// What a registry row means to a caller that needs the tables.
enum Observed {
    Ready(PreparedRow),
    FinalFailure(PreparedRow),
    /// A job that died without writing `failed` on every allowed attempt: the
    /// row is still `running` with an expired lease and the claim refuses it.
    DeadJob(PreparedRow),
    /// A live job holds it: attach, never trigger again.
    Running,
    /// Nobody holds it: no row, a retryable failure, an expired lease or an
    /// older format. `failed_attempts` is set for a failed row; `has_row`
    /// tells "no row at all" (where the host's progress hint may say it is
    /// already queued) from a row that can be taken over.
    Claimable {
        has_row: bool,
        failed_attempts: Option<i32>,
    },
    /// A failure after our own request.
    NewFailure(PreparedRow),
}

/// A port call that must not outlive the caller's wait bound, but always gets
/// [`CALL_FLOOR`] to answer: a real registry read is pending on its first
/// poll, so a zero or very short wait must not report `StillPreparing` for a
/// table that is already ready.
async fn within<T>(deadline: Instant, fut: impl std::future::Future<Output = T>) -> Option<T> {
    let floor = Instant::now() + CALL_FLOOR;
    tokio::time::timeout_at(deadline.max(floor), fut).await.ok()
}

fn still_preparing(progress: Option<PrepareProgressInfo>) -> EnsureOutcome {
    EnsureOutcome::StillPreparing { progress }
}

impl TabularPrepare {
    pub fn new(config: PrepareConfig, registry: Arc<dyn PreparationRegistry>) -> Self {
        Self {
            config,
            registry,
            clock: Arc::new(Utc::now),
            requested: Mutex::new(HashSet::new()),
            poll_interval: POLL_INTERVAL,
        }
    }

    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Make sure `req.source_key` is prepared, waiting at most `wait`.
    ///
    /// - Switch off: `NotEnabled`, nothing is touched.
    /// - A live job holds it: attach and wait, no second trigger.
    /// - Nobody holds it: request once, then wait.
    /// - The wait ends first: `StillPreparing`, never an error. The bound
    ///   covers the awaited registry and port calls too, not only the sleeps.
    ///
    /// Known limits: concurrent callers in this process share one request, but
    /// callers in other processes (and a later call after a wait ended) may
    /// request again, which is safe because triggers are idempotent. With the
    /// switch on and no runner wired the request goes nowhere and every call
    /// ends `StillPreparing`.
    pub async fn ensure_prepared(
        &self,
        req: &PrepareRequest,
        wait: Duration,
    ) -> Result<EnsureOutcome, PrepareError> {
        if !self.config.large_tabular {
            return Ok(EnsureOutcome::NotEnabled);
        }
        let deadline = Instant::now() + wait.min(MAX_WAIT);
        let mut guard: Option<RequestedGuard> = None;
        let mut baseline: Option<Option<i32>> = None;
        let mut requested_by_us = false;
        loop {
            let Some(row) = within(deadline, self.registry.get(&req.source_key)).await else {
                return Ok(still_preparing(None));
            };
            let observed = self.observe(row?, baseline);
            match observed {
                Observed::Ready(row) => {
                    // Decide from the row already read: the touch only changes
                    // something when the use was last recorded longer ago than
                    // the interval (or never). Otherwise it is the normal,
                    // throttled case: hand the table out with no write attempt
                    // and no extra registry read.
                    let due = row
                        .last_used_at
                        .is_none_or(|at| at < (self.clock)() - LAST_USED_INTERVAL);
                    if !due || self.mark_used(&req.source_key).await {
                        return Ok(EnsureOutcome::Ready(Box::new(row)));
                    }
                    // The touch was expected to change the row and did not: it
                    // is no longer `ready` (the cleanup pass took it since we
                    // read it). Look again; only a row that is still ready is
                    // handed out, anything else follows the normal path below.
                    // A failed re-read is best-effort, exactly like a failed
                    // touch: it is logged and the table we hold is handed out
                    // (failing the call here would turn a bookkeeping hiccup
                    // into a tool error); a re-read that outlives the wait
                    // bound ends the call as still preparing.
                    let Some(again) = within(deadline, self.registry.get(&req.source_key)).await
                    else {
                        return Ok(still_preparing(None));
                    };
                    match again {
                        Ok(Some(still)) if still.status == PrepareStatus::Ready => {
                            return Ok(EnsureOutcome::Ready(Box::new(still)));
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(
                                target: "colmena::tabular_prepare",
                                source_key = %req.source_key,
                                error = %e,
                                "could not re-read a prepared table after an unrecorded use; handing it out"
                            );
                            return Ok(EnsureOutcome::Ready(Box::new(row)));
                        }
                    }
                }
                Observed::FinalFailure(row) => return Ok(failed(&row, true)),
                Observed::NewFailure(row) => return Ok(failed(&row, false)),
                Observed::DeadJob(row) => return Ok(dead_job(&row)),
                Observed::Running => {}
                Observed::Claimable {
                    has_row,
                    failed_attempts,
                } => {
                    baseline.get_or_insert(failed_attempts);
                    if !requested_by_us {
                        if let Some(owned) = self.claim_the_request(req, has_row, deadline).await {
                            // Held across the request: a failure or a dropped
                            // call is not remembered as "already requested".
                            match within(deadline, self.config.trigger.request(req.clone())).await {
                                Some(sent) => sent?,
                                None => return Ok(still_preparing(None)),
                            }
                            guard = Some(owned);
                            requested_by_us = true;
                        }
                    }
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                drop(guard);
                let progress = within(deadline, self.config.progress.read(&req.source_key))
                    .await
                    .flatten();
                return Ok(still_preparing(progress));
            }
            tokio::time::sleep(self.poll_interval.min(left)).await;
        }
    }

    /// Decide whether this call must send the request, returning the guard
    /// that keeps the source marked in flight. Concurrent callers in this
    /// process share one; an item the host already queued (visible through the
    /// progress port while there is no row yet) is not requested again. The
    /// hint is ignored when a row exists: a stale one must not block the
    /// takeover of a dead job.
    async fn claim_the_request<'a>(
        &'a self,
        req: &PrepareRequest,
        has_row: bool,
        deadline: Instant,
    ) -> Option<RequestedGuard<'a>> {
        if !self
            .requested
            .lock()
            .unwrap()
            .insert(req.source_key.clone())
        {
            return None;
        }
        let guard = RequestedGuard {
            owner: self,
            key: req.source_key.clone(),
        };
        if !has_row {
            let hint = within(deadline, self.config.progress.read(&req.source_key))
                .await
                .flatten();
            let queued = matches!(
                hint,
                Some(PrepareProgressInfo {
                    state: ProgressState::Queued | ProgressState::Running,
                    ..
                })
            );
            if queued {
                return None;
            }
        }
        Some(guard)
    }

    /// A ready table is being handed out: note the use (throttled), so the
    /// TTL pass measures use and not creation. Best-effort: a failure or a
    /// timeout is logged and counts as recorded (the caller is not failed or
    /// delayed). Only called when the touch is due (the use was last recorded
    /// longer ago than the interval, or never). Returns `false` only when the
    /// registry answered that the row was not changed although it was due: the
    /// row is no longer `ready` (the cleanup pass took it), and the caller must
    /// look again before handing the table out.
    async fn mark_used(&self, source_key: &str) -> bool {
        let now = (self.clock)();
        let bound = Instant::now();
        let outcome = within(
            bound,
            self.registry
                .touch_last_used(source_key, now, LAST_USED_INTERVAL),
        )
        .await;
        let Some(outcome) = outcome else {
            tracing::warn!(
                target: "colmena::tabular_prepare",
                source_key,
                "recording the use of a prepared table timed out"
            );
            return true;
        };
        match outcome {
            Ok(changed) => changed,
            Err(e) => {
                tracing::warn!(
                    target: "colmena::tabular_prepare",
                    source_key,
                    error = %e,
                    "could not record the use of a prepared table"
                );
                true
            }
        }
    }

    fn observe(&self, row: Option<PreparedRow>, baseline: Option<Option<i32>>) -> Observed {
        let Some(row) = row else {
            return Observed::Claimable {
                has_row: false,
                failed_attempts: None,
            };
        };
        // Being removed by the cleanup pass: a live lease means wait. An
        // expired one means the pass died or keeps failing; the claim takes
        // such a row, so the source is requested like any other and is never
        // stuck behind a cleanup that is not coming back. Checked before the
        // format: the format must not outrank `deleting`.
        if row.status == PrepareStatus::Deleting {
            return match row.lease_until {
                Some(until) if until >= (self.clock)() => Observed::Running,
                _ => Observed::Claimable {
                    has_row: true,
                    failed_attempts: None,
                },
            };
        }
        if row.format_version < FORMAT_VERSION {
            return Observed::Claimable {
                has_row: true,
                failed_attempts: None,
            };
        }
        match row.status {
            PrepareStatus::Ready => Observed::Ready(row),
            PrepareStatus::Failed if row.attempts >= MAX_ATTEMPTS => Observed::FinalFailure(row),
            PrepareStatus::Failed => match baseline {
                // The failure we found before requesting is not a new one;
                // a higher attempt count, or any failure we did not start
                // from, is.
                Some(Some(seen)) if row.attempts <= seen => Observed::Claimable {
                    has_row: true,
                    failed_attempts: Some(seen),
                },
                Some(_) => Observed::NewFailure(row),
                None => Observed::Claimable {
                    has_row: true,
                    failed_attempts: Some(row.attempts),
                },
            },
            PrepareStatus::Deleting => unreachable!("handled above"),
            PrepareStatus::Running => match row.lease_until {
                Some(until) if until >= (self.clock)() => Observed::Running,
                _ if row.attempts >= MAX_ATTEMPTS => Observed::DeadJob(row),
                _ => Observed::Claimable {
                    has_row: true,
                    failed_attempts: None,
                },
            },
        }
    }
}

fn dead_job(row: &PreparedRow) -> EnsureOutcome {
    EnsureOutcome::Failed {
        error_code: "lease_expired".to_string(),
        error_detail: "the preparation job stopped without reporting a result".to_string(),
        attempts: row.attempts,
        final_failure: true,
    }
}

fn failed(row: &PreparedRow, final_failure: bool) -> EnsureOutcome {
    EnsureOutcome::Failed {
        error_code: row
            .error_code
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        error_detail: row.error_detail.clone().unwrap_or_default(),
        attempts: row.attempts,
        final_failure,
    }
}

#[cfg(test)]
mod ensure_tests {
    use super::*;
    use crate::tabular_prepare::ports::*;
    use crate::tabular_prepare::registry::*;
    use async_trait::async_trait;
    use chrono::{Duration as ChronoDuration, TimeZone};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()
    }

    fn row(status: PrepareStatus, attempts: i32, lease_secs: Option<i64>) -> PreparedRow {
        PreparedRow {
            source_storage_key: "k".to_string(),
            status,
            format_version: FORMAT_VERSION,
            manifest_key: None,
            blob_keys: vec![],
            tables_json: None,
            source_bytes: 60_000_000,
            prepared_bytes: None,
            error_code: matches!(status, PrepareStatus::Failed).then(|| "time".to_string()),
            error_detail: matches!(status, PrepareStatus::Failed).then(|| "too slow".to_string()),
            lease_owner: lease_secs.map(|_| "job".to_string()),
            lease_until: lease_secs.map(|s| now() + ChronoDuration::seconds(s)),
            attempts,
            created_at: now(),
            updated_at: now(),
            last_used_at: None,
        }
    }

    /// Registry whose single row can change while a call is waiting. Only
    /// `get` exists: `ensure_prepared` must never write.
    struct FakeRegistry {
        current: Mutex<Option<PreparedRow>>,
        gets: AtomicUsize,
        touches: AtomicUsize,
        fail_touch: Mutex<bool>,
        hang_touch: Mutex<bool>,
        /// What `touch_last_used` answers (default true) and the row it leaves
        /// behind (cleanup taking the row between the read and the touch).
        touch_answer: Mutex<Option<bool>>,
        /// The nth `get` (1-based) fails.
        fail_get_on: Mutex<Option<usize>>,
        after_touch: Mutex<Option<Option<PreparedRow>>>,
    }

    impl FakeRegistry {
        fn new(row: Option<PreparedRow>) -> Arc<Self> {
            Arc::new(Self {
                current: Mutex::new(row),
                gets: AtomicUsize::new(0),
                touches: AtomicUsize::new(0),
                fail_touch: Mutex::new(false),
                hang_touch: Mutex::new(false),
                touch_answer: Mutex::new(None),
                fail_get_on: Mutex::new(None),
                after_touch: Mutex::new(None),
            })
        }
        fn set(&self, row: Option<PreparedRow>) {
            *self.current.lock().unwrap() = row;
        }
    }

    #[async_trait]
    impl PreparationRegistry for FakeRegistry {
        async fn get(&self, _k: &str) -> Result<Option<PreparedRow>, RegistryError> {
            // A real registry read is pending on its first poll.
            tokio::task::yield_now().await;
            let call = self.gets.fetch_add(1, Ordering::SeqCst) + 1;
            if *self.fail_get_on.lock().unwrap() == Some(call) {
                return Err(RegistryError::Backend("read refused".into()));
            }
            Ok(self.current.lock().unwrap().clone())
        }
        async fn claim(&self, _r: ClaimRequest) -> Result<Option<Claim>, RegistryError> {
            panic!("ensure_prepared must not claim")
        }
        async fn complete(
            &self,
            _k: &str,
            _o: &str,
            _i: ReadyInfo,
            _n: DateTime<Utc>,
        ) -> Result<TerminalOutcome, RegistryError> {
            panic!("ensure_prepared must not write")
        }
        async fn track_blobs(
            &self,
            _k: &str,
            _o: &str,
            _b: &[String],
            _n: DateTime<Utc>,
        ) -> Result<TerminalOutcome, RegistryError> {
            panic!("ensure_prepared must not write")
        }
        async fn fail_with_blobs(
            &self,
            _k: &str,
            _o: &str,
            _c: &str,
            _d: &str,
            _b: &[String],
            _n: DateTime<Utc>,
        ) -> Result<TerminalOutcome, RegistryError> {
            panic!("ensure_prepared must not write")
        }
        async fn still_owned(&self, _k: &str, _o: &str) -> Result<bool, RegistryError> {
            panic!("ensure_prepared must not check ownership")
        }
        async fn release(&self, _k: &str, _o: &str) -> Result<bool, RegistryError> {
            panic!("ensure_prepared must not write")
        }
        async fn delete(&self, _k: &str) -> Result<bool, RegistryError> {
            panic!("ensure_prepared must not write")
        }
        async fn find_stale(
            &self,
            _c: DateTime<Utc>,
            _n: DateTime<Utc>,
            _a: Option<&str>,
            _l: u32,
        ) -> Result<Vec<PreparedRow>, RegistryError> {
            panic!("ensure_prepared must not run the cleanup pass")
        }
        async fn list_ready_after(
            &self,
            _a: Option<&str>,
            _l: u32,
        ) -> Result<Vec<PreparedRow>, RegistryError> {
            panic!("ensure_prepared must not run the cleanup pass")
        }
        async fn begin_delete(
            &self,
            _r: &PreparedRow,
            _o: &str,
            _l: ChronoDuration,
            _n: DateTime<Utc>,
        ) -> Result<bool, RegistryError> {
            panic!("ensure_prepared must not run the cleanup pass")
        }
        async fn finish_delete(&self, _k: &str, _o: &str) -> Result<bool, RegistryError> {
            panic!("ensure_prepared must not run the cleanup pass")
        }
        async fn delete_if_unchanged(&self, _r: &PreparedRow) -> Result<bool, RegistryError> {
            panic!("ensure_prepared must not run the cleanup pass")
        }
        async fn mark_manifest_missing(
            &self,
            _r: &PreparedRow,
            _n: DateTime<Utc>,
        ) -> Result<bool, RegistryError> {
            panic!("ensure_prepared must not write")
        }
        async fn touch_last_used(
            &self,
            _k: &str,
            _n: DateTime<Utc>,
            _i: ChronoDuration,
        ) -> Result<bool, RegistryError> {
            self.touches.fetch_add(1, Ordering::SeqCst);
            if *self.hang_touch.lock().unwrap() {
                std::future::pending::<()>().await;
            }
            if *self.fail_touch.lock().unwrap() {
                return Err(RegistryError::Backend("touch refused".into()));
            }
            if let Some(next) = self.after_touch.lock().unwrap().take() {
                *self.current.lock().unwrap() = next;
            }
            Ok(self.touch_answer.lock().unwrap().unwrap_or(true))
        }
    }

    #[derive(Default)]
    struct FakeTrigger {
        requests: Mutex<Vec<PrepareRequest>>,
        fail: bool,
    }

    #[async_trait]
    impl PrepareTrigger for FakeTrigger {
        async fn request(&self, req: PrepareRequest) -> Result<(), PrepareTriggerError> {
            // Let a concurrent caller run while the request is in flight.
            tokio::task::yield_now().await;
            if self.fail {
                return Err(PrepareTriggerError::Unavailable("queue down".into()));
            }
            self.requests.lock().unwrap().push(req);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeProgress {
        reads: AtomicUsize,
        value: Mutex<Option<PrepareProgressInfo>>,
        /// While set, `read` never completes (a slow progress backend).
        block: Mutex<bool>,
    }

    #[async_trait]
    impl PrepareProgress for FakeProgress {
        async fn report(&self, _k: &str, _i: PrepareProgressInfo) {}
        async fn read(&self, _k: &str) -> Option<PrepareProgressInfo> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if *self.block.lock().unwrap() {
                std::future::pending::<()>().await;
            }
            self.value.lock().unwrap().clone()
        }
    }

    struct Harness {
        prepare: Arc<TabularPrepare>,
        registry: Arc<FakeRegistry>,
        trigger: Arc<FakeTrigger>,
        progress: Arc<FakeProgress>,
    }

    fn harness(enabled: bool, row: Option<PreparedRow>) -> Harness {
        harness_with(enabled, row, FakeTrigger::default())
    }

    fn harness_with(enabled: bool, row: Option<PreparedRow>, trigger: FakeTrigger) -> Harness {
        let registry = FakeRegistry::new(row);
        let trigger = Arc::new(trigger);
        let progress = Arc::new(FakeProgress::default());
        let config = PrepareConfig {
            large_tabular: enabled,
            trigger: trigger.clone(),
            progress: progress.clone(),
        };
        let prepare =
            Arc::new(TabularPrepare::new(config, registry.clone()).with_clock(Arc::new(now)));
        Harness {
            prepare,
            registry,
            trigger,
            progress,
        }
    }

    fn req() -> PrepareRequest {
        PrepareRequest {
            source_key: "k".to_string(),
            mime_type: "text/csv".to_string(),
            filename: "sales.csv".to_string(),
            size_bytes: 60_000_000,
        }
    }

    const WAIT: Duration = Duration::from_secs(30);

    fn requests(h: &Harness) -> usize {
        h.trigger.requests.lock().unwrap().len()
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_with_the_switch_off_touches_nothing() {
        let h = harness(false, Some(row(PrepareStatus::Failed, 1, None)));
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert_eq!(out, EnsureOutcome::NotEnabled);
        assert_eq!(h.registry.gets.load(Ordering::SeqCst), 0);
        assert_eq!(requests(&h), 0);
        assert_eq!(h.progress.reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_returns_a_ready_row_without_a_trigger() {
        let ready = row(PrepareStatus::Ready, 1, None);
        let h = harness(true, Some(ready.clone()));
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert_eq!(out, EnsureOutcome::Ready(Box::new(ready)));
        assert_eq!(requests(&h), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_attaches_to_a_running_job_without_a_second_trigger() {
        let h = harness(true, Some(row(PrepareStatus::Running, 1, Some(300))));
        let registry = h.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            registry.set(Some(row(PrepareStatus::Ready, 1, None)));
        });
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(requests(&h), 0, "a running job is never triggered again");
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_requests_once_when_there_is_no_job() {
        let h = harness(true, None);
        let registry = h.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(4)).await;
            registry.set(Some(row(PrepareStatus::Running, 1, Some(300))));
            tokio::time::sleep(Duration::from_secs(4)).await;
            registry.set(Some(row(PrepareStatus::Ready, 1, None)));
        });
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(requests(&h), 1);
        assert_eq!(h.trigger.requests.lock().unwrap()[0], req());
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_attaches_to_an_item_the_host_already_queued() {
        let h = harness(true, None);
        *h.progress.value.lock().unwrap() = Some(PrepareProgressInfo {
            state: ProgressState::Queued,
            done: 0,
            total: None,
        });
        let registry = h.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            registry.set(Some(row(PrepareStatus::Ready, 1, None)));
        });
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(requests(&h), 0, "already queued: no second request");
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_wait_expires_as_still_preparing_not_an_error() {
        let h = harness(true, Some(row(PrepareStatus::Running, 1, Some(3600))));
        *h.progress.value.lock().unwrap() = Some(PrepareProgressInfo {
            state: ProgressState::Running,
            done: 40,
            total: Some(100),
        });
        let started = tokio::time::Instant::now();
        let out = h
            .prepare
            .ensure_prepared(&req(), Duration::from_secs(10))
            .await
            .unwrap();
        let waited = started.elapsed();
        assert_eq!(
            out,
            EnsureOutcome::StillPreparing {
                progress: Some(PrepareProgressInfo {
                    state: ProgressState::Running,
                    done: 40,
                    total: Some(100),
                })
            }
        );
        assert!(out.message().contains("40 %"), "{}", out.message());
        assert!(
            waited >= Duration::from_secs(10) && waited <= Duration::from_secs(11),
            "waited {waited:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_failed_row_under_three_attempts_is_requested_again() {
        let h = harness(true, Some(row(PrepareStatus::Failed, 1, None)));
        let registry = h.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            registry.set(Some(row(PrepareStatus::Running, 2, Some(300))));
            tokio::time::sleep(Duration::from_secs(5)).await;
            registry.set(Some(row(PrepareStatus::Ready, 2, None)));
        });
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(
            requests(&h),
            1,
            "re-requested once, the stale failure is not mistaken for a new one"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_reports_a_new_failure_that_is_not_yet_final() {
        let h = harness(true, Some(row(PrepareStatus::Failed, 1, None)));
        let registry = h.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            registry.set(Some(row(PrepareStatus::Failed, 2, None)));
        });
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        match out {
            EnsureOutcome::Failed {
                attempts,
                final_failure,
                ..
            } => {
                assert_eq!(attempts, 2);
                assert!(!final_failure, "a later request may try again");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(requests(&h), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_third_failure_is_final_with_a_clear_error() {
        let h = harness(true, Some(row(PrepareStatus::Failed, MAX_ATTEMPTS, None)));
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        let message = out.message();
        match &out {
            EnsureOutcome::Failed {
                error_code,
                attempts,
                final_failure,
                ..
            } => {
                assert_eq!(error_code, "time");
                assert_eq!(*attempts, MAX_ATTEMPTS);
                assert!(*final_failure);
            }
            other => panic!("expected a final failure, got {other:?}"),
        }
        assert!(message.contains("3 attempts"), "{message}");
        assert!(message.contains("time"), "{message}");
        assert!(message.contains("too slow"), "{message}");
        assert!(
            !message.to_lowercase().contains("whole file"),
            "a failure never offers a whole-file fallback: {message}"
        );
        assert_eq!(requests(&h), 0, "a final failure is not triggered again");
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_takes_over_an_expired_running_lease_by_requesting() {
        let h = harness(true, Some(row(PrepareStatus::Running, 1, Some(-5))));
        let registry = h.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            registry.set(Some(row(PrepareStatus::Ready, 2, None)));
        });
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(requests(&h), 1, "the dead job is replaced");
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_ready_row_of_an_older_format_is_prepared_again() {
        let mut old = row(PrepareStatus::Ready, 1, None);
        old.format_version = FORMAT_VERSION - 1;
        let h = harness(true, Some(old));
        let registry = h.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            registry.set(Some(row(PrepareStatus::Ready, 1, None)));
        });
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(requests(&h), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_dead_job_at_the_attempt_cap_is_a_final_failure() {
        // The job died three times without writing `failed`: the row is still
        // `running` with an expired lease and the claim refuses it.
        let h = harness(
            true,
            Some(row(PrepareStatus::Running, MAX_ATTEMPTS, Some(-5))),
        );
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        match &out {
            EnsureOutcome::Failed {
                error_code,
                attempts,
                final_failure,
                ..
            } => {
                assert_eq!(error_code, "lease_expired");
                assert_eq!(*attempts, MAX_ATTEMPTS);
                assert!(*final_failure);
            }
            other => panic!("expected a final failure, got {other:?}"),
        }
        assert!(out.message().contains("3 attempts"), "{}", out.message());
        assert_eq!(
            requests(&h),
            0,
            "nothing can claim it, so nothing is requested"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_dead_job_under_the_cap_is_still_taken_over() {
        let h = harness(
            true,
            Some(row(PrepareStatus::Running, MAX_ATTEMPTS - 1, Some(-5))),
        );
        let registry = h.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            registry.set(Some(row(PrepareStatus::Ready, MAX_ATTEMPTS, None)));
        });
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(requests(&h), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_stale_hint_does_not_block_a_takeover() {
        let hint = || {
            Some(PrepareProgressInfo {
                state: ProgressState::Running,
                done: 1,
                total: Some(10),
            })
        };
        // An expired lease and an older format both have a row: the hint
        // (which may outlive a dead job) is not consulted.
        let mut old_format = row(PrepareStatus::Ready, 1, None);
        old_format.format_version = FORMAT_VERSION - 1;
        for existing in [row(PrepareStatus::Running, 1, Some(-5)), old_format] {
            let h = harness(true, Some(existing));
            *h.progress.value.lock().unwrap() = hint();
            let registry = h.registry.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                registry.set(Some(row(PrepareStatus::Ready, 2, None)));
            });
            let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
            assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
            assert_eq!(requests(&h), 1, "takeover requested despite the stale hint");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_waits_on_a_row_being_deleted_without_requesting() {
        let mut deleting = row(PrepareStatus::Deleting, 1, Some(600));
        deleting.lease_owner = Some("gc".to_string());
        let h = harness(true, Some(deleting));
        let registry = h.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            registry.set(None); // the collector finished
            tokio::time::sleep(Duration::from_secs(3)).await;
            registry.set(Some(row(PrepareStatus::Ready, 1, None)));
        });
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(requests(&h), 1, "requested only after the row was gone");
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_an_expired_deleting_lease_is_recovered_by_requesting() {
        let mut deleting = row(PrepareStatus::Deleting, 1, Some(-5));
        deleting.lease_owner = Some("gc-dead".to_string());
        let h = harness(true, Some(deleting));
        let registry = h.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            registry.set(Some(row(PrepareStatus::Ready, 1, None)));
        });
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(
            requests(&h),
            1,
            "the source is not stuck behind a dead cleanup"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_live_deleting_lease_with_an_older_format_still_waits() {
        let mut deleting = row(PrepareStatus::Deleting, 1, Some(600));
        deleting.format_version = FORMAT_VERSION - 1;
        let h = harness(true, Some(deleting));
        let out = h
            .prepare
            .ensure_prepared(&req(), Duration::from_secs(5))
            .await
            .unwrap();
        assert!(
            matches!(out, EnsureOutcome::StillPreparing { .. }),
            "got {out:?}"
        );
        assert_eq!(
            requests(&h),
            0,
            "the format check must not outrank `deleting`"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_concurrent_callers_cause_one_trigger() {
        let h = harness(true, None);
        let registry = h.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(6)).await;
            registry.set(Some(row(PrepareStatus::Ready, 1, None)));
        });
        let r = req();
        let (a, b, c) = tokio::join!(
            h.prepare.ensure_prepared(&r, WAIT),
            h.prepare.ensure_prepared(&r, WAIT),
            h.prepare.ensure_prepared(&r, WAIT),
        );
        for out in [a, b, c] {
            assert!(matches!(out.unwrap(), EnsureOutcome::Ready(_)));
        }
        assert_eq!(requests(&h), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_surfaces_a_trigger_failure_and_allows_a_retry() {
        let h = harness_with(
            true,
            None,
            FakeTrigger {
                fail: true,
                ..Default::default()
            },
        );
        let err = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap_err();
        assert!(err.to_string().contains("queue down"), "{err}");
        // The failed request is not remembered as "already requested".
        let err_again = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap_err();
        assert!(err_again.to_string().contains("queue down"));
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_dropped_call_does_not_leak_the_in_flight_key() {
        let h = harness(true, None);
        *h.progress.block.lock().unwrap() = true;
        let prepare = h.prepare.clone();
        let task = tokio::spawn(async move { prepare.ensure_prepared(&req(), WAIT).await });
        // Let it reach the blocked progress read, then drop it.
        tokio::time::sleep(Duration::from_millis(10)).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        // The next call must still be able to send the request.
        *h.progress.block.lock().unwrap() = false;
        let registry = h.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            registry.set(Some(row(PrepareStatus::Ready, 1, None)));
        });
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(requests(&h), 1, "the aborted call left no stale key behind");
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_the_wait_bound_covers_a_hung_progress_port() {
        let h = harness(true, Some(row(PrepareStatus::Running, 1, Some(3600))));
        *h.progress.block.lock().unwrap() = true;
        let r = req();
        let call = h.prepare.ensure_prepared(&r, Duration::from_secs(5));
        let out = tokio::time::timeout(Duration::from_secs(60), call)
            .await
            .expect("ensure_prepared must return within its bound")
            .unwrap();
        assert_eq!(out, EnsureOutcome::StillPreparing { progress: None });
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_an_enormous_wait_does_not_overflow() {
        let h = harness(true, Some(row(PrepareStatus::Ready, 1, None)));
        let out = h
            .prepare
            .ensure_prepared(&req(), Duration::MAX)
            .await
            .unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)));
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_ready_table_is_returned_even_with_a_zero_wait() {
        let h = harness(true, Some(row(PrepareStatus::Ready, 1, None)));
        let out = h
            .prepare
            .ensure_prepared(&req(), Duration::ZERO)
            .await
            .unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_zero_wait_still_sends_the_request_when_there_is_no_job() {
        let h = harness(true, None);
        let out = h
            .prepare
            .ensure_prepared(&req(), Duration::ZERO)
            .await
            .unwrap();
        assert!(
            matches!(out, EnsureOutcome::StillPreparing { .. }),
            "got {out:?}"
        );
        assert_eq!(requests(&h), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_notes_the_use_of_a_ready_table_only() {
        let ready = harness(true, Some(row(PrepareStatus::Ready, 1, None)));
        ready.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert_eq!(ready.registry.touches.load(Ordering::SeqCst), 1);

        for existing in [
            Some(row(PrepareStatus::Running, 1, Some(300))),
            Some(row(PrepareStatus::Failed, MAX_ATTEMPTS, None)),
        ] {
            let h = harness(true, existing);
            h.prepare
                .ensure_prepared(&req(), Duration::from_secs(2))
                .await
                .unwrap();
            assert_eq!(h.registry.touches.load(Ordering::SeqCst), 0);
        }
        let off = harness(false, Some(row(PrepareStatus::Ready, 1, None)));
        off.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert_eq!(off.registry.touches.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_failed_use_note_never_fails_the_call() {
        let h = harness(true, Some(row(PrepareStatus::Ready, 1, None)));
        *h.registry.fail_touch.lock().unwrap() = true;
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(h.registry.touches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_hung_use_note_does_not_hold_the_ready_answer() {
        let h = harness(true, Some(row(PrepareStatus::Ready, 1, None)));
        *h.registry.hang_touch.lock().unwrap() = true;
        let r = req();
        let call = h.prepare.ensure_prepared(&r, WAIT);
        let out = tokio::time::timeout(Duration::from_secs(60), call)
            .await
            .expect("recording a use must not outlive its short bound")
            .unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_row_taken_by_cleanup_between_read_and_touch_is_not_handed_out(
    ) {
        let h = harness(true, Some(row(PrepareStatus::Ready, 1, None)));
        // The cleanup pass claims the row right after we read it: our touch
        // changes nothing and the row is now `deleting` with a live lease.
        *h.registry.touch_answer.lock().unwrap() = Some(false);
        *h.registry.after_touch.lock().unwrap() =
            Some(Some(row(PrepareStatus::Deleting, 1, Some(600))));
        let out = h
            .prepare
            .ensure_prepared(&req(), Duration::from_secs(5))
            .await
            .unwrap();
        assert!(
            matches!(out, EnsureOutcome::StillPreparing { .. }),
            "a table being deleted must not be handed out, got {out:?}"
        );
        assert_eq!(requests(&h), 0, "a live cleanup lease is waited on");
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_throttled_touch_still_hands_the_ready_table_out() {
        // touch answers false because the use was already recorded today; the
        // row is still ready, so it is handed out.
        let h = harness(true, Some(row(PrepareStatus::Ready, 1, None)));
        *h.registry.touch_answer.lock().unwrap() = Some(false);
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(h.registry.touches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_row_deleted_between_read_and_touch_is_requested_again() {
        let h = harness(true, Some(row(PrepareStatus::Ready, 1, None)));
        *h.registry.touch_answer.lock().unwrap() = Some(false);
        *h.registry.after_touch.lock().unwrap() = Some(None); // cleanup finished
        let out = h
            .prepare
            .ensure_prepared(&req(), Duration::from_secs(3))
            .await
            .unwrap();
        assert!(
            matches!(out, EnsureOutcome::StillPreparing { .. }),
            "got {out:?}"
        );
        assert_eq!(requests(&h), 1, "the normal path: no row means request");
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_use_recorded_recently_costs_neither_a_touch_nor_a_second_read(
    ) {
        // The normal case: the use was already recorded today. Decided from the
        // row already read: no write attempt and no extra registry read.
        let mut recent = row(PrepareStatus::Ready, 1, None);
        recent.last_used_at = Some(now() - ChronoDuration::hours(1));
        let h = harness(true, Some(recent));
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(h.registry.touches.load(Ordering::SeqCst), 0);
        assert_eq!(h.registry.gets.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_use_older_than_the_interval_is_recorded_again() {
        let mut stale = row(PrepareStatus::Ready, 1, None);
        stale.last_used_at = Some(now() - ChronoDuration::days(2));
        let h = harness(true, Some(stale));
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(h.registry.touches.load(Ordering::SeqCst), 1);
        assert_eq!(
            h.registry.gets.load(Ordering::SeqCst),
            1,
            "the touch worked: no re-read"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_a_failed_re_read_after_an_unexpected_touch_is_best_effort() {
        // The touch was due and changed nothing, so the row is re-read; that
        // read fails. Like a failed touch, it must not fail the call.
        let h = harness(true, Some(row(PrepareStatus::Ready, 1, None)));
        *h.registry.touch_answer.lock().unwrap() = Some(false);
        *h.registry.fail_get_on.lock().unwrap() = Some(2);
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
    }
}
