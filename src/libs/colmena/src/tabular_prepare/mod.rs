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
    PreparationRegistry, PrepareStatus, PreparedRow, RegistryError,
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
        let mut requested = false;
        loop {
            let row = self.registry.get(&req.source_key).await?;
            match row {
                Some(row) if row.status == PrepareStatus::Ready => {
                    return Ok(EnsureOutcome::Ready(Box::new(row)));
                }
                Some(row) if self.holds_a_live_lease(&row) => {}
                other => {
                    if !requested && self.should_request(req, other.is_some()).await {
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

    fn holds_a_live_lease(&self, row: &PreparedRow) -> bool {
        row.status == PrepareStatus::Running
            && row.lease_until.is_some_and(|until| until >= (self.clock)())
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

    fn row(status: PrepareStatus, lease_secs: Option<i64>) -> PreparedRow {
        PreparedRow {
            source_storage_key: "k".to_string(),
            status,
            format_version: FORMAT_VERSION,
            manifest_key: None,
            blob_keys: vec![],
            tables_json: None,
            source_bytes: 60_000_000,
            prepared_bytes: None,
            error_code: None,
            error_detail: None,
            lease_owner: lease_secs.map(|_| "job".to_string()),
            lease_until: lease_secs.map(|s| now() + ChronoDuration::seconds(s)),
            attempts: 1,
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
        let h = harness(false, Some(row(PrepareStatus::Failed, None)));
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert_eq!(out, EnsureOutcome::NotEnabled);
        assert_eq!(h.registry.gets.load(Ordering::SeqCst), 0);
        assert_eq!(requests(&h), 0);
        assert_eq!(h.progress.reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_returns_a_ready_row_without_a_trigger() {
        let ready = row(PrepareStatus::Ready, None);
        let h = harness(true, Some(ready.clone()));
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert_eq!(out, EnsureOutcome::Ready(Box::new(ready)));
        assert_eq!(requests(&h), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_attaches_to_a_running_job_without_a_second_trigger() {
        let h = harness(true, Some(row(PrepareStatus::Running, Some(300))));
        let registry = h.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            registry.set(Some(row(PrepareStatus::Ready, None)));
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
            registry.set(Some(row(PrepareStatus::Running, Some(300))));
            tokio::time::sleep(Duration::from_secs(4)).await;
            registry.set(Some(row(PrepareStatus::Ready, None)));
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
            registry.set(Some(row(PrepareStatus::Ready, None)));
        });
        let out = h.prepare.ensure_prepared(&req(), WAIT).await.unwrap();
        assert!(matches!(out, EnsureOutcome::Ready(_)), "got {out:?}");
        assert_eq!(requests(&h), 0, "already queued: no second request");
    }

    #[tokio::test(start_paused = true)]
    async fn tabular_prepare_ensure_wait_expires_as_still_preparing_not_an_error() {
        let h = harness(true, Some(row(PrepareStatus::Running, Some(3600))));
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
}
