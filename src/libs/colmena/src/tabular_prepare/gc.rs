//! Cleanup of prepared tables, run by the `attachment_gc` binary next to its
//! own pass over `conversation_attachments`.
//!
//! Idempotent steps: delete the prepared tables of a source whose blob was
//! just deleted (this slice); later slices add failure handling and lease
//! accounting, the containment checks on tracked keys, the cancellation of a live
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
//! Time is read from a clock once per row, never once per pass, so each row's
//! lease starts when the pass claims it. With an empty table none of this makes
//! a storage call.

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
    summary.blobs_deleted += keys.len() as u64;
    if registry.finish_delete(source, &owner).await? {
        summary.rows_deleted += 1;
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
    use std::sync::{Arc, Mutex};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()
    }

    /// In-memory storage that remembers what was deleted and in which order,
    /// and can refuse to delete or to find chosen keys.
    #[derive(Default)]
    struct FakeStorage {
        blobs: Mutex<HashSet<String>>,
        deleted: Mutex<Vec<String>>,
        failing: Mutex<HashSet<String>>,
        unreachable: Mutex<bool>,
        reads: Mutex<usize>,
    }

    impl FakeStorage {
        fn with(keys: &[String]) -> Arc<Self> {
            let s = Self::default();
            s.blobs.lock().unwrap().extend(keys.iter().cloned());
            Arc::new(s)
        }
        fn calls(&self) -> usize {
            self.deleted.lock().unwrap().len() + *self.reads.lock().unwrap()
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
}
