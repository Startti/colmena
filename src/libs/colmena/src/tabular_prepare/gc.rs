//! Cleanup of prepared tables, run by the `attachment_gc` binary next to its
//! own pass over `conversation_attachments`.
//!
//! Idempotent steps: delete the prepared tables of a source whose blob was
//! just deleted (this slice); later slices add the cancellation of a live
//! preparation, the TTL pass and the pass that makes a `ready` row whose
//! manifest has disappeared claimable again.
//!
//! A row is never deleted from under a live preparation: the pass first
//! CLAIMS it by moving it to `deleting` under its own lease, conditional on
//! the status, `updated_at` and `last_used_at` it read and on the row holding
//! no live lease (`begin_delete`). Only then does it delete the blobs
//! (`OutputStorageRepository::delete_derived`) and, last, the row. A
//! `deleting` row with a live lease is not claimable by a preparation; if the
//! pass dies or storage refuses, the lease expires and the next run (or a
//! preparation) takes the row over.
//!
//! Time is read from a clock once per row, never once per pass: each row's
//! lease starts when the pass claims it, so a long pass cannot leave a late
//! row with a lease that is already half spent. If a lease nevertheless
//! expires while the blobs are being deleted and something claims the row,
//! `finish_delete` returns false: the pass logs it and counts `leases_lost`
//! and leaves the new owner's row alone.
//!
//! Containment. Before it deletes anything the pass checks every tracked key
//! (`contain_keys`): never the source key itself, never a key with a `..`
//! segment, and only keys inside the root the host's storage adapter reports
//! with `derived_root` (compared on a path-segment boundary, so
//! `u/s/prepared-other/x` is not inside `u/s/prepared`). A host whose adapter
//! reports no root cannot have its keys contained, so the pass REFUSES to
//! delete them: it leaves the row, logs an error and counts `keys_rejected`.
//! A row that tracks no key has nothing to contain and is processed normally.
//! With an empty table none of this makes a storage call.

use crate::storage::domain::OutputStorageRepository;
use crate::tabular_prepare::registry::{PreparationRegistry, PreparedRow, RegistryError};
use chrono::{DateTime, Duration, Utc};

/// Source of "now" for the passes. Read once per row.
pub type Clock = dyn Fn() -> DateTime<Utc> + Send + Sync;

/// How long a cleanup pass holds a row it is deleting before another run may
/// take it over.
const GC_LEASE: Duration = Duration::minutes(10);

/// What the prepared-tables cleanup did in one run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PreparedGcSummary {
    pub rows_deleted: u64,
    pub blobs_deleted: u64,
    /// `ready` rows demoted because their manifest is gone.
    pub rows_reset: u64,
    pub storage_errors: u64,
    /// Sources the pass could not settle: the row had changed or was leased by
    /// someone else (another pass, or a preparation). Nothing was wrongly
    /// deleted; the caller must retry on its next run.
    pub busy: u64,
    /// Rows left alone because a tracked key could not be contained: it is
    /// the source, has a `..` segment, lies outside the derived root, or the
    /// host reports no derived root.
    pub keys_rejected: u64,
    /// Rows whose `deleting` lease expired and was taken by someone else
    /// before the pass could finish them.
    pub leases_lost: u64,
}

impl PreparedGcSummary {
    /// Whether the run did or hit anything worth a log line.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether the prepared tables of a source were NOT fully cleaned up and the
    /// caller must retry on its next run: a blob could not be deleted
    /// (`storage_errors`) or the row could not be settled (`busy`).
    pub fn is_incomplete(&self) -> bool {
        self.storage_errors > 0 || self.busy > 0
    }

    /// Add another run's counts to this one.
    pub fn merge(&mut self, other: PreparedGcSummary) {
        self.rows_deleted += other.rows_deleted;
        self.blobs_deleted += other.blobs_deleted;
        self.rows_reset += other.rows_reset;
        self.storage_errors += other.storage_errors;
        self.busy += other.busy;
        self.keys_rejected += other.keys_rejected;
        self.leases_lost += other.leases_lost;
    }
}

/// The keys a row tracks, manifest last.
fn tracked_keys(row: &PreparedRow) -> Vec<String> {
    let manifest = row.manifest_key.as_deref();
    let mut keys: Vec<String> = row
        .blob_keys
        .iter()
        .filter(|k| Some(k.as_str()) != manifest)
        .cloned()
        .collect();
    keys.extend(row.manifest_key.clone());
    keys
}

/// Why a tracked key cannot be deleted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub key: String,
    pub reason: &'static str,
}

/// The one validation every deletion path shares (claim-and-delete and the
/// cancellation of a live preparation). `Ok` when `keys` is empty or every key
/// is contained.
pub(crate) fn contain_keys(
    storage: &dyn OutputStorageRepository,
    source: &str,
    keys: &[String],
) -> Result<(), Refusal> {
    if keys.is_empty() {
        return Ok(());
    }
    let refuse = |key: &String, reason| Refusal {
        key: key.clone(),
        reason,
    };
    if let Some(k) = keys.iter().find(|k| *k == source) {
        return Err(refuse(k, "the key is the source itself"));
    }
    if let Some(k) = keys.iter().find(|k| k.split('/').any(|seg| seg == "..")) {
        return Err(refuse(k, "the key has a `..` segment"));
    }
    let Some(root) = storage.derived_root(source) else {
        return Err(refuse(
            &keys[0],
            "the storage adapter reports no derived root, so the key cannot be contained",
        ));
    };
    let prefix = format!("{}/", root.trim_end_matches('/'));
    match keys.iter().find(|k| !k.starts_with(&prefix)) {
        Some(k) => Err(refuse(k, "the key lies outside the derived root")),
        None => Ok(()),
    }
}

fn log_refusal(source: &str, refusal: &Refusal) {
    tracing::error!(
        target: "colmena::attachment_gc",
        event = "gc.prepared.key_rejected",
        source_key = source,
        blob = refusal.key.as_str(),
        reason = refusal.reason,
        "tracked key refused; the row is left untouched"
    );
}

/// Take a row, delete its derived blobs and then the row. A row that changed
/// since it was read (a preparation claimed it) is left alone. Returns `false`
/// only in that case, so a caller that re-reads can try again.
async fn remove_prepared(
    registry: &dyn PreparationRegistry,
    storage: &dyn OutputStorageRepository,
    row: &PreparedRow,
    clock: &Clock,
    dry_run: bool,
    summary: &mut PreparedGcSummary,
) -> Result<bool, RegistryError> {
    let source = row.source_storage_key.as_str();
    let keys = tracked_keys(row);
    // Validate first, dry run included: a dry run must not promise deletions
    // the real run would refuse.
    if let Err(refusal) = contain_keys(storage, source, &keys) {
        summary.keys_rejected += 1;
        log_refusal(source, &refusal);
        return Ok(true);
    }
    if dry_run {
        tracing::info!(
            target: "colmena::attachment_gc",
            event = "gc.prepared.dry_run.would_delete",
            source_key = source,
            blobs = keys.len(),
            "[dry-run] would delete prepared tables"
        );
        return Ok(true);
    }
    let owner = format!("gc-{}", uuid::Uuid::new_v4());
    if !registry
        .begin_delete(row, &owner, GC_LEASE, clock())
        .await?
    {
        tracing::info!(
            target: "colmena::attachment_gc",
            event = "gc.prepared.row_changed",
            source_key = source,
            "row changed since it was read (or holds a live lease); skipping"
        );
        return Ok(false);
    }
    // An adapter may read an empty list as "delete everything under the prefix": with
    // nothing tracked, nothing is asked of it.
    if !keys.is_empty() {
        if let Err(e) = storage.delete_derived(source, &keys).await {
            summary.storage_errors += 1;
            tracing::warn!(
                target: "colmena::attachment_gc",
                event = "gc.prepared.storage_delete_failed",
                source_key = source,
                error = %e,
                "prepared blob delete failed; the row stays `deleting` and a later run retries"
            );
            return Ok(true);
        }
    }
    summary.blobs_deleted += keys.len() as u64;
    if registry.finish_delete(source, &owner).await? {
        summary.rows_deleted += 1;
    } else {
        summary.leases_lost += 1;
        tracing::warn!(
            target: "colmena::attachment_gc",
            event = "gc.prepared.lease_lost",
            source_key = source,
            "the cleanup lease expired and the row was taken over before it could be finished; leaving it"
        );
    }
    Ok(true)
}

/// A source blob was just deleted: delete its prepared tables and row.
pub async fn delete_prepared_for_source(
    registry: &dyn PreparationRegistry,
    storage: &dyn OutputStorageRepository,
    source_key: &str,
    clock: &Clock,
    dry_run: bool,
) -> Result<PreparedGcSummary, RegistryError> {
    let mut summary = PreparedGcSummary::default();
    if let Some(row) = registry.get(source_key).await? {
        if !remove_prepared(registry, storage, &row, clock, dry_run, &mut summary).await? {
            // The row changed or is leased by someone else: not settled.
            summary.busy += 1;
            tracing::warn!(
                target: "colmena::attachment_gc",
                event = "gc.prepared.busy",
                source_key,
                "the prepared row changed or is leased by someone else; retry next run"
            );
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::domain::{
        OutputStorageRepository, StorageError, StoreRequest, StoredBytes, StoredOutput,
        StoredStream,
    };
    use crate::tabular_prepare::registry::*;
    use crate::tabular_prepare::sqlite_registry::SqlitePreparationRegistry;
    use async_trait::async_trait;
    use chrono::TimeZone;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::{Arc, Mutex};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()
    }

    /// In-memory storage that remembers what was deleted and in which order,
    /// and can refuse to delete or to find chosen keys. Its layout puts a
    /// source's blobs under `<parent>/prepared/`.
    #[derive(Default)]
    struct FakeStorage {
        blobs: Mutex<HashSet<String>>,
        deleted: Mutex<Vec<String>>,
        failing: Mutex<HashSet<String>>,
        unreachable: Mutex<bool>,
        reads: Mutex<usize>,
        no_root: Mutex<bool>,
        /// Report the root without a trailing slash (a sibling-prefix trap).
        bare_root: Mutex<bool>,
        /// Runs before `delete_derived` deletes anything.
        hook: Mutex<Option<Hook>>,
        /// Times `delete_derived` was asked, whatever the list.
        derived_calls: Mutex<usize>,
    }

    type Hook = Box<dyn Fn() -> futures::future::BoxFuture<'static, ()> + Send + Sync>;

    impl FakeStorage {
        fn with(keys: &[String]) -> Arc<Self> {
            let s = Self::default();
            s.blobs.lock().unwrap().extend(keys.iter().cloned());
            Arc::new(s)
        }
        fn calls(&self) -> usize {
            self.deleted.lock().unwrap().len() + *self.reads.lock().unwrap()
        }
        fn refuse(&self, key: &str) {
            self.failing.lock().unwrap().insert(key.to_string());
        }
    }

    #[async_trait]
    impl OutputStorageRepository for FakeStorage {
        async fn store(&self, _r: StoreRequest) -> Result<StoredOutput, StorageError> {
            unimplemented!()
        }
        async fn read(&self, _k: &str) -> Result<StoredBytes, StorageError> {
            unimplemented!()
        }
        async fn read_stream(&self, key: &str) -> Result<StoredStream, StorageError> {
            *self.reads.lock().unwrap() += 1;
            if *self.unreachable.lock().unwrap() {
                return Err(StorageError::BackendUnavailable("down".into()));
            }
            if !self.blobs.lock().unwrap().contains(key) {
                return Err(StorageError::InvalidInput(format!("unknown key {key}")));
            }
            Ok(StoredStream {
                stream: Box::pin(futures::stream::empty()),
                size_bytes: 0,
                mime_type: String::new(),
                filename: String::new(),
            })
        }
        async fn delete(&self, key: &str) -> Result<(), StorageError> {
            if self.failing.lock().unwrap().contains(key) {
                return Err(StorageError::BackendUnavailable("refused".into()));
            }
            self.blobs.lock().unwrap().remove(key);
            self.deleted.lock().unwrap().push(key.to_string());
            Ok(())
        }
        fn derived_root(&self, source: &str) -> Option<String> {
            if *self.no_root.lock().unwrap() {
                return None;
            }
            let bare = *self.bare_root.lock().unwrap();
            source.rsplit_once('/').map(|(parent, _)| {
                if bare {
                    format!("{parent}/prepared")
                } else {
                    format!("{parent}/prepared/")
                }
            })
        }
        async fn delete_derived(
            &self,
            _source: &str,
            tracked_keys: &[String],
        ) -> Result<(), StorageError> {
            *self.derived_calls.lock().unwrap() += 1;
            let hook = self.hook.lock().unwrap().take();
            if let Some(hook) = hook {
                hook().await;
            }
            for key in tracked_keys {
                self.delete(key).await?;
            }
            Ok(())
        }
    }

    struct Fixture {
        registry: Arc<SqlitePreparationRegistry>,
        _dir: tempfile::TempDir,
    }

    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(dir.path().join("gc.db"))
            .create_if_missing(true);
        let pool = Arc::new(
            SqlitePoolOptions::new()
                .max_connections(2)
                .connect_with(options)
                .await
                .unwrap(),
        );
        sqlx::migrate!("migrations/sqlite")
            .run(&*pool)
            .await
            .unwrap();
        Fixture {
            registry: Arc::new(SqlitePreparationRegistry::from_pool(pool)),
            _dir: dir,
        }
    }

    fn source_of(name: &str) -> String {
        format!("chat-attachments/u/s/{name}.csv")
    }

    fn manifest_of(name: &str) -> String {
        format!("chat-attachments/u/s/prepared/{name}/manifest.json")
    }

    fn parts_of(name: &str) -> Vec<String> {
        (0..2)
            .map(|i| format!("chat-attachments/u/s/prepared/{name}/t0/part-0000{i}.parquet"))
            .collect()
    }

    fn all_blobs(name: &str) -> Vec<String> {
        let mut keys = parts_of(name);
        keys.push(manifest_of(name));
        keys
    }

    fn claim_at(name: &str, owner: &str, at: DateTime<Utc>) -> ClaimRequest {
        ClaimRequest {
            source_key: source_of(name),
            source_bytes: 60_000_000,
            format_version: FORMAT_VERSION,
            owner: owner.to_string(),
            lease: lease_for(Duration::seconds(300)),
            now: at,
        }
    }

    /// A `ready` row for the source `name`, created `age_days` ago.
    async fn seed_ready(f: &Fixture, name: &str, age_days: i64) {
        let at = now() - Duration::days(age_days);
        f.registry
            .claim(claim_at(name, "job", at))
            .await
            .unwrap()
            .unwrap();
        let info = ReadyInfo {
            manifest_key: manifest_of(name),
            blob_keys: parts_of(name),
            tables_json: "[]".to_string(),
            prepared_bytes: 1,
        };
        f.registry
            .complete(&source_of(name), "job", info, at)
            .await
            .unwrap();
    }

    fn storage_with(names: &[&str]) -> Arc<FakeStorage> {
        FakeStorage::with(&names.iter().flat_map(|n| all_blobs(n)).collect::<Vec<_>>())
    }

    async fn row(f: &Fixture, name: &str) -> Option<PreparedRow> {
        f.registry.get(&source_of(name)).await.unwrap()
    }

    /// A clock frozen at `now()` plus an offset.
    fn at(offset: Duration) -> impl Fn() -> DateTime<Utc> + Send + Sync {
        move || now() + offset
    }

    async fn gc_source(
        f: &Fixture,
        storage: &FakeStorage,
        name: &str,
        clock: &Clock,
        dry_run: bool,
    ) -> PreparedGcSummary {
        delete_prepared_for_source(&*f.registry, storage, &source_of(name), clock, dry_run)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn attachment_gc_deletes_every_prepared_blob_and_the_manifest_with_the_source() {
        let f = fixture().await;
        seed_ready(&f, "a", 0).await;
        let storage = storage_with(&["a"]);
        let summary = gc_source(&f, &storage, "a", &now, false).await;
        assert_eq!(
            summary,
            PreparedGcSummary {
                rows_deleted: 1,
                blobs_deleted: 3,
                ..Default::default()
            }
        );
        let deleted = storage.deleted.lock().unwrap().clone();
        assert_eq!(
            deleted.last().unwrap(),
            &manifest_of("a"),
            "manifest goes last"
        );
        assert!(storage.blobs.lock().unwrap().is_empty());
        assert_eq!(row(&f, "a").await, None);
    }

    #[tokio::test]
    async fn attachment_gc_a_source_without_a_prepared_row_costs_nothing() {
        let f = fixture().await;
        seed_ready(&f, "a", 0).await;
        let storage = storage_with(&["a"]);
        let summary = gc_source(&f, &storage, "other", &now, false).await;
        assert_eq!(summary, PreparedGcSummary::default());
        assert_eq!(storage.calls(), 0);
        assert!(row(&f, "a").await.is_some(), "other rows are untouched");
    }

    #[tokio::test]
    async fn attachment_gc_dry_run_of_the_source_path_changes_nothing() {
        let f = fixture().await;
        seed_ready(&f, "a", 0).await;
        let storage = storage_with(&["a"]);
        let s = gc_source(&f, &storage, "a", &now, true).await;
        assert!(s.is_empty());
        assert!(storage.deleted.lock().unwrap().is_empty());
        assert_eq!(row(&f, "a").await.unwrap().status, PrepareStatus::Ready);
    }

    /// A row held by another pass's live `deleting` lease cannot be taken: that is
    /// not a success, the caller must retry on its next run.
    #[tokio::test]
    async fn attachment_gc_a_row_leased_by_another_pass_is_counted_as_incomplete() {
        let f = fixture().await;
        seed_ready(&f, "a", 0).await;
        let observed = row(&f, "a").await.unwrap();
        assert!(f
            .registry
            .begin_delete(&observed, "gc-other", Duration::minutes(10), now())
            .await
            .unwrap());
        let storage = storage_with(&["a"]);
        let summary = gc_source(&f, &storage, "a", &now, false).await;
        assert_eq!(summary.busy, 1);
        assert!(summary.is_incomplete());
        assert_eq!(storage.calls(), 0, "nothing was deleted");
        // A finished pass is complete.
        seed_ready(&f, "b", 0).await;
        let ok = gc_source(&f, &storage_with(&["b"]), "b", &now, false).await;
        assert!(!ok.is_incomplete());
    }

    #[tokio::test]
    async fn attachment_gc_a_refused_delete_leaves_a_deleting_row_the_next_run_finishes() {
        let f = fixture().await;
        seed_ready(&f, "a", 0).await;
        let storage = storage_with(&["a"]);
        storage.refuse(&parts_of("a")[1]);
        let first = gc_source(&f, &storage, "a", &now, false).await;
        assert_eq!(first.storage_errors, 1);
        assert_eq!(first.rows_deleted, 0);
        assert_eq!(row(&f, "a").await.unwrap().status, PrepareStatus::Deleting);
        // While the lease is live another run leaves it alone.
        let busy = gc_source(&f, &storage, "a", &at(Duration::minutes(5)), false).await;
        assert_eq!(
            busy,
            PreparedGcSummary {
                busy: 1,
                ..Default::default()
            },
            "another pass holds it: not settled, so the caller retries"
        );

        storage.failing.lock().unwrap().clear();
        let second = gc_source(&f, &storage, "a", &at(Duration::minutes(11)), false).await;
        assert_eq!(second.rows_deleted, 1);
        assert!(storage.blobs.lock().unwrap().is_empty());
        assert_eq!(row(&f, "a").await, None);
    }

    #[tokio::test]
    async fn attachment_gc_a_persistent_delete_failure_leaves_a_row_a_preparation_can_take() {
        let f = fixture().await;
        seed_ready(&f, "a", 0).await;
        let storage = storage_with(&["a"]);
        storage.refuse(&parts_of("a")[0]);
        let first = gc_source(&f, &storage, "a", &now, false).await;
        assert_eq!(first.storage_errors, 1, "the failure is reported");
        assert_eq!(row(&f, "a").await.unwrap().status, PrepareStatus::Deleting);
        // The cleanup never comes back, but after its lease the source can be
        // prepared again instead of waiting for it.
        let later = now() + Duration::minutes(11);
        assert_eq!(
            f.registry.claim(claim_at("a", "job", later)).await.unwrap(),
            Some(Claim { attempts: 1 })
        );
    }

    #[tokio::test]
    async fn attachment_gc_a_lease_that_expires_mid_delete_is_counted_and_the_new_owner_is_left_alone(
    ) {
        let f = fixture().await;
        seed_ready(&f, "a", 0).await;
        let storage = storage_with(&["a"]);
        // While the blobs are being deleted the 10-minute lease runs out and a
        // preparation claims the row.
        let offset = Arc::new(AtomicI64::new(0));
        {
            let registry = f.registry.clone();
            let offset = offset.clone();
            *storage.hook.lock().unwrap() = Some(Box::new(move || {
                let registry = registry.clone();
                let offset = offset.clone();
                Box::pin(async move {
                    offset.store(11 * 60, Ordering::SeqCst);
                    let later = now() + Duration::minutes(11);
                    registry
                        .claim(claim_at("a", "worker", later))
                        .await
                        .unwrap()
                        .expect("the expired deleting lease is claimable");
                })
            }));
        }
        let clock = {
            let offset = offset.clone();
            move || now() + Duration::seconds(offset.load(Ordering::SeqCst))
        };
        let summary = gc_source(&f, &storage, "a", &clock, false).await;
        assert_eq!(
            summary.leases_lost, 1,
            "finish_delete returning false is visible"
        );
        assert_eq!(summary.rows_deleted, 0);
        let live = row(&f, "a").await.expect("the new owner's row survives");
        assert_eq!(live.status, PrepareStatus::Running);
        assert_eq!(live.lease_owner.as_deref(), Some("worker"));
    }

    /// Seed a ready row for `name` tracking exactly `keys` (plus the usual
    /// manifest key unless it is empty).
    async fn seed_tracking(f: &Fixture, name: &str, keys: Vec<String>, manifest: Option<String>) {
        f.registry
            .claim(claim_at(name, "job", now()))
            .await
            .unwrap()
            .unwrap();
        let info = ReadyInfo {
            manifest_key: manifest.unwrap_or_default(),
            blob_keys: keys,
            tables_json: "[]".to_string(),
            prepared_bytes: 1,
        };
        f.registry
            .complete(&source_of(name), "job", info, now())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn attachment_gc_refuses_a_tracked_key_outside_the_derived_root() {
        let f = fixture().await;
        seed_tracking(
            &f,
            "evil",
            vec!["chat-attachments/other-user/secret.csv".to_string()],
            Some(manifest_of("evil")),
        )
        .await;
        let storage = FakeStorage::with(&["chat-attachments/other-user/secret.csv".to_string()]);
        let summary = gc_source(&f, &storage, "evil", &now, false).await;
        assert_eq!(summary.keys_rejected, 1);
        assert_eq!(summary.rows_deleted, 0);
        assert_eq!(storage.calls(), 0, "nothing was deleted");
        assert_eq!(row(&f, "evil").await.unwrap().status, PrepareStatus::Ready);
    }

    #[tokio::test]
    async fn attachment_gc_refuses_a_sibling_prefix_of_the_derived_root() {
        // `prepared-other` starts with `prepared` but is not inside it.
        for bare in [false, true] {
            let f = fixture().await;
            let sibling = "chat-attachments/u/s/prepared-other/x/part-00000.parquet".to_string();
            seed_tracking(&f, "sib", vec![sibling.clone()], Some(manifest_of("sib"))).await;
            let storage = FakeStorage::with(std::slice::from_ref(&sibling));
            *storage.bare_root.lock().unwrap() = bare;
            let summary = gc_source(&f, &storage, "sib", &now, false).await;
            assert_eq!(summary.keys_rejected, 1, "bare_root={bare}");
            assert_eq!(storage.calls(), 0);
        }
        // The same root with a key really inside it is accepted, bare or not.
        for bare in [false, true] {
            let f = fixture().await;
            seed_ready(&f, "ok", 0).await;
            let storage = storage_with(&["ok"]);
            *storage.bare_root.lock().unwrap() = bare;
            let summary = gc_source(&f, &storage, "ok", &now, false).await;
            assert_eq!(summary.rows_deleted, 1, "bare_root={bare}");
        }
    }

    #[tokio::test]
    async fn attachment_gc_without_a_derived_root_refuses_the_keys_it_cannot_contain() {
        let f = fixture().await;
        seed_ready(&f, "a", 0).await;
        let storage = storage_with(&["a"]);
        *storage.no_root.lock().unwrap() = true;
        let summary = gc_source(&f, &storage, "a", &now, false).await;
        assert_eq!(summary.keys_rejected, 1);
        assert_eq!(summary.blobs_deleted, 0);
        assert_eq!(storage.calls(), 0, "no blob is deleted without containment");
        assert_eq!(row(&f, "a").await.unwrap().status, PrepareStatus::Ready);
    }

    #[tokio::test]
    async fn attachment_gc_a_row_that_tracks_no_key_needs_no_root() {
        let f = fixture().await;
        f.registry
            .claim(claim_at("empty", "job", now()))
            .await
            .unwrap()
            .unwrap();
        f.registry
            .fail(&source_of("empty"), "job", "time", "x", now())
            .await
            .unwrap();
        let storage = FakeStorage::with(&[]);
        *storage.no_root.lock().unwrap() = true;
        let summary = gc_source(&f, &storage, "empty", &now, false).await;
        assert_eq!(
            summary.rows_deleted, 1,
            "nothing to contain: the row just goes"
        );
        assert_eq!(summary.keys_rejected, 0);
    }

    #[tokio::test]
    async fn attachment_gc_never_asks_the_adapter_to_delete_with_an_empty_list() {
        let f = fixture().await;
        f.registry
            .claim(claim_at("empty", "job", now()))
            .await
            .unwrap()
            .unwrap();
        f.registry
            .fail(&source_of("empty"), "job", "time", "x", now())
            .await
            .unwrap();
        // With a root and without one: a row that tracks nothing is settled and the
        // adapter, which might read an empty list as "delete by prefix", is not asked.
        let storage = FakeStorage::with(&[]);
        let summary = gc_source(&f, &storage, "empty", &now, false).await;
        assert_eq!(summary.rows_deleted, 1);
        assert_eq!(*storage.derived_calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn attachment_gc_never_deletes_the_source_key_or_a_dot_dot_key_with_or_without_a_root() {
        let sneaky = "chat-attachments/u/s/prepared/x/../../../other/secret.csv".to_string();
        for no_root in [false, true] {
            for (name, bad) in [("self", source_of("self")), ("dots", sneaky.clone())] {
                let f = fixture().await;
                seed_tracking(&f, name, vec![bad.clone()], Some(manifest_of(name))).await;
                let storage = FakeStorage::with(std::slice::from_ref(&bad));
                *storage.no_root.lock().unwrap() = no_root;
                let summary = gc_source(&f, &storage, name, &now, false).await;
                assert_eq!(summary.keys_rejected, 1, "{name} no_root={no_root}");
                assert_eq!(storage.calls(), 0);
            }
        }
    }

    #[tokio::test]
    async fn attachment_gc_a_dry_run_refuses_what_the_real_run_refuses() {
        let f = fixture().await;
        seed_tracking(
            &f,
            "evil",
            vec!["chat-attachments/other-user/secret.csv".to_string()],
            Some(manifest_of("evil")),
        )
        .await;
        let storage = FakeStorage::with(&[]);
        let dry = gc_source(&f, &storage, "evil", &now, true).await;
        let real = gc_source(&f, &storage, "evil", &now, false).await;
        assert_eq!(
            dry, real,
            "the dry run must not promise what the real run refuses"
        );
        assert_eq!(dry.keys_rejected, 1);
    }
}
