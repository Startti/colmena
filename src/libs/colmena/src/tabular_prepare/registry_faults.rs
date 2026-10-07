//! A registry wrapper for tests that makes chosen writes fail or never return,
//! so the driver's handling of a registry that misbehaves at the end of a run is
//! tested against the real backends underneath.

use super::registry::*;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::Arc;

/// A virtual clock for the proof that a job ends before its lease does: every
/// bounded step that passes through it takes (almost) its full bound, and each
/// step is logged with the time at which it took effect.
pub(crate) struct Slow {
    now: std::sync::Mutex<DateTime<Utc>>,
    pub log: std::sync::Mutex<Vec<(&'static str, DateTime<Utc>)>>,
    pub lease_until: std::sync::Mutex<Option<DateTime<Utc>>>,
    inside_budget: AtomicBool,
}

impl Slow {
    pub fn new(start: DateTime<Utc>) -> Arc<Self> {
        Arc::new(Self {
            now: std::sync::Mutex::new(start),
            log: std::sync::Mutex::new(Vec::new()),
            lease_until: std::sync::Mutex::new(None),
            inside_budget: AtomicBool::new(false),
        })
    }
    pub fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap()
    }
    pub fn advance(&self, by: std::time::Duration) {
        *self.now.lock().unwrap() += Duration::from_std(by).unwrap();
    }
    /// A step that takes its whole bound (less a millisecond), then takes effect.
    /// The conversion runs inside the budget, which is modelled as one block of time
    /// (see `leave_budget`): the steps inside it neither advance the clock nor count.
    pub fn enter_budget(&self) {
        self.inside_budget.store(true, SeqCst);
    }
    /// The budget is over: it took its whole length.
    pub fn leave_budget(&self, length: std::time::Duration) {
        self.advance(length);
        self.inside_budget.store(false, SeqCst);
    }
    pub fn pass(&self, op: &'static str) {
        if self.inside_budget.load(SeqCst) {
            return;
        }
        self.advance(
            crate::tabular_prepare::driver::TERMINAL_STEP - std::time::Duration::from_millis(1),
        );
        self.log.lock().unwrap().push((op, self.now()));
    }
}

#[derive(Default)]
pub(crate) struct Faults {
    pub slow: std::sync::Mutex<Option<Arc<Slow>>>,
    /// `complete` answers a backend error whose text must never be logged.
    pub fail_complete: AtomicBool,
    /// `complete` never returns.
    pub hang_complete: AtomicBool,
    /// Says when `complete` has been reached (and is about to hang).
    pub complete_reached: tokio::sync::Notify,
    /// `fail_with_blobs` answers a backend error.
    pub fail_fail: AtomicBool,
    /// `fail_with_blobs` never returns, after saying it was reached.
    pub hang_fail: AtomicBool,
    pub fail_reached: tokio::sync::Notify,
    /// Runs right after `fail_with_blobs` was applied (a retry gets in here).
    pub after_fail: std::sync::Mutex<Option<AfterFail>>,
    /// The row is deleted just before the first tracking write.
    pub delete_before_track: AtomicBool,
    /// `release` never returns, after saying it was reached.
    pub hang_release: AtomicBool,
    pub release_reached: tokio::sync::Notify,
}

pub(crate) type AfterFail =
    Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

pub(crate) struct FaultyRegistry {
    pub inner: Arc<dyn PreparationRegistry>,
    pub faults: Faults,
}

impl FaultyRegistry {
    pub fn new(inner: Arc<dyn PreparationRegistry>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            faults: Faults::default(),
        })
    }
}

pub(crate) const SECRET: &str = "secret-registry-text-chat-attachments/u/s/x.csv";

#[async_trait]
impl PreparationRegistry for FaultyRegistry {
    async fn claim(&self, r: ClaimRequest) -> Result<Option<Claim>, RegistryError> {
        let slow = self.faults.slow.lock().unwrap().clone();
        if let Some(s) = &slow {
            s.pass("claim");
        }
        let key = r.source_key.clone();
        let out = self.inner.claim(r).await?;
        if let Some(s) = slow {
            *s.lease_until.lock().unwrap() =
                self.inner.get(&key).await?.and_then(|row| row.lease_until);
        }
        Ok(out)
    }
    async fn complete(
        &self,
        k: &str,
        o: &str,
        i: ReadyInfo,
        n: DateTime<Utc>,
    ) -> Result<TerminalOutcome, RegistryError> {
        if let Some(s) = self.faults.slow.lock().unwrap().clone() {
            s.pass("complete");
        }
        if self.faults.hang_complete.load(SeqCst) {
            self.faults.complete_reached.notify_one();
            futures::future::pending::<()>().await;
        }
        if self.faults.fail_complete.load(SeqCst) {
            return Err(RegistryError::Backend(SECRET.into()));
        }
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
        if let Some(s) = self.faults.slow.lock().unwrap().clone() {
            s.pass("fail_with_blobs");
        }
        if self.faults.hang_fail.load(SeqCst) {
            self.faults.fail_reached.notify_one();
            futures::future::pending::<()>().await;
        }
        if self.faults.fail_fail.load(SeqCst) {
            return Err(RegistryError::Backend(SECRET.into()));
        }
        let out = self.inner.fail_with_blobs(k, o, c, d, b, n).await?;
        let hook = self.faults.after_fail.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook().await;
        }
        Ok(out)
    }
    async fn track_blobs(
        &self,
        k: &str,
        o: &str,
        b: &[String],
        n: DateTime<Utc>,
    ) -> Result<TerminalOutcome, RegistryError> {
        if self.faults.delete_before_track.swap(false, SeqCst) {
            self.inner.delete(k).await?;
        }
        if let Some(s) = self.faults.slow.lock().unwrap().clone() {
            s.enter_budget();
        }
        self.inner.track_blobs(k, o, b, n).await
    }
    async fn still_owned(&self, k: &str, o: &str) -> Result<bool, RegistryError> {
        if let Some(s) = self.faults.slow.lock().unwrap().clone() {
            s.pass("still_owned");
        }
        self.inner.still_owned(k, o).await
    }
    async fn release(&self, k: &str, o: &str) -> Result<bool, RegistryError> {
        if let Some(s) = self.faults.slow.lock().unwrap().clone() {
            s.pass("release");
        }
        if self.faults.hang_release.load(SeqCst) {
            self.faults.release_reached.notify_one();
            futures::future::pending::<()>().await;
        }
        self.inner.release(k, o).await
    }
    async fn delete(&self, k: &str) -> Result<bool, RegistryError> {
        self.inner.delete(k).await
    }
    async fn get(&self, k: &str) -> Result<Option<PreparedRow>, RegistryError> {
        self.inner.get(k).await
    }
    async fn touch_last_used(
        &self,
        k: &str,
        n: DateTime<Utc>,
        i: Duration,
    ) -> Result<bool, RegistryError> {
        self.inner.touch_last_used(k, n, i).await
    }
    async fn find_stale(
        &self,
        c: DateTime<Utc>,
        n: DateTime<Utc>,
        a: Option<&str>,
        l: u32,
    ) -> Result<Vec<PreparedRow>, RegistryError> {
        self.inner.find_stale(c, n, a, l).await
    }
    async fn list_ready_after(
        &self,
        a: Option<&str>,
        l: u32,
    ) -> Result<Vec<PreparedRow>, RegistryError> {
        self.inner.list_ready_after(a, l).await
    }
    async fn begin_delete(
        &self,
        r: &PreparedRow,
        o: &str,
        l: Duration,
        n: DateTime<Utc>,
    ) -> Result<bool, RegistryError> {
        self.inner.begin_delete(r, o, l, n).await
    }
    async fn finish_delete(&self, k: &str, o: &str) -> Result<bool, RegistryError> {
        self.inner.finish_delete(k, o).await
    }
    async fn delete_if_unchanged(&self, r: &PreparedRow) -> Result<bool, RegistryError> {
        self.inner.delete_if_unchanged(r).await
    }
    async fn mark_manifest_missing(
        &self,
        r: &PreparedRow,
        n: DateTime<Utc>,
    ) -> Result<bool, RegistryError> {
        self.inner.mark_manifest_missing(r, n).await
    }
}
