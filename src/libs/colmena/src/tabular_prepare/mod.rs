//! Large tabular preparation (dark behind `COLMENA_LARGE_TABULAR`).
//!
//! A large CSV/Excel attachment is prepared once into typed columnar tables,
//! tracked in a registry keyed by the source `storage_key`. This module holds
//! the registry ([`registry`], with SQLite and Postgres implementations), the
//! ports the host replaces ([`ports`]) and the `ensure_prepared` entry a tool
//! calls before it needs the tables ([`TabularPrepare::ensure_prepared`]); the
//! cleanup pass `attachment_gc` runs will follow. Nothing here converts a file
//! yet and nothing calls it.
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
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::time::Instant;

pub mod ports;
pub mod postgres_registry;
pub mod registry;
pub mod sqlite_registry;

#[cfg(test)]
mod registry_contract;

/// How often a waiting caller re-reads the registry.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

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
    poll_interval: Duration,
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

impl TabularPrepare {
    pub fn new(config: PrepareConfig, registry: Arc<dyn PreparationRegistry>) -> Self {
        Self {
            config,
            registry,
            clock: Arc::new(Utc::now),
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
    /// - `ready`: `Ready`.
    /// - `failed` with attempts left, or an expired lease: request once, then wait.
    /// - Attempts exhausted (three failures, or a job that died three times
    ///   without reporting): `Failed` with `final_failure`, never requested.
    /// - A live job holds it: attach and wait, no second trigger.
    /// - Nobody holds it: request once, then wait.
    /// - The wait ends first: `StillPreparing`, never an error.
    pub async fn ensure_prepared(
        &self,
        req: &PrepareRequest,
        wait: Duration,
    ) -> Result<EnsureOutcome, PrepareError> {
        if !self.config.large_tabular {
            return Ok(EnsureOutcome::NotEnabled);
        }
        let deadline = Instant::now() + wait;
        let mut baseline: Option<Option<i32>> = None;
        let mut requested = false;
        loop {
            let row = self.registry.get(&req.source_key).await?;
            match self.observe(row, baseline) {
                Observed::Ready(row) => return Ok(EnsureOutcome::Ready(Box::new(row))),
                Observed::FinalFailure(row) => return Ok(failed(&row, true)),
                Observed::NewFailure(row) => return Ok(failed(&row, false)),
                Observed::DeadJob(row) => return Ok(dead_job(&row)),
                Observed::Running => {}
                Observed::Claimable {
                    has_row,
                    failed_attempts,
                } => {
                    baseline.get_or_insert(failed_attempts);
                    if !requested && self.should_request(req, has_row).await {
                        self.config.trigger.request(req.clone()).await?;
                        requested = true;
                    }
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let progress = self.config.progress.read(&req.source_key).await;
                return Ok(EnsureOutcome::StillPreparing { progress });
            }
            tokio::time::sleep(self.poll_interval.min(left)).await;
        }
    }

    /// Whether to send the request: always when a row exists that nobody
    /// holds; with no row, unless the host's progress port says the item is
    /// already queued or started.
    async fn should_request(&self, req: &PrepareRequest, has_row: bool) -> bool {
        if has_row {
            return true;
        }
        !matches!(
            self.config.progress.read(&req.source_key).await,
            Some(PrepareProgressInfo {
                state: ProgressState::Queued | ProgressState::Running,
                ..
            })
        )
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
    }

    impl FakeRegistry {
        fn new(row: Option<PreparedRow>) -> Arc<Self> {
            Arc::new(Self {
                current: Mutex::new(row),
                gets: AtomicUsize::new(0),
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
            self.gets.fetch_add(1, Ordering::SeqCst);
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
    }

    #[derive(Default)]
    struct FakeTrigger {
        requests: Mutex<Vec<PrepareRequest>>,
    }

    #[async_trait]
    impl PrepareTrigger for FakeTrigger {
        async fn request(&self, req: PrepareRequest) -> Result<(), PrepareTriggerError> {
            self.requests.lock().unwrap().push(req);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeProgress {
        reads: AtomicUsize,
        value: Mutex<Option<PrepareProgressInfo>>,
    }

    #[async_trait]
    impl PrepareProgress for FakeProgress {
        async fn report(&self, _k: &str, _i: PrepareProgressInfo) {}
        async fn read(&self, _k: &str) -> Option<PrepareProgressInfo> {
            self.reads.fetch_add(1, Ordering::SeqCst);
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
        let registry = FakeRegistry::new(row);
        let trigger = Arc::new(FakeTrigger::default());
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
}
