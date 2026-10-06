//! A registry wrapper for tests that makes chosen writes fail or never return,
//! so the driver's handling of a registry that misbehaves at the end of a run is
//! tested against the real backends underneath.

use super::registry::*;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct Faults {
    /// `complete` answers a backend error whose text must never be logged.
    pub fail_complete: AtomicBool,
    /// `complete` never returns.
    pub hang_complete: AtomicBool,
    /// `fail_with_blobs` answers a backend error.
    pub fail_fail: AtomicBool,
    /// `fail_with_blobs` never returns.
    pub hang_fail: AtomicBool,
    /// The row is deleted just before the first tracking write.
    pub delete_before_track: AtomicBool,
    /// `release` never returns.
    pub hang_release: AtomicBool,
}

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
        self.inner.claim(r).await
    }
    async fn complete(
        &self,
        k: &str,
        o: &str,
        i: ReadyInfo,
        n: DateTime<Utc>,
    ) -> Result<TerminalOutcome, RegistryError> {
        if self.faults.hang_complete.load(SeqCst) {
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
        if self.faults.hang_fail.load(SeqCst) {
            futures::future::pending::<()>().await;
        }
        if self.faults.fail_fail.load(SeqCst) {
            return Err(RegistryError::Backend(SECRET.into()));
        }
        self.inner.fail_with_blobs(k, o, c, d, b, n).await
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
        self.inner.track_blobs(k, o, b, n).await
    }
    async fn still_owned(&self, k: &str, o: &str) -> Result<bool, RegistryError> {
        self.inner.still_owned(k, o).await
    }
    async fn release(&self, k: &str, o: &str) -> Result<bool, RegistryError> {
        if self.faults.hang_release.load(SeqCst) {
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
