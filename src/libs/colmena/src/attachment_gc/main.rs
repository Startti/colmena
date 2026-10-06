//! Plan C: TTL cleanup binary for `conversation_attachments` + their backing
//! blobs in `OutputStorageRepository`.
//!
//! Reads `COLMENA_ATTACHMENT_TTL_DAYS` (default 7), `DATABASE_URL`, and
//! storage adapter env (mirrors the `dag_engine` binary). Queries
//! `find_stale_attachments` in batches (configurable via
//! `COLMENA_ATTACHMENT_GC_BATCH_SIZE`, default 100), deletes the underlying
//! blob from storage, then deletes the registry row. Loops until a batch
//! returns fewer than `batch_size` rows.
//!
//! Idempotent: if the storage blob delete fails, the registry row is
//! preserved so the next invocation retries.
//!
//! Designed to run as a Cloud Scheduler job pointing at a Cloud Run Job
//! that executes this binary. See `docs/developer_guide/36_attachment_gc.md`
//! for the deployment recipe.

use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use colmena::dag_engine::engine::EngineConfig;
use colmena::dag_engine::infrastructure::pool_registry::PgPoolRegistry;
use colmena::llm::domain::{AttachmentRegistry, StaleAttachmentQuery};
use colmena::llm::infrastructure::persistence::PostgresAttachmentRegistry;
use colmena::storage::domain::OutputStorageRepository;

#[derive(Parser, Debug)]
#[command(version, about = "Periodically delete stale attachments + their blobs")]
struct Cli {
    /// Override the env-derived TTL (days). Default is `COLMENA_ATTACHMENT_TTL_DAYS` or 7.
    #[arg(long)]
    ttl_days: Option<u32>,

    /// Override the batch size. Default is `COLMENA_ATTACHMENT_GC_BATCH_SIZE` or 100.
    #[arg(long)]
    batch_size: Option<u32>,

    /// Dry run — log what would be deleted without actually deleting.
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

#[derive(Debug, Default, Clone, Copy)]
struct GcSummary {
    /// Rows removed (a storage reference counts here too).
    total_deleted: u64,
    total_storage_errors: u64,
    /// Of `total_deleted`, rows that pointed at an object the host owns: the
    /// row went, the object was left alone.
    total_host_references_released: u64,
    /// Dry run only: rows that would be deleted with their blob.
    would_delete: u64,
    /// Dry run only: host references whose row would be dropped, object kept.
    would_release_host_references: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let ttl_days = cli.ttl_days.unwrap_or_else(|| {
        std::env::var("COLMENA_ATTACHMENT_TTL_DAYS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(7)
    });
    let batch_size = cli.batch_size.unwrap_or_else(|| {
        std::env::var("COLMENA_ATTACHMENT_GC_BATCH_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100)
    });

    let cutoff = chrono::Utc::now() - chrono::Duration::days(ttl_days as i64);

    tracing::info!(
        target: "colmena::attachment_gc",
        event = "gc.start",
        ttl_days,
        batch_size,
        cutoff = %cutoff,
        dry_run = cli.dry_run,
        "starting attachment_gc"
    );

    let (registry, storage) = build_services_from_env().await?;

    let summary = run_gc(registry, storage, cutoff, batch_size, cli.dry_run).await?;

    tracing::info!(
        target: "colmena::attachment_gc",
        event = "gc.end",
        total_deleted = summary.total_deleted,
        total_storage_errors = summary.total_storage_errors,
        "attachment_gc complete"
    );

    Ok(())
}

/// Build the `(AttachmentRegistry, OutputStorageRepository)` pair from
/// environment variables.
///
/// **Wiring strategy:** reuses [`EngineConfig::from_env`] for the storage
/// adapter (which encapsulates the `COLMENA_LOCAL` / callback / local-http /
/// in-memory selection logic), then builds a fresh
/// [`PgPoolRegistry`] + [`PostgresAttachmentRegistry`] for the registry
/// side. Migrations are run on the internal pool so the binary is safe to
/// invoke against a fresh database (no-op when migrations are up to date).
async fn build_services_from_env() -> Result<
    (
        Arc<dyn AttachmentRegistry>,
        Arc<dyn OutputStorageRepository>,
    ),
    Box<dyn std::error::Error>,
> {
    let engine_config = EngineConfig::from_env().await?;

    let pool_registry = Arc::new(PgPoolRegistry::new(engine_config.pool_config));

    // Pin the internal pool so we can run migrations on it. PostgresAttachmentRegistry::new
    // also calls get_or_create on the same URL — it will hit the cache and reuse this Arc.
    let internal_pool = pool_registry
        .pin(&engine_config.internal_database_url)
        .await?;

    let mut migrator = sqlx::migrate!("migrations/postgres");
    migrator.set_ignore_missing(true);
    migrator.run(&*internal_pool).await?;

    let registry =
        PostgresAttachmentRegistry::new(pool_registry, &engine_config.internal_database_url)
            .await?;

    Ok((Arc::new(registry), engine_config.storage))
}

/// Core GC loop. Extracted from `main` so it can be unit-tested with
/// in-memory registry + mock storage.
///
/// Loops over [`AttachmentRegistry::find_stale_attachments`] in batches of
/// `batch_size`. For each row:
///   1. (skip if `dry_run`) Delete the blob from storage. On failure log a
///      warning and skip — the registry row is preserved so the next run
///      retries.
///   2. Delete the registry row. On failure log an error and continue;
///      the blob is already gone so the row will become inconsistent —
///      this should be rare (registry delete is just a DELETE by PK).
///
/// Stops when a batch returns fewer than `batch_size` rows.
async fn run_gc(
    registry: Arc<dyn AttachmentRegistry>,
    storage: Arc<dyn OutputStorageRepository>,
    cutoff: chrono::DateTime<chrono::Utc>,
    batch_size: u32,
    dry_run: bool,
) -> Result<GcSummary, Box<dyn std::error::Error>> {
    let mut summary = GcSummary::default();

    loop {
        let stale = registry
            .find_stale_attachments(StaleAttachmentQuery {
                cutoff,
                limit: batch_size,
            })
            .await?;

        let batch_len = stale.len();
        if batch_len == 0 {
            break;
        }

        tracing::info!(
            target: "colmena::attachment_gc",
            event = "gc.batch.start",
            batch_size = batch_len,
            "processing batch"
        );

        for row in stale {
            let host_owned = row.is_host_storage_ref();
            if dry_run {
                // Says what it WOULD do, and never prints a host object's key.
                if host_owned {
                    summary.would_release_host_references += 1;
                    tracing::info!(
                        target: "colmena::attachment_gc",
                        event = "gc.dry_run.would_release",
                        document_id = %row.document_id,
                        agent_session_id = %row.agent_session_id,
                        provider = %row.provider,
                        "[dry-run] would release (drop the row, object kept)"
                    );
                } else {
                    summary.would_delete += 1;
                    tracing::info!(
                        target: "colmena::attachment_gc",
                        event = "gc.dry_run.would_delete",
                        document_id = %row.document_id,
                        agent_session_id = %row.agent_session_id,
                        storage_key = ?row.storage_key,
                        last_used_at = ?row.last_used_at,
                        registered_at = %row.registered_at,
                        "[dry-run] would delete"
                    );
                }
                continue;
            }

            // Step 1: delete the blob (best-effort), unless the object is the
            // HOST's: a row that references one is only a row. If the blob delete
            // fails, skip deleting the registry row so the next run retries.
            if let Some(storage_key) = row.storage_key.as_deref().filter(|_| !host_owned) {
                if let Err(e) = storage.delete(storage_key).await {
                    tracing::warn!(
                        target: "colmena::attachment_gc",
                        event = "gc.storage_delete_failed",
                        document_id = %row.document_id,
                        storage_key,
                        error = %e,
                        "storage.delete failed; skipping registry delete, will retry next run"
                    );
                    summary.total_storage_errors += 1;
                    continue;
                }
            }

            // Step 2: delete the registry row.
            if let Err(e) = registry
                .delete_attachment(&row.agent_session_id, &row.document_id)
                .await
            {
                tracing::error!(
                    target: "colmena::attachment_gc",
                    event = "gc.registry_delete_failed",
                    document_id = %row.document_id,
                    agent_session_id = %row.agent_session_id,
                    error = %e,
                    "registry delete failed; the blob (if any) is already gone"
                );
                continue;
            }

            summary.total_deleted += 1;
            if host_owned {
                // Counted only now that the row is really gone.
                summary.total_host_references_released += 1;
                tracing::info!(
                    target: "colmena::attachment_gc",
                    event = "gc.host_reference_object_kept",
                    document_id = %row.document_id,
                    agent_session_id = %row.agent_session_id,
                    "storage reference: row dropped, the host's object was left alone"
                );
            }
        }

        tracing::info!(
            target: "colmena::attachment_gc",
            event = "gc.batch.end",
            batch_size = batch_len,
            total_deleted = summary.total_deleted,
            total_storage_errors = summary.total_storage_errors,
            total_host_references_released = summary.total_host_references_released,
            "batch complete"
        );

        if (batch_len as u32) < batch_size {
            break;
        }

        // Tiny breather between batches so we don't hammer the DB.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    //! Unit tests for the GC loop, using in-memory SQLite + LocalCache storage.

    use super::*;
    use colmena::llm::domain::attachments::origin;
    use colmena::llm::domain::{AttachmentSource, ProviderKind, UpsertAttachmentInput};
    use colmena::llm::infrastructure::persistence::SqliteAttachmentRegistry;
    use colmena::storage::domain::StoreRequest;
    use colmena::storage::infrastructure::LocalCacheStorageAdapter;
    use sqlx::SqlitePool;

    async fn seed_attachment(
        registry: &SqliteAttachmentRegistry,
        storage: &LocalCacheStorageAdapter,
        agent_session_id: &str,
        document_id: &str,
        bytes: &[u8],
    ) -> String {
        // Persist bytes first to obtain a storage_key.
        let stored = storage
            .store(StoreRequest {
                bytes: bytes.to_vec(),
                mime_type: "application/octet-stream".to_string(),
                filename: format!("{}.bin", document_id),
                session_id: None,
                agent_session_id: Some(agent_session_id.to_string()),
            })
            .await
            .expect("storage.store");

        registry
            .upsert(UpsertAttachmentInput {
                agent_session_id: agent_session_id.to_string(),
                document_id: document_id.to_string(),
                provider: ProviderKind::OpenAi,
                provider_file_id: format!("file-{}", document_id),
                mime_type: "application/octet-stream".to_string(),
                filename: format!("{}.bin", document_id),
                size_bytes: Some(bytes.len() as u64),
                label: None,
                description: None,
                source: AttachmentSource::Inline,
                storage_key: Some(stored.storage_key.clone()),
                origin: Some(origin::USER_UPLOAD.to_string()),
            })
            .await
            .expect("upsert");

        stored.storage_key
    }

    /// Backdate `registered_at` (and null out `last_used_at`) so the row is
    /// stale relative to the test's cutoff. Updates directly via raw SQL —
    /// the registry trait has no API for this.
    async fn backdate(pool: &SqlitePool, document_id: &str, days_old: i64) {
        let dt = chrono::Utc::now() - chrono::Duration::days(days_old);
        sqlx::query(
            "UPDATE conversation_attachments \
             SET registered_at = ?1, last_used_at = NULL \
             WHERE document_id = ?2",
        )
        .bind(dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .bind(document_id)
        .execute(pool)
        .await
        .expect("backdate update");
    }

    /// Returns (registry, pool, _tempdir_guard). The pool is shared between
    /// the registry and the test's raw-SQL backdate helper. The TempDir must
    /// outlive the pool — drop it last.
    async fn fresh_sqlite_registry(
    ) -> (SqliteAttachmentRegistry, Arc<SqlitePool>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("gc_test.db");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());

        let pool = Arc::new(SqlitePool::connect(&url).await.expect("pool connect"));
        sqlx::migrate!("migrations/sqlite")
            .set_ignore_missing(true)
            .run(&*pool)
            .await
            .expect("migrate");
        let registry = SqliteAttachmentRegistry::from_pool(pool.clone());
        (registry, pool, dir)
    }

    #[tokio::test]
    async fn run_gc_deletes_stale_rows_and_blobs() {
        let (registry, pool, _dir) = fresh_sqlite_registry().await;
        let storage = Arc::new(LocalCacheStorageAdapter::new());

        // Seed 3 stale + 1 fresh attachment.
        let k1 = seed_attachment(&registry, &storage, "sess", "old-1", b"aaa").await;
        let k2 = seed_attachment(&registry, &storage, "sess", "old-2", b"bbb").await;
        let k3 = seed_attachment(&registry, &storage, "sess", "old-3", b"ccc").await;
        let k4 = seed_attachment(&registry, &storage, "sess", "fresh", b"ddd").await;

        backdate(&pool, "old-1", 10).await;
        backdate(&pool, "old-2", 10).await;
        backdate(&pool, "old-3", 10).await;
        // "fresh" remains at now() so it's NOT stale.

        let cutoff = chrono::Utc::now() - chrono::Duration::days(7);

        let registry_arc: Arc<dyn AttachmentRegistry> = Arc::new(registry);
        let storage_arc: Arc<dyn OutputStorageRepository> = storage.clone();

        let summary = run_gc(registry_arc.clone(), storage_arc.clone(), cutoff, 2, false)
            .await
            .unwrap();

        assert_eq!(summary.total_deleted, 3);
        assert_eq!(summary.total_storage_errors, 0);

        // Stale blobs are gone, fresh blob remains.
        assert!(storage.read(&k1).await.is_err());
        assert!(storage.read(&k2).await.is_err());
        assert!(storage.read(&k3).await.is_err());
        assert!(storage.read(&k4).await.is_ok());

        // Stale rows are gone, fresh row remains.
        assert!(registry_arc
            .lookup_by_document_id("sess", "old-1")
            .await
            .unwrap()
            .is_none());
        assert!(registry_arc
            .lookup_by_document_id("sess", "fresh")
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn run_gc_dry_run_deletes_nothing() {
        let (registry, pool, _dir) = fresh_sqlite_registry().await;
        let storage = Arc::new(LocalCacheStorageAdapter::new());

        let k1 = seed_attachment(&registry, &storage, "sess", "old-1", b"aaa").await;
        backdate(&pool, "old-1", 30).await;

        let cutoff = chrono::Utc::now() - chrono::Duration::days(7);

        let registry_arc: Arc<dyn AttachmentRegistry> = Arc::new(registry);
        let storage_arc: Arc<dyn OutputStorageRepository> = storage.clone();

        let summary = run_gc(registry_arc.clone(), storage_arc.clone(), cutoff, 10, true)
            .await
            .unwrap();

        assert_eq!(summary.total_deleted, 0);
        assert_eq!(summary.total_storage_errors, 0);

        // Nothing was deleted.
        assert!(storage.read(&k1).await.is_ok());
        assert!(registry_arc
            .lookup_by_document_id("sess", "old-1")
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn run_gc_returns_immediately_when_no_stale_rows() {
        let (registry, _pool, _dir) = fresh_sqlite_registry().await;
        let storage = Arc::new(LocalCacheStorageAdapter::new());

        // Seed a fresh row (not stale).
        seed_attachment(&registry, &storage, "sess", "fresh", b"xyz").await;

        let cutoff = chrono::Utc::now() - chrono::Duration::days(7);

        let registry_arc: Arc<dyn AttachmentRegistry> = Arc::new(registry);
        let storage_arc: Arc<dyn OutputStorageRepository> = storage.clone();

        let summary = run_gc(registry_arc, storage_arc, cutoff, 100, false)
            .await
            .unwrap();

        assert_eq!(summary.total_deleted, 0);
        assert_eq!(summary.total_storage_errors, 0);
    }

    /// A storage whose `delete` is recorded: the host's objects must never reach it.
    struct DeleteRecorder {
        inner: LocalCacheStorageAdapter,
        deleted: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl OutputStorageRepository for DeleteRecorder {
        async fn store(
            &self,
            req: StoreRequest,
        ) -> Result<colmena::storage::domain::StoredOutput, colmena::storage::domain::StorageError>
        {
            self.inner.store(req).await
        }
        async fn read(
            &self,
            key: &str,
        ) -> Result<colmena::storage::domain::StoredBytes, colmena::storage::domain::StorageError>
        {
            self.inner.read(key).await
        }
        async fn read_stream(
            &self,
            key: &str,
        ) -> Result<colmena::storage::domain::StoredStream, colmena::storage::domain::StorageError>
        {
            self.inner.read_stream(key).await
        }
        async fn delete(&self, key: &str) -> Result<(), colmena::storage::domain::StorageError> {
            self.deleted.lock().unwrap().push(key.to_string());
            self.inner.delete(key).await
        }
    }

    #[tokio::test]
    async fn a_stale_storage_reference_row_is_dropped_but_the_hosts_object_is_never_deleted() {
        let (registry, pool, _dir) = fresh_sqlite_registry().await;
        let storage = Arc::new(DeleteRecorder {
            inner: LocalCacheStorageAdapter::new(),
            deleted: Default::default(),
        });
        let host_key = seed_row(
            &registry,
            &pool,
            &storage,
            "host-ref",
            ProviderKind::OpenAi,
            true,
        )
        .await;
        let copy_key = seed_attachment(&registry, &storage.inner, "sess", "copy", b"mine").await;
        backdate(&pool, "host-ref", 30).await;
        backdate(&pool, "copy", 30).await;

        let registry_arc: Arc<dyn AttachmentRegistry> = Arc::new(registry);
        let storage_arc: Arc<dyn OutputStorageRepository> = storage.clone();
        let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
        let summary = run_gc(registry_arc.clone(), storage_arc, cutoff, 10, false)
            .await
            .unwrap();

        assert_eq!(
            *storage.deleted.lock().unwrap(),
            std::slice::from_ref(&copy_key)
        );
        assert!(
            storage.read(&host_key).await.is_ok(),
            "the host's object stays"
        );
        assert!(
            storage.read(&copy_key).await.is_err(),
            "an engine copy is deleted as before"
        );
        for id in ["host-ref", "copy"] {
            assert!(registry_arc
                .lookup_by_document_id("sess", id)
                .await
                .unwrap()
                .is_none());
        }
        assert_eq!(summary.total_deleted, 2);
        assert_eq!(summary.total_host_references_released, 1);
        assert_eq!(summary.total_storage_errors, 0);
    }

    #[tokio::test]
    async fn a_dry_run_deletes_nothing_for_a_storage_reference_either() {
        let (registry, pool, _dir) = fresh_sqlite_registry().await;
        let storage = Arc::new(DeleteRecorder {
            inner: LocalCacheStorageAdapter::new(),
            deleted: Default::default(),
        });
        seed_row(
            &registry,
            &pool,
            &storage,
            "host-ref",
            ProviderKind::OpenAi,
            true,
        )
        .await;
        backdate(&pool, "host-ref", 30).await;
        let registry_arc: Arc<dyn AttachmentRegistry> = Arc::new(registry);
        let storage_arc: Arc<dyn OutputStorageRepository> = storage.clone();
        let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
        run_gc(registry_arc.clone(), storage_arc, cutoff, 10, true)
            .await
            .unwrap();
        assert!(storage.deleted.lock().unwrap().is_empty());
        assert!(registry_arc
            .lookup_by_document_id("sess", "host-ref")
            .await
            .unwrap()
            .is_some());
    }

    /// A row of `provider` for `document_id`: the host's (`host`) or the engine's.
    async fn seed_row(
        registry: &SqliteAttachmentRegistry,
        pool: &SqlitePool,
        storage: &DeleteRecorder,
        document_id: &str,
        provider: ProviderKind,
        host: bool,
    ) -> String {
        let key = seed_attachment(registry, &storage.inner, "sess", document_id, b"x").await;
        // `seed_attachment` registered it under OpenAi; rewrite it under `provider`.
        sqlx::query("DELETE FROM conversation_attachments WHERE document_id = ?1")
            .bind(document_id)
            .execute(pool)
            .await
            .unwrap();
        registry
            .upsert(UpsertAttachmentInput {
                agent_session_id: "sess".to_string(),
                document_id: document_id.to_string(),
                provider,
                provider_file_id: if host {
                    String::new()
                } else {
                    "pf".to_string()
                },
                mime_type: "text/csv".to_string(),
                filename: "f.csv".to_string(),
                size_bytes: Some(10),
                label: None,
                description: None,
                source: AttachmentSource::Path(key.clone()),
                storage_key: Some(key.clone()),
                origin: Some(
                    if host {
                        origin::HOST_STORAGE_REF
                    } else {
                        origin::USER_UPLOAD
                    }
                    .to_string(),
                ),
            })
            .await
            .unwrap();
        key
    }

    async fn backdate_row(pool: &SqlitePool, document_id: &str, provider: &str) {
        let dt = chrono::Utc::now() - chrono::Duration::days(30);
        sqlx::query(
            "UPDATE conversation_attachments SET registered_at = ?1, last_used_at = NULL \
             WHERE document_id = ?2 AND provider = ?3",
        )
        .bind(dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .bind(document_id)
        .bind(provider)
        .execute(pool)
        .await
        .unwrap();
    }

    fn recorder() -> Arc<DeleteRecorder> {
        Arc::new(DeleteRecorder {
            inner: LocalCacheStorageAdapter::new(),
            deleted: Default::default(),
        })
    }

    /// A dry run says what it WOULD do and never prints a host key.
    #[tokio::test]
    async fn a_dry_run_counts_what_it_would_release_and_what_it_would_delete() {
        let (registry, pool, _dir) = fresh_sqlite_registry().await;
        let storage = recorder();
        seed_row(&registry, &pool, &storage, "h", ProviderKind::OpenAi, true).await;
        seed_row(&registry, &pool, &storage, "e", ProviderKind::OpenAi, false).await;
        backdate_row(&pool, "h", "openai").await;
        backdate_row(&pool, "e", "openai").await;
        let registry_arc: Arc<dyn AttachmentRegistry> = Arc::new(registry);
        let storage_arc: Arc<dyn OutputStorageRepository> = storage.clone();
        let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
        let summary = run_gc(registry_arc.clone(), storage_arc, cutoff, 10, true)
            .await
            .unwrap();
        assert_eq!(summary.would_release_host_references, 1);
        assert_eq!(summary.would_delete, 1);
        assert_eq!(summary.total_deleted, 0);
        assert_eq!(summary.total_host_references_released, 0);
        assert!(storage.deleted.lock().unwrap().is_empty());
        for id in ["h", "e"] {
            assert!(registry_arc
                .lookup_by_document_id("sess", id)
                .await
                .unwrap()
                .is_some());
        }
    }

    /// The `released` counter moves only when the row is really gone.
    #[tokio::test]
    async fn a_release_is_counted_only_after_the_row_is_deleted() {
        let (registry, pool, _dir) = fresh_sqlite_registry().await;
        let storage = recorder();
        seed_row(&registry, &pool, &storage, "h", ProviderKind::OpenAi, true).await;
        backdate_row(&pool, "h", "openai").await;
        let failing: Arc<dyn AttachmentRegistry> = Arc::new(FailingDelete(registry));
        let storage_arc: Arc<dyn OutputStorageRepository> = storage.clone();
        let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
        let summary = run_gc(failing.clone(), storage_arc, cutoff, 10, false)
            .await
            .unwrap();
        assert_eq!(summary.total_host_references_released, 0);
        assert_eq!(summary.total_deleted, 0);
        assert!(failing
            .lookup_by_document_id("sess", "h")
            .await
            .unwrap()
            .is_some());
        assert!(storage.deleted.lock().unwrap().is_empty());
    }

    /// A registry whose row deletes fail and whose reads delegate.
    struct FailingDelete(SqliteAttachmentRegistry);

    #[async_trait::async_trait]
    impl AttachmentRegistry for FailingDelete {
        async fn upsert(
            &self,
            i: UpsertAttachmentInput,
        ) -> Result<(), colmena::llm::domain::AttachmentError> {
            self.0.upsert(i).await
        }
        async fn lookup(
            &self,
            s: &str,
            d: &str,
            p: ProviderKind,
        ) -> Result<
            Option<colmena::llm::domain::ConversationAttachment>,
            colmena::llm::domain::AttachmentError,
        > {
            self.0.lookup(s, d, p).await
        }
        async fn refresh_provider_file_id(
            &self,
            s: &str,
            d: &str,
            p: ProviderKind,
            id: &str,
        ) -> Result<(), colmena::llm::domain::AttachmentError> {
            self.0.refresh_provider_file_id(s, d, p, id).await
        }
        async fn update_description(
            &self,
            s: &str,
            d: &str,
            p: ProviderKind,
            t: &str,
        ) -> Result<(), colmena::llm::domain::AttachmentError> {
            self.0.update_description(s, d, p, t).await
        }
        async fn list_for_session(
            &self,
            s: &str,
        ) -> Result<
            Vec<colmena::llm::domain::ConversationAttachment>,
            colmena::llm::domain::AttachmentError,
        > {
            self.0.list_for_session(s).await
        }
        async fn lookup_by_document_id(
            &self,
            s: &str,
            d: &str,
        ) -> Result<
            Option<colmena::llm::domain::ConversationAttachment>,
            colmena::llm::domain::AttachmentError,
        > {
            self.0.lookup_by_document_id(s, d).await
        }
        async fn touch_last_used(
            &self,
            s: &str,
            d: &str,
        ) -> Result<(), colmena::llm::domain::AttachmentError> {
            self.0.touch_last_used(s, d).await
        }
        async fn find_stale_attachments(
            &self,
            q: colmena::llm::domain::StaleAttachmentQuery,
        ) -> Result<
            Vec<colmena::llm::domain::ConversationAttachment>,
            colmena::llm::domain::AttachmentError,
        > {
            self.0.find_stale_attachments(q).await
        }
        async fn delete_attachment(
            &self,
            _: &str,
            _: &str,
        ) -> Result<(), colmena::llm::domain::AttachmentError> {
            Err(colmena::llm::domain::AttachmentError::RepositoryFailed(
                "delete failed".into(),
            ))
        }
    }
}
