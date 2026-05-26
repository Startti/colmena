# Attachment Uniform Resolution — Plan C (TTL Cleanup Binary)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep `OutputStorageRepository` from growing unbounded by introducing a periodic cleanup binary that deletes attachments whose `last_used_at` (or `registered_at` as fallback) is older than a configurable threshold. Plan A introduced the `last_used_at` column and the resolver-driven touch; Plan C consumes it.

**Architecture:** A new domain method on `AttachmentRegistry` finds stale rows (`COALESCE(last_used_at, registered_at) < cutoff`). A second method deletes a row by `(agent_session_id, document_id)`. A new binary `attachment_gc` orchestrates: read env config, query stale rows in batches, for each row delete the underlying blob from `OutputStorageRepository`, then delete the registry row. Idempotent — safe to re-run if the previous iteration died mid-batch (storage delete failures don't block subsequent attempts; registry row stays until storage confirms removal). Designed to run as a Cloud Scheduler → Cloud Run Job (or local cron during dev).

**Tech Stack:** Rust, existing `sqlx` for DB, `clap` for CLI flags, existing `tracing` for structured logging. New `OutputStorageRepository::delete` method (additive). No new dependencies.

**Spec:** [docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md](../specs/2026-05-25-attachment-uniform-resolution-design.md) — decision D10.

**Depends on:** Plan A landed (`storage_key`, `origin`, `last_used_at` columns + resolver). Plan B is NOT a prerequisite for Plan C — they're independent.

**Out of scope:**
- Streaming-based cleanup of stale rows that never had `last_used_at` set (handled by `COALESCE(last_used_at, registered_at)` fallback).
- Per-agent or per-tenant TTL overrides (single global threshold via env var).
- Soft-delete / restore. Cleanup is destructive.
- Dashboard / metrics push (just structured logs — Cloud Logging picks them up).

---

## File Structure

**Create:**
- `src/libs/colmena/src/attachment_gc/main.rs` — binary entry point.
- `src/libs/colmena/src/attachment_gc/mod.rs` (or just `main.rs` + a small `lib.rs`-style structure if preferred) — gc orchestration logic.
- `docs/developer_guide/36_attachment_gc.md` — operational runbook (Spanish, matching docs convention).

**Modify:**
- `src/libs/colmena/Cargo.toml` — add `[[bin]] name = "attachment_gc" path = "src/attachment_gc/main.rs"`.
- `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs` — add `find_stale_attachments` + `delete_attachment` to the trait.
- `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs` — impl the two new methods.
- `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs` — impl the two new methods.
- `src/libs/colmena/src/storage/domain/output_storage_repository.rs` — add `delete(&self, storage_key: &str)` to the trait.
- `src/libs/colmena/src/storage/infrastructure/local_cache_adapter.rs` — impl `delete`.
- `src/libs/colmena/src/storage/infrastructure/local_http_adapter.rs` — impl `delete`.
- `src/libs/colmena/src/storage/infrastructure/http_callback_adapter.rs` — impl `delete` (calls a host application endpoint to delete from GCS).
- `docs/DEVELOPER_GUIDE.md` — add `36_attachment_gc.md` to the TOC.
- `CLAUDE.md` — note the new binary under "Build Commands" + "Current Status".

---

## Task 1: Extend domain types — `find_stale_attachments` + `delete_attachment`

**Goal:** Add the two new methods to the `AttachmentRegistry` trait + a small input type for the query. No infrastructure code yet.

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs`

- [ ] **Step 1: Define the input type for the stale-attachments query**

In `attachment_registry.rs`, after `UpsertAttachmentInput`:

```rust
/// Input for `AttachmentRegistry::find_stale_attachments`.
///
/// A row is "stale" when `COALESCE(last_used_at, registered_at) < cutoff`.
/// The `cutoff` is computed by the caller (typically `now() - ttl`).
#[derive(Debug, Clone)]
pub struct StaleAttachmentQuery {
    /// Rows with `COALESCE(last_used_at, registered_at) < cutoff` are returned.
    pub cutoff: chrono::DateTime<chrono::Utc>,
    /// Maximum number of rows to return in this batch. The binary loops until
    /// the query returns < `limit` rows.
    pub limit: u32,
}
```

- [ ] **Step 2: Add the two trait methods**

Append to the `AttachmentRegistry` trait:

```rust
    /// Plan C: find rows whose `COALESCE(last_used_at, registered_at)` is older
    /// than `query.cutoff`, returning up to `query.limit`. The binary
    /// `attachment_gc` loops until this returns fewer than `limit` rows.
    ///
    /// Returns rows ordered by `COALESCE(last_used_at, registered_at) ASC`
    /// (oldest first) so each batch is deterministic.
    async fn find_stale_attachments(
        &self,
        query: StaleAttachmentQuery,
    ) -> Result<Vec<ConversationAttachment>, AttachmentError>;

    /// Plan C: delete a single attachment row by `(agent_session_id, document_id)`.
    /// Caller is responsible for deleting the underlying blob from
    /// `OutputStorageRepository` BEFORE calling this — the registry row is
    /// the cleanup checkpoint.
    ///
    /// Returns `Ok(())` whether or not the row existed (idempotent).
    async fn delete_attachment(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<(), AttachmentError>;
```

- [ ] **Step 3: Re-export `StaleAttachmentQuery` from `mod.rs`**

Open `src/libs/colmena/src/llm/domain/attachments/mod.rs` and add:

```rust
pub use attachment_registry::{
    AttachmentRegistry, StaleAttachmentQuery, UpsertAttachmentInput,
};
```

(Append `StaleAttachmentQuery` to whatever the existing re-export list is.)

- [ ] **Step 4: Verify compile**

Run: `cargo check --all-targets`

Expected: compile errors in `postgres_attachment_registry.rs` and `sqlite_attachment_registry.rs` (trait methods not implemented) — addressed in Task 2.

**Do not fix the registry impls in this task** — commit the typed-but-broken state first.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/domain/attachments/attachment_registry.rs \
        src/libs/colmena/src/llm/domain/attachments/mod.rs
git commit -m "feat(attachments): add find_stale_attachments + delete_attachment trait methods

Plan C foundation: domain port for the TTL cleanup binary.
StaleAttachmentQuery carries the cutoff timestamp + batch limit; the
binary loops until a batch returns fewer than limit rows.
delete_attachment is idempotent.

Registry impls and binary land in subsequent tasks.

Plan C — TTL Cleanup."
```

---

## Task 2: Implement the two new methods in Postgres + SQLite registries

**Goal:** Make the registry impls compile again and pass tests for the two new methods.

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs`

- [ ] **Step 1: Add tests for both methods in `postgres_attachment_registry.rs`**

Append to the existing `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn find_stale_attachments_returns_rows_older_than_cutoff() {
        let pool = pool_from_env().await;
        let reg = PostgresAttachmentRegistry::new(pool.clone());
        let sid = format!("agent_{}", uuid::Uuid::new_v4());

        // Insert one row from "yesterday" (stale relative to today) and one
        // from "now" (fresh).
        reg.upsert(UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: "stale-doc".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-stale".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "old.pdf".to_string(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key: Some("sk-stale".to_string()),
            origin: Some("user_upload".to_string()),
        }).await.unwrap();

        reg.upsert(UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: "fresh-doc".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-fresh".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "new.pdf".to_string(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key: Some("sk-fresh".to_string()),
            origin: Some("user_upload".to_string()),
        }).await.unwrap();

        // Force the first row to look stale by backdating registered_at.
        sqlx::query("UPDATE conversation_attachments SET registered_at = NOW() - INTERVAL '8 days', last_used_at = NULL WHERE document_id = $1")
            .bind("stale-doc")
            .execute(&pool)
            .await
            .unwrap();

        let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
        let stale = reg.find_stale_attachments(StaleAttachmentQuery {
            cutoff,
            limit: 100,
        }).await.unwrap();

        let stale_ids: Vec<&str> = stale.iter().map(|r| r.document_id.as_str()).collect();
        assert!(stale_ids.contains(&"stale-doc"), "stale-doc should be in stale list");
        assert!(!stale_ids.contains(&"fresh-doc"), "fresh-doc should NOT be in stale list");
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn delete_attachment_removes_row_and_is_idempotent() {
        let pool = pool_from_env().await;
        let reg = PostgresAttachmentRegistry::new(pool.clone());
        let sid = format!("agent_{}", uuid::Uuid::new_v4());

        reg.upsert(UpsertAttachmentInput {
            agent_session_id: sid.clone(),
            document_id: "to-delete".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-del".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_bytes: Some(100),
            label: None,
            description: None,
            source: AttachmentSource::Inline,
            storage_key: Some("sk-del".to_string()),
            origin: Some("user_upload".to_string()),
        }).await.unwrap();

        assert!(reg.lookup_by_document_id(&sid, "to-delete").await.unwrap().is_some());

        reg.delete_attachment(&sid, "to-delete").await.unwrap();
        assert!(reg.lookup_by_document_id(&sid, "to-delete").await.unwrap().is_none());

        // Second delete is a no-op (idempotent).
        reg.delete_attachment(&sid, "to-delete").await.unwrap();
    }
```

- [ ] **Step 2: Run the failing tests**

```bash
source .env && cargo test --lib postgres_attachment_registry -- --ignored
```

Expected: COMPILE ERROR — methods not implemented.

- [ ] **Step 3: Implement `find_stale_attachments` in `postgres_attachment_registry.rs`**

Add after `touch_last_used`:

```rust
    async fn find_stale_attachments(
        &self,
        query: StaleAttachmentQuery,
    ) -> Result<Vec<ConversationAttachment>, AttachmentError> {
        let rows = sqlx::query(r#"
            SELECT agent_session_id, document_id, provider, provider_file_id,
                   mime_type, filename, size_bytes, label, description,
                   source_kind, source_value, registered_at, refreshed_at,
                   storage_key, origin, last_used_at
            FROM conversation_attachments
            WHERE COALESCE(last_used_at, registered_at) < $1
            ORDER BY COALESCE(last_used_at, registered_at) ASC
            LIMIT $2
        "#)
        .bind(query.cutoff)
        .bind(query.limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(e.to_string()))?;

        Ok(rows.iter().map(Self::row_to_attachment).collect())
    }

    async fn delete_attachment(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<(), AttachmentError> {
        sqlx::query(r#"
            DELETE FROM conversation_attachments
            WHERE agent_session_id = $1 AND document_id = $2
        "#)
        .bind(agent_session_id)
        .bind(document_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AttachmentError::RepositoryFailed(e.to_string()))?;
        Ok(())
    }
```

Adjust `AttachmentError` variant name to match what the file already uses (e.g., `AttachmentError::Storage` or `AttachmentError::RepositoryFailed` — check the existing impls).

- [ ] **Step 4: Mirror in `sqlite_attachment_registry.rs`**

Same logic, SQLite syntax:
- Replace `$1, $2` with `?1, ?2` (or positional `?` if the file uses that).
- `COALESCE(last_used_at, registered_at)` works in SQLite too.
- Use whatever cutoff binding format the file uses for other timestamp queries (likely `cutoff.to_rfc3339()` or sqlx's native conversion).
- `delete_attachment`: `DELETE FROM ... WHERE agent_session_id = ?1 AND document_id = ?2`.

- [ ] **Step 5: Add equivalent SQLite tests** (no `#[ignore]` since SQLite doesn't need env vars)

Mirror the structure from Step 1 but using SQLite (`SqliteAttachmentRegistry::new("sqlite::memory:")`). For the "backdate" step, use:

```rust
sqlx::query("UPDATE conversation_attachments SET registered_at = datetime('now', '-8 days'), last_used_at = NULL WHERE document_id = ?1")
    .bind("stale-doc")
    .execute(&pool)
    .await
    .unwrap();
```

- [ ] **Step 6: Run tests**

```bash
cargo test --lib sqlite_attachment_registry
source .env && cargo test --lib postgres_attachment_registry -- --ignored
```

Both should pass.

- [ ] **Step 7:** Run `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo check --all-targets`. Clean.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/persistence/postgres_attachment_registry.rs \
        src/libs/colmena/src/llm/infrastructure/persistence/sqlite_attachment_registry.rs
git commit -m "feat(attachments): impl find_stale_attachments + delete_attachment in both registries

Postgres + SQLite. COALESCE(last_used_at, registered_at) ensures rows
that never got touched (last_used_at NULL) still age out via their
registration timestamp. Ordered ASC so each batch starts from the
oldest row.

Delete is idempotent (DELETE WHERE returns 0 rows affected silently).

Includes integration tests in both backends.

Plan C — TTL Cleanup."
```

---

## Task 3: Add `delete` to `OutputStorageRepository`

**Goal:** Storage adapters need a way to remove blobs. Today none of them have a delete method.

**Files:**
- Modify: `src/libs/colmena/src/storage/domain/output_storage_repository.rs`
- Modify: `src/libs/colmena/src/storage/infrastructure/local_cache_adapter.rs`
- Modify: `src/libs/colmena/src/storage/infrastructure/local_http_adapter.rs`
- Modify: `src/libs/colmena/src/storage/infrastructure/http_callback_adapter.rs`

- [ ] **Step 1: Add the trait method**

In `output_storage_repository.rs`, append to the trait:

```rust
    /// Plan C: delete the blob associated with `storage_key`. Idempotent —
    /// returns `Ok(())` whether or not the blob existed. Backend failures
    /// (e.g., GCS unavailable) bubble up as `StorageError::BackendUnavailable`
    /// so the gc binary can retry on the next run.
    async fn delete(&self, storage_key: &str) -> Result<(), StorageError>;
```

- [ ] **Step 2: Implement in `local_cache_adapter.rs`**

The local cache is an in-memory map. Delete is a HashMap removal:

```rust
    async fn delete(&self, storage_key: &str) -> Result<(), StorageError> {
        self.store.write().await.remove(storage_key);
        Ok(())
    }
```

(Adjust to the actual concurrency primitive — `Mutex`, `RwLock`, etc.)

- [ ] **Step 3: Implement in `local_http_adapter.rs`**

Local HTTP adapter writes to a tmp directory. Delete = filesystem delete:

```rust
    async fn delete(&self, storage_key: &str) -> Result<(), StorageError> {
        let path = self.path_for(storage_key);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StorageError::BackendUnavailable(format!(
                "failed to delete {}: {}", path.display(), e
            ))),
        }
    }
```

- [ ] **Step 4: Implement in `http_callback_adapter.rs`**

Production adapter calls the host application's API. Add a new callback URL pattern (e.g. `<base>/internal/gcs/delete`) and POST `{ "storage_key": "..." }`:

```rust
    async fn delete(&self, storage_key: &str) -> Result<(), StorageError> {
        let url = format!("{}/internal/gcs/delete", self.callback_base_url);
        let resp = self.client
            .post(&url)
            .header("X-Internal-Token", &self.callback_secret)
            .json(&serde_json::json!({ "storage_key": storage_key }))
            .send()
            .await
            .map_err(|e| StorageError::BackendUnavailable(format!("delete callback: {e}")))?;

        match resp.status() {
            s if s.is_success() => Ok(()),
            s if s == reqwest::StatusCode::NOT_FOUND => Ok(()),  // idempotent
            s => Err(StorageError::BackendUnavailable(format!(
                "delete callback returned {}: {}", s, resp.text().await.unwrap_or_default()
            ))),
        }
    }
```

Match the exact method on `HttpCallbackStorageAdapter`'s state (it has a `client`, a `callback_base_url`, a `callback_secret`, etc. — look at existing code for the proper field names).

**Note for the ADP team:** the host application needs a new endpoint `/internal/gcs/delete` that authenticates via `X-Internal-Token` and deletes the blob from GCS. This belongs in the ADP migration notes (already covered conceptually under "TTL cleanup pending" but worth surfacing here).

- [ ] **Step 5: Add tests for each adapter**

In `local_cache_adapter.rs`:

```rust
    #[tokio::test]
    async fn delete_removes_stored_blob_and_is_idempotent() {
        let adapter = LocalCacheStorageAdapter::new();
        let stored = adapter.store(StoreRequest {
            bytes: b"hello".to_vec(),
            mime_type: "text/plain".to_string(),
            filename: "h.txt".to_string(),
            session_id: None,
            agent_session_id: None,
        }).await.unwrap();

        adapter.read(&stored.storage_key).await.unwrap();  // exists

        adapter.delete(&stored.storage_key).await.unwrap();
        assert!(matches!(
            adapter.read(&stored.storage_key).await,
            Err(StorageError::InvalidInput(_))
        ));

        // Idempotent.
        adapter.delete(&stored.storage_key).await.unwrap();
    }
```

Mirror for `local_http_adapter.rs`. For `http_callback_adapter.rs`, the test is wiremock-based — set up a mock POST endpoint at `/internal/gcs/delete` and assert it was called.

- [ ] **Step 6:** Run tests + lints. Clean.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/storage/domain/output_storage_repository.rs \
        src/libs/colmena/src/storage/infrastructure/*.rs
git commit -m "feat(storage): add delete method to OutputStorageRepository

All three adapters (LocalCache, LocalHttp, HttpCallback) implement
idempotent delete. The HttpCallback impl posts to <base>/internal/gcs/delete
— ADP host application must expose this endpoint.

Backend failures bubble up as StorageError::BackendUnavailable so the
attachment_gc binary (Task 4) can retry on the next run.

Plan C — TTL Cleanup."
```

---

## Task 4: New `attachment_gc` binary

**Goal:** Standalone binary that reads env config, queries stale rows in batches, deletes the blob from storage then the registry row, and logs results structurally.

**Files:**
- Create: `src/libs/colmena/src/attachment_gc/main.rs`
- Modify: `src/libs/colmena/Cargo.toml` — register the binary.

- [ ] **Step 1: Register the binary in `Cargo.toml`**

In the `[[bin]]` block area:

```toml
[[bin]]
name = "attachment_gc"
path = "src/attachment_gc/main.rs"
```

- [ ] **Step 2: Author the binary**

Create `src/libs/colmena/src/attachment_gc/main.rs`:

```rust
//! Plan C: TTL cleanup binary for conversation_attachments + their backing
//! blobs in OutputStorageRepository.
//!
//! Reads `COLMENA_ATTACHMENT_TTL_DAYS` (default 7), `DATABASE_URL`, and
//! storage adapter env (mirrors the dag_engine binary). Queries
//! `find_stale_attachments` in batches of 100 (configurable via
//! `COLMENA_ATTACHMENT_GC_BATCH_SIZE`), deletes the underlying blob from
//! storage, then deletes the registry row. Loops until a batch returns
//! fewer than `batch_size` rows.
//!
//! Designed to run as a Cloud Scheduler job pointing at a Cloud Run Job
//! that executes this binary. See docs/developer_guide/36_attachment_gc.md
//! for the deployment recipe.

use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use colmena::llm::domain::attachments::{
    AttachmentRegistry, StaleAttachmentQuery,
};
use colmena::storage::domain::OutputStorageRepository;

#[derive(Parser, Debug)]
#[command(version, about = "Periodically delete stale attachments + their blobs")]
struct Cli {
    /// Override the env-derived TTL (days). Default is COLMENA_ATTACHMENT_TTL_DAYS or 7.
    #[arg(long)]
    ttl_days: Option<u32>,

    /// Override the batch size. Default is COLMENA_ATTACHMENT_GC_BATCH_SIZE or 100.
    #[arg(long)]
    batch_size: Option<u32>,

    /// Dry run — log what would be deleted without actually deleting.
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    let ttl_days = cli
        .ttl_days
        .unwrap_or_else(|| {
            std::env::var("COLMENA_ATTACHMENT_TTL_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(7)
        });
    let batch_size = cli
        .batch_size
        .unwrap_or_else(|| {
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

    // Construct services. For now we reuse the same env-based wiring the
    // dag_engine binary uses. See `colmena::dag_engine::api` for the helper
    // that builds (AttachmentRegistry, OutputStorageRepository) from env.
    let (registry, storage) = build_services_from_env().await?;

    let mut total_deleted: u64 = 0;
    let mut total_storage_errors: u64 = 0;

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
            if cli.dry_run {
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
                continue;
            }

            // 1. Delete the blob (best-effort). If it fails, skip deleting
            //    the registry row so the next run retries.
            if let Some(storage_key) = row.storage_key.as_deref() {
                if let Err(e) = storage.delete(storage_key).await {
                    tracing::warn!(
                        target: "colmena::attachment_gc",
                        event = "gc.storage_delete_failed",
                        document_id = %row.document_id,
                        storage_key,
                        error = %e,
                        "storage.delete failed; skipping registry delete, will retry next run"
                    );
                    total_storage_errors += 1;
                    continue;
                }
            }

            // 2. Delete the registry row.
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
                    "registry.delete_attachment failed; storage blob already deleted"
                );
                continue;
            }

            total_deleted += 1;
        }

        tracing::info!(
            target: "colmena::attachment_gc",
            event = "gc.batch.end",
            batch_size = batch_len,
            total_deleted,
            total_storage_errors,
            "batch complete"
        );

        // If this batch was less than batch_size we're done.
        if (batch_len as u32) < batch_size {
            break;
        }

        // Gentle yield between batches to avoid hammering DB / GCS.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    tracing::info!(
        target: "colmena::attachment_gc",
        event = "gc.end",
        total_deleted,
        total_storage_errors,
        "attachment_gc complete"
    );

    Ok(())
}

async fn build_services_from_env() -> Result<
    (Arc<dyn AttachmentRegistry>, Arc<dyn OutputStorageRepository>),
    Box<dyn std::error::Error>,
> {
    // Mirror the wiring in colmena::dag_engine::api::build_engine_from_env (or
    // wherever the dag_engine binary builds its services). For Plan C we only
    // need the two services — no LlmRepository, no NodeRegistry.
    //
    // Specifically:
    //   - AttachmentRegistry → PostgresAttachmentRegistry (read DATABASE_URL)
    //                          OR SqliteAttachmentRegistry if DATABASE_URL is sqlite://...
    //   - OutputStorageRepository → HttpCallbackStorageAdapter (prod)
    //                              OR LocalCacheStorageAdapter (no env)
    //                              OR LocalHttpStorageAdapter (COLMENA_LOCAL=true)
    //
    // Extract or expose a helper from colmena::dag_engine::api so this binary
    // doesn't duplicate the wiring logic. If extraction is too invasive,
    // inline the necessary parts here with a TODO comment.
    todo!("Wire from env — extract or inline from dag_engine::api::build_*")
}
```

The `build_services_from_env` is the one place that needs careful thought. The dag_engine binary builds a full engine with many services. Plan C only needs two. The cleanest fix is to extract a helper in `colmena::dag_engine::api` (or wherever the service composition lives) that returns just the registry + storage; the gc binary calls it.

If the existing wiring is tightly coupled to engine construction and hard to extract, an acceptable alternative is to inline the necessary pieces:

```rust
async fn build_services_from_env() -> Result<...> {
    use colmena::llm::infrastructure::persistence::postgres_attachment_registry::PostgresAttachmentRegistry;
    use colmena::storage::infrastructure::http_callback_adapter::HttpCallbackStorageAdapter;
    // ... etc.

    let database_url = std::env::var("DATABASE_URL")?;
    let pool = sqlx::PgPool::connect(&database_url).await?;
    let registry: Arc<dyn AttachmentRegistry> = Arc::new(PostgresAttachmentRegistry::new(pool));

    let storage: Arc<dyn OutputStorageRepository> = if std::env::var("COLMENA_LOCAL").ok().as_deref() == Some("true") {
        Arc::new(LocalHttpStorageAdapter::new(/* ... */))
    } else if let Ok(callback_url) = std::env::var("COLMENA_STORAGE_CALLBACK_URL") {
        let secret = std::env::var("COLMENA_STORAGE_CALLBACK_SECRET")?;
        Arc::new(HttpCallbackStorageAdapter::new(callback_url, secret))
    } else {
        return Err("no storage adapter configured".into());
    };

    Ok((registry, storage))
}
```

(Read the dag_engine main.rs to see exactly how it does this.)

- [ ] **Step 3: Smoke-test the binary**

```bash
cargo build --bin attachment_gc
COLMENA_ATTACHMENT_TTL_DAYS=7 \
DATABASE_URL=$DATABASE_URL \
COLMENA_LOCAL=true \
COLMENA_LOCAL_STORAGE_DIR=/tmp/colmena-out \
cargo run --bin attachment_gc -- --dry-run
```

Expected: structured log output showing what would be deleted. No actual deletes (dry-run).

If you have a local DB with test data, run without `--dry-run` and observe actual deletes.

- [ ] **Step 4: Run `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo check --all-targets`. Clean.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/Cargo.toml \
        src/libs/colmena/src/attachment_gc/
git commit -m "feat(attachment_gc): new binary for TTL cleanup of attachments + blobs

Reads COLMENA_ATTACHMENT_TTL_DAYS (default 7) + DATABASE_URL + storage
env (same as dag_engine). Loops over find_stale_attachments in batches,
deletes the blob from storage then the registry row. Idempotent — if
storage delete fails, registry row is preserved so the next run retries.
--dry-run flag for safe inspection.

Structured tracing logs for Cloud Logging.

Plan C — TTL Cleanup."
```

---

## Task 5: Operational runbook

**Goal:** Document how to deploy and operate the `attachment_gc` binary in Cloud Run + Cloud Scheduler. The ADP team is the consumer of this doc.

**Files:**
- Create: `docs/developer_guide/36_attachment_gc.md`
- Modify: `docs/DEVELOPER_GUIDE.md` — add entry to TOC.
- Modify: `CLAUDE.md` — note the new binary under "Build Commands" + "Current Status".

- [ ] **Step 1: Write the runbook**

Create `docs/developer_guide/36_attachment_gc.md` (Spanish, matching docs convention):

```markdown
# 36. Attachment GC — Cleanup binary for TTL'd attachments

Binario standalone que recorre `conversation_attachments`, encuentra filas cuyo
`COALESCE(last_used_at, registered_at) < now() - N días`, y borra:
1. El blob asociado en `OutputStorageRepository` (vía `delete(storage_key)`).
2. La fila en `conversation_attachments` (`delete_attachment(agent_session_id, document_id)`).

## Configuración

| Env var | Default | Descripción |
|---|---|---|
| `COLMENA_ATTACHMENT_TTL_DAYS` | `7` | Edad máxima (en días) que una fila puede tener sin ser usada antes de eliminarse. |
| `COLMENA_ATTACHMENT_GC_BATCH_SIZE` | `100` | Tamaño del batch que se procesa por iteración. |
| `DATABASE_URL` | (requerido) | Misma DB que el dag_engine. |
| `COLMENA_LOCAL` | `false` | Si `true`, usa `LocalHttpStorageAdapter` o `LocalCacheStorageAdapter` (para dev). |
| `COLMENA_STORAGE_CALLBACK_URL` | (prod) | Mismo callback que el dag_engine para sign-put. El GC usará `<base>/internal/gcs/delete`. |
| `COLMENA_STORAGE_CALLBACK_SECRET` | (prod) | Mismo secret. |

CLI flags (override env):
- `--ttl-days N` — override de `COLMENA_ATTACHMENT_TTL_DAYS`.
- `--batch-size N` — override de batch size.
- `--dry-run` — log lo que borraría sin borrar nada.

## Comportamiento

1. Calcula `cutoff = now() - ttl_days`.
2. En loop: `find_stale_attachments(cutoff, batch_size)` → procesa cada fila:
   1. Borra el blob: `storage.delete(storage_key)`. Si falla, **skip** (la fila se preserva, próximo run reintenta).
   2. Borra la fila: `registry.delete_attachment(sid, doc_id)`. Si falla, log error pero el blob ya está borrado.
3. Si el batch devuelve menos filas que `batch_size`, terminamos.
4. Sleep de 100ms entre batches para no martillar DB / storage backend.

Resultado final: log estructurado con `total_deleted` y `total_storage_errors`.

## Deployment

### Local (dev)

```bash
source .env
COLMENA_ATTACHMENT_TTL_DAYS=1 \
cargo run --bin attachment_gc -- --dry-run
```

Sin `--dry-run` borra de verdad. Usar con cuidado si tu DB local tiene data importante.

### Producción (GCP)

Recomendación: **Cloud Scheduler → Cloud Run Job**.

1. Build una imagen Docker que contenga el binario `attachment_gc` (puede ser la misma imagen del dag_engine — el binario está en el mismo workspace).
2. Crear un Cloud Run Job:
   ```bash
   gcloud run jobs create attachment-gc \
     --image gcr.io/PROJECT/colmena:latest \
     --command attachment_gc \
     --set-env-vars=COLMENA_ATTACHMENT_TTL_DAYS=7 \
     --set-env-vars=COLMENA_STORAGE_CALLBACK_URL=https://your-host-api.example.com/internal/gcs/sign-put \
     --set-secrets=DATABASE_URL=projects/PROJECT/secrets/database-url:latest \
     --set-secrets=COLMENA_STORAGE_CALLBACK_SECRET=projects/PROJECT/secrets/storage-callback-secret:latest \
     --max-retries=1 \
     --task-timeout=10m
   ```
3. Crear un Cloud Scheduler trigger:
   ```bash
   gcloud scheduler jobs create http attachment-gc-trigger \
     --schedule="0 3 * * *" \
     --uri="https://run.googleapis.com/v2/projects/PROJECT/locations/REGION/jobs/attachment-gc:run" \
     --http-method=POST \
     --oauth-service-account-email=cloud-scheduler@PROJECT.iam.gserviceaccount.com
   ```
   (Diario a las 3 AM UTC.)

### Endpoint requerido en la host application

El `HttpCallbackStorageAdapter::delete` postea a `<base>/internal/gcs/delete` con body `{ "storage_key": "..." }` + header `X-Internal-Token`. La host application debe:
1. Validar el header.
2. Borrar el blob de GCS por su path (`storage_key`).
3. Devolver 200 si tuvo éxito, 404 si el blob no existía (también OK — el GC lo trata como idempotente).
4. Devolver 5xx en caso de error transitorio (el GC reintentará en la próxima corrida).

## Monitoring

Logs estructurados con target `colmena::attachment_gc`. Filtros útiles en Cloud Logging:

```
resource.type="cloud_run_job"
resource.labels.job_name="attachment-gc"
jsonPayload.event=("gc.start" OR "gc.end")
```

Métricas a vigilar:
- `total_deleted` por corrida — debería crecer linealmente con el throughput de attachments.
- `total_storage_errors` — debería ser 0 o casi 0 en estado normal. Si crece, la host application está fallando o la API del backend de storage tiene problemas.
- Duración de la corrida — si crece más allá de la mitad del task-timeout, considerar bajar `batch_size` o paralelizar.

## Rollback

El binario no tiene rollback — los blobs/filas borrados son permanentes. Para "rollback" en sentido operacional:
- Pausar el Cloud Scheduler job (`gcloud scheduler jobs pause attachment-gc-trigger`).
- Si descubrís que el TTL está muy agresivo y borraste algo importante, subí `COLMENA_ATTACHMENT_TTL_DAYS` a un valor que no vuelva a ser superado pronto. Pero los datos ya borrados no vuelven.

## Riesgos conocidos

- **Storage delete failure → blob huérfano**: si la host application falla al borrar el blob pero responde 5xx, el GC preserva la fila y reintenta. Si la host application borra el blob pero responde 5xx (raro), tendremos una fila sin blob — el próximo run intentará borrar el blob, recibirá 404, y borrará la fila. Idempotencia salva.
- **Registry delete failure post storage delete**: si borramos el blob pero falla la fila, el próximo run intentará borrar el blob de nuevo (404 OK) y luego borrará la fila. Idempotencia salva.
- **TTL muy bajo**: borra docs que el usuario quería conservar. Mitigation: empezar con `COLMENA_ATTACHMENT_TTL_DAYS=30` en prod por las primeras semanas, monitorear quejas, bajar gradualmente.
- **TTL muy alto**: storage crece sin tope. Mitigation: monitorear el tamaño de la tabla `conversation_attachments` y el tamaño del bucket GCS.
```

- [ ] **Step 2: Update `docs/DEVELOPER_GUIDE.md` TOC**

Add an entry in the section index:

```markdown
- [36. Attachment GC — Cleanup binary for TTL'd attachments](developer_guide/36_attachment_gc.md)
```

- [ ] **Step 3: Update `CLAUDE.md`**

Under "Build Commands":

```markdown
- **DAG Engine CLI (run)**: `cargo run --bin dag_engine -- run <path/to/graph.json>`
- **DAG Engine CLI (serve)**: `cargo run --bin dag_engine -- serve <path/to/graph.json>`
- **Attachment GC (cleanup)**: `cargo run --bin attachment_gc -- --dry-run` (or without --dry-run to actually delete)
```

Under "Current Status":

```markdown
- **Attachment uniform resolution Plan C shipped 2026-05-25** — new `attachment_gc`
  binary deletes `conversation_attachments` rows + their backing blobs when
  `COALESCE(last_used_at, registered_at) < now() - COLMENA_ATTACHMENT_TTL_DAYS` (default 7).
  Designed to run as Cloud Scheduler → Cloud Run Job. Requires host application
  to expose `<base>/internal/gcs/delete` endpoint. See
  [`docs/developer_guide/36_attachment_gc.md`](docs/developer_guide/36_attachment_gc.md).
```

- [ ] **Step 4: Commit**

```bash
git add docs/developer_guide/36_attachment_gc.md \
        docs/DEVELOPER_GUIDE.md \
        CLAUDE.md
git commit -m "docs(attachment_gc): operational runbook + Cloud Run Job recipe

Documents:
- Env config + CLI flags
- Cloud Scheduler → Cloud Run Job deployment
- Required host-application endpoint (POST /internal/gcs/delete)
- Monitoring queries + metrics to watch
- Risks + mitigations (TTL too low/high, transient failures)

Plan C — TTL Cleanup."
```

---

## Verification checklist

After all tasks land, run:

- [ ] `cargo fmt --check` — clean.
- [ ] `cargo clippy --all-targets -- -D warnings` — clean.
- [ ] `cargo test --verbose` — all tests pass.
- [ ] `cargo build --bin attachment_gc` — binary builds.
- [ ] `source .env && cargo run --bin attachment_gc -- --dry-run` — runs against the dev DB, logs structured output, returns 0.
- [ ] ADP team has been notified of the new `/internal/gcs/delete` endpoint requirement.

---

## Self-review

**Spec coverage:**
- D10 (TTL cleanup binary) → Tasks 1-5. Complete.

**Type consistency:**
- `find_stale_attachments(StaleAttachmentQuery) -> Vec<ConversationAttachment>` and `delete_attachment(&str, &str) -> ()` consistent between trait, both impls, and binary call sites.
- `OutputStorageRepository::delete(&str) -> Result<(), StorageError>` consistent across the 3 adapters.

**Ambiguity check:**
- The "if storage delete fails, skip registry delete" policy is explicit in Task 4 + the runbook. No ambiguity in behavior.
- The `--dry-run` flag's behavior is explicit (log only, no mutations).

**Scope check:** focused on D10. Not creeping into other decisions.

---

## Risks

1. **Host application endpoint missing**: the `HttpCallbackStorageAdapter::delete` requires a new endpoint. Until ADP ships it, the binary in production will see 100% storage errors and won't delete any registry rows. **Mitigation:** before scheduling the Cloud Run Job in prod, confirm with the ADP team that the endpoint is live. The `--dry-run` flag can be used to verify wiring without touching anything.

2. **TTL miscalibration**: too aggressive deletes user data; too lax wastes storage. **Mitigation:** start with `COLMENA_ATTACHMENT_TTL_DAYS=30` in prod, monitor user complaints + storage size, tune over 4-8 weeks.

3. **Cascading failures during a batch**: if every storage.delete fails (e.g., backend down), the binary processes the whole batch with no deletions. **Mitigation:** acceptable — next scheduled run retries. Storage backend monitoring should fire independently.

4. **Test data pollution**: the integration tests insert rows and backdate them. If they don't clean up properly, future test runs see stale rows. **Mitigation:** each test generates a fresh `agent_session_id` (UUID); tests don't interfere across runs.

5. **Cloud Run Job timeout**: if the batch loop exceeds 10 minutes (task-timeout), Cloud Run kills the job. **Mitigation:** the binary processes batches of 100 with a 100ms sleep between them. For ~100k stale rows, that's ~100 batches × ~5s = 500s = 8min, just within timeout. If volumes grow beyond that, the runbook recommends paralleling by `agent_session_id` shards or shrinking the batch size.

---

## Out of Scope (deferred for now)

- Soft-delete with grace period (e.g., move to a "trash" table, restorable for 7 days).
- Per-agent or per-tenant TTL overrides.
- Metrics push to Cloud Monitoring (just logs for now).
- Parallel batch processing (sequential is fine at current volumes).
- A web UI / admin dashboard to inspect/restore deletions.
