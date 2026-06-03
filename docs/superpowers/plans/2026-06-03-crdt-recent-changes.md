# CRDT Documents — Recent Changes Awareness + Artifact Discovery — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every llm_call with `crdt_documents` config auto-aware of changes other peers made since the agent's last turn, and add discovery tools so the agent can list/create workbooks within a session.

**Architecture:** Three new SQL tables (`crdt_doc_events`, `crdt_doc_session_cursors`, `crdt_doc_session_artifacts`) + a `CrdtBackend` trait with two impls (`DirectBackend` for local/shared mode, `RestBackend` for ws_peer mode) + auto-injected summary block in `system_message` + two new tools (`crdt_doc_list_my_artifacts`, `crdt_doc_create_artifact`) + extended `crdt_doc_get_recent_changes`.

**Tech Stack:** Rust (sqlx for SQL, reqwest for HTTP, axum for new REST endpoints, async_trait), tokio. Existing colmena patterns (migrations in `migrations/{sqlite,postgres}/`, sqlx connection pool already in EngineConfig).

**Spec:** [`docs/superpowers/specs/2026-06-03-crdt-recent-changes-design.md`](../specs/2026-06-03-crdt-recent-changes-design.md)

---

## File map

| File | Action | Responsibility |
|---|---|---|
| `src/libs/colmena/migrations/sqlite/20260603000000_crdt_doc_changes.sql` | Create | SQLite schema for the 3 tables |
| `src/libs/colmena/migrations/postgres/20260603000000_crdt_doc_changes.sql` | Create | Postgres schema for the 3 tables |
| `src/libs/colmena/src/crdt_documents/change_tracker_store.rs` | Create | `ChangeTrackerStore` trait + `InMemoryChangeTrackerStore` + `SqlxChangeTrackerStore` |
| `src/libs/colmena/src/crdt_documents/crdt_backend.rs` | Create | `CrdtBackend` trait + `DirectBackend` (uses store) + `RestBackend` (HTTP) |
| `src/libs/colmena/src/crdt_documents/change_tracker.rs` | Modify | Refactor to wrap `ChangeTrackerStore` (preserve public API for callers) |
| `src/libs/colmena/src/crdt_documents/runtime.rs` | Modify | Add `store` field; wire from `DATABASE_URL` |
| `src/libs/colmena/src/crdt_documents/server.rs` | Modify | Add REST endpoints `GET /changes`, `GET /by-session/:sid`, `POST /cursor`; extend `POST /documents` |
| `src/libs/colmena/src/crdt_documents/ws_peer.rs` | Modify | Pass `?peer_type=agent&session_id=X` query string on connect |
| `src/libs/colmena/src/crdt_documents/mod.rs` | Modify | Export new modules |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_context.rs` | Modify | Add `session_id`, `backend`, `max_event_id_observed` AtomicU64 |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs` | Modify | Extend `get_recent_changes` args; add 2 new tools |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_summary.rs` | Create | Build the auto-injected `system_message` block |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` | Modify | Export new module/tools |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Modify | Inject summary block; update cursor post-loop; build CrdtDocsContext with session_id |
| `src/libs/colmena/tests/crdt_doc_recent_changes_test.rs` | Create | Integration test: full B end-to-end |
| `docs/developer_guide/38_crdt_documents.md` | Modify | Document new tools, auto-summary, 3 tables |
| `docs/node_configurations.json` | Modify | Add the 2 new tools' configs |
| `docs/BACKLOG.md` | Modify | Add deferred items (per-cell peer:browser attribution, paginación, etc.) |
| `docs/CHANGELOG_2026-06.md` | Create | New monthly changelog with B as first entry |

---

## Task 1: SQL migrations (sqlite + postgres)

**Files:**
- Create: `src/libs/colmena/migrations/sqlite/20260603000000_crdt_doc_changes.sql`
- Create: `src/libs/colmena/migrations/postgres/20260603000000_crdt_doc_changes.sql`

- [ ] **Step 1: Inspect existing migration files for dialect quirks**

Read `src/libs/colmena/migrations/sqlite/20260513000001_conversation_attachments.sql` and `src/libs/colmena/migrations/postgres/20260513000001_conversation_attachments.sql` to see exact patterns for `IF NOT EXISTS`, autoincrement column, timestamp default. Goal: match style.

- [ ] **Step 2: Write the SQLite migration**

```sql
-- src/libs/colmena/migrations/sqlite/20260603000000_crdt_doc_changes.sql

CREATE TABLE IF NOT EXISTS crdt_doc_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    artifact_id TEXT NOT NULL,
    sheet_id TEXT,
    origin TEXT NOT NULL,
    summary TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS crdt_doc_events_lookup
    ON crdt_doc_events(artifact_id, id);
CREATE INDEX IF NOT EXISTS crdt_doc_events_by_sheet
    ON crdt_doc_events(artifact_id, sheet_id, id);

CREATE TABLE IF NOT EXISTS crdt_doc_session_cursors (
    agent_session_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    last_event_id INTEGER NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (agent_session_id, artifact_id)
);

CREATE TABLE IF NOT EXISTS crdt_doc_session_artifacts (
    agent_session_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_accessed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (agent_session_id, artifact_id)
);
CREATE INDEX IF NOT EXISTS crdt_doc_session_artifacts_recent_idx
    ON crdt_doc_session_artifacts(agent_session_id, last_accessed_at DESC);
```

- [ ] **Step 3: Write the Postgres migration**

```sql
-- src/libs/colmena/migrations/postgres/20260603000000_crdt_doc_changes.sql

CREATE TABLE IF NOT EXISTS crdt_doc_events (
    id BIGSERIAL PRIMARY KEY,
    artifact_id TEXT NOT NULL,
    sheet_id TEXT,
    origin TEXT NOT NULL,
    summary TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS crdt_doc_events_lookup
    ON crdt_doc_events(artifact_id, id);
CREATE INDEX IF NOT EXISTS crdt_doc_events_by_sheet
    ON crdt_doc_events(artifact_id, sheet_id, id);

CREATE TABLE IF NOT EXISTS crdt_doc_session_cursors (
    agent_session_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    last_event_id BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (agent_session_id, artifact_id)
);

CREATE TABLE IF NOT EXISTS crdt_doc_session_artifacts (
    agent_session_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_accessed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (agent_session_id, artifact_id)
);
CREATE INDEX IF NOT EXISTS crdt_doc_session_artifacts_recent_idx
    ON crdt_doc_session_artifacts(agent_session_id, last_accessed_at DESC);
```

- [ ] **Step 4: Verify migrations parse (smoke build)**

Run: `cargo build --bin dag_engine`
Expected: build passes. Migrations are bundled into the binary via `sqlx::migrate!()` macro elsewhere in the codebase; build will fail if SQL syntax is wrong.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/migrations/sqlite/20260603000000_crdt_doc_changes.sql \
        src/libs/colmena/migrations/postgres/20260603000000_crdt_doc_changes.sql
git commit -m "feat(crdt_documents): migrations for events + cursors + session_artifacts (B-T1)"
```

---

## Task 2: `ChangeTrackerStore` trait + `InMemoryChangeTrackerStore`

**Files:**
- Create: `src/libs/colmena/src/crdt_documents/change_tracker_store.rs`
- Modify: `src/libs/colmena/src/crdt_documents/mod.rs`

- [ ] **Step 1: Write the failing tests first**

Append to `change_tracker_store.rs` (created in step 2):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::ArtifactId;

    fn make_event(artifact: &ArtifactId, origin: &str, summary: &str) -> NewEvent {
        NewEvent {
            artifact_id: artifact.clone(),
            sheet_id: Some("sh_test".to_string()),
            origin: origin.to_string(),
            summary: summary.to_string(),
        }
    }

    #[tokio::test]
    async fn in_memory_records_and_lists_events_in_order() {
        let store = InMemoryChangeTrackerStore::new();
        let aid = ArtifactId::new();
        let id1 = store.insert_event(make_event(&aid, "agent:s1", "a")).await.unwrap();
        let id2 = store.insert_event(make_event(&aid, "agent:s2", "b")).await.unwrap();
        assert!(id2 > id1);
        let evs = store.events_since(&aid, 0, None, None, 100).await.unwrap();
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].summary, "a");
        assert_eq!(evs[1].summary, "b");
    }

    #[tokio::test]
    async fn in_memory_filters_by_origin() {
        let store = InMemoryChangeTrackerStore::new();
        let aid = ArtifactId::new();
        store.insert_event(make_event(&aid, "agent:me", "mine")).await.unwrap();
        store.insert_event(make_event(&aid, "agent:other", "theirs")).await.unwrap();
        let evs = store.events_since(&aid, 0, None, Some("agent:me"), 100).await.unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].summary, "theirs");
    }

    #[tokio::test]
    async fn in_memory_filters_by_sheet() {
        let store = InMemoryChangeTrackerStore::new();
        let aid = ArtifactId::new();
        let mut ev_a = make_event(&aid, "agent:s1", "a");
        ev_a.sheet_id = Some("sh_a".into());
        let mut ev_b = make_event(&aid, "agent:s1", "b");
        ev_b.sheet_id = Some("sh_b".into());
        store.insert_event(ev_a).await.unwrap();
        store.insert_event(ev_b).await.unwrap();
        let evs = store.events_since(&aid, 0, Some("sh_a"), None, 100).await.unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].summary, "a");
    }

    #[tokio::test]
    async fn in_memory_caps_results_at_limit() {
        let store = InMemoryChangeTrackerStore::new();
        let aid = ArtifactId::new();
        for i in 0..10 {
            store.insert_event(make_event(&aid, "agent:s1", &format!("e{i}"))).await.unwrap();
        }
        let evs = store.events_since(&aid, 0, None, None, 3).await.unwrap();
        assert_eq!(evs.len(), 3);
    }

    #[tokio::test]
    async fn in_memory_cursor_upsert_and_lookup() {
        let store = InMemoryChangeTrackerStore::new();
        let aid = ArtifactId::new();
        assert_eq!(store.cursor_for("s1", &aid).await.unwrap(), None);
        store.upsert_cursor("s1", &aid, 42).await.unwrap();
        assert_eq!(store.cursor_for("s1", &aid).await.unwrap(), Some(42));
        store.upsert_cursor("s1", &aid, 100).await.unwrap();
        assert_eq!(store.cursor_for("s1", &aid).await.unwrap(), Some(100));
    }

    #[tokio::test]
    async fn in_memory_touch_artifact_then_list() {
        let store = InMemoryChangeTrackerStore::new();
        let a1 = ArtifactId::new();
        let a2 = ArtifactId::new();
        store.touch_artifact("s1", &a1, Some("first")).await.unwrap();
        store.touch_artifact("s1", &a2, Some("second")).await.unwrap();
        let list = store.artifacts_for_session("s1", 10).await.unwrap();
        assert_eq!(list.len(), 2);
        // Most recent first (a2 was touched last)
        assert_eq!(list[0].name, "second");
        assert_eq!(list[1].name, "first");
    }
}
```

- [ ] **Step 2: Write the trait + InMemory impl**

```rust
// src/libs/colmena/src/crdt_documents/change_tracker_store.rs

//! Storage adapter for the CRDT change tracker. Two implementations:
//! `InMemoryChangeTrackerStore` (tests + dev with no DB) and
//! `SqlxChangeTrackerStore` (Task 3, both SQLite + Postgres).

use crate::crdt_documents::ArtifactId;
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct NewEvent {
    pub artifact_id: ArtifactId,
    pub sheet_id: Option<String>,
    pub origin: String,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub id: u64,
    pub artifact_id: String,
    pub sheet_id: Option<String>,
    pub origin: String,
    pub summary: String,
    pub created_at: String, // ISO 8601 / TIMESTAMPTZ as string for portability
}

#[derive(Debug, Clone)]
pub struct StoredArtifact {
    pub artifact_id: String,
    pub name: String,
    pub created_at: String,
    pub last_accessed_at: String,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sql: {0}")]
    Sql(String),
}

#[async_trait]
pub trait ChangeTrackerStore: Send + Sync {
    async fn insert_event(&self, ev: NewEvent) -> Result<u64, StoreError>;

    async fn events_since(
        &self,
        artifact_id: &ArtifactId,
        since_event_id: u64,
        sheet_id_filter: Option<&str>,
        exclude_origin: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StoredEvent>, StoreError>;

    async fn cursor_for(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
    ) -> Result<Option<u64>, StoreError>;

    async fn upsert_cursor(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        last_event_id: u64,
    ) -> Result<(), StoreError>;

    async fn touch_artifact(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        name: Option<&str>,
    ) -> Result<(), StoreError>;

    async fn artifacts_for_session(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<StoredArtifact>, StoreError>;
}

// ── In-memory impl (tests + no-DB dev) ───────────────────────────────────

use std::sync::Mutex;

pub struct InMemoryChangeTrackerStore {
    inner: Mutex<InMemoryState>,
}

struct InMemoryState {
    events: Vec<StoredEvent>,
    next_id: u64,
    cursors: std::collections::HashMap<(String, String), u64>,
    artifacts: std::collections::HashMap<(String, String), StoredArtifact>,
}

impl Default for InMemoryChangeTrackerStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryChangeTrackerStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(InMemoryState {
                events: Vec::new(),
                next_id: 1,
                cursors: Default::default(),
                artifacts: Default::default(),
            }),
        }
    }
}

#[async_trait]
impl ChangeTrackerStore for InMemoryChangeTrackerStore {
    async fn insert_event(&self, ev: NewEvent) -> Result<u64, StoreError> {
        let mut g = self.inner.lock().unwrap();
        let id = g.next_id;
        g.next_id += 1;
        g.events.push(StoredEvent {
            id,
            artifact_id: ev.artifact_id.to_string(),
            sheet_id: ev.sheet_id,
            origin: ev.origin,
            summary: ev.summary,
            created_at: chrono_now_iso(),
        });
        Ok(id)
    }

    async fn events_since(
        &self,
        artifact_id: &ArtifactId,
        since_event_id: u64,
        sheet_id_filter: Option<&str>,
        exclude_origin: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        let g = self.inner.lock().unwrap();
        let aid_str = artifact_id.to_string();
        let mut out: Vec<StoredEvent> = g
            .events
            .iter()
            .filter(|e| e.artifact_id == aid_str)
            .filter(|e| e.id > since_event_id)
            .filter(|e| match sheet_id_filter {
                Some(s) => e.sheet_id.as_deref() == Some(s),
                None => true,
            })
            .filter(|e| match exclude_origin {
                Some(s) => e.origin != s,
                None => true,
            })
            .take(limit as usize)
            .cloned()
            .collect();
        out.sort_by_key(|e| e.id);
        Ok(out)
    }

    async fn cursor_for(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
    ) -> Result<Option<u64>, StoreError> {
        let g = self.inner.lock().unwrap();
        Ok(g.cursors
            .get(&(session_id.to_string(), artifact_id.to_string()))
            .copied())
    }

    async fn upsert_cursor(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        last_event_id: u64,
    ) -> Result<(), StoreError> {
        let mut g = self.inner.lock().unwrap();
        g.cursors
            .insert((session_id.to_string(), artifact_id.to_string()), last_event_id);
        Ok(())
    }

    async fn touch_artifact(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        name: Option<&str>,
    ) -> Result<(), StoreError> {
        let mut g = self.inner.lock().unwrap();
        let key = (session_id.to_string(), artifact_id.to_string());
        let now = chrono_now_iso();
        g.artifacts
            .entry(key)
            .and_modify(|a| {
                a.last_accessed_at = now.clone();
                if let Some(n) = name {
                    a.name = n.to_string();
                }
            })
            .or_insert(StoredArtifact {
                artifact_id: artifact_id.to_string(),
                name: name.unwrap_or("(untitled)").to_string(),
                created_at: now.clone(),
                last_accessed_at: now,
            });
        Ok(())
    }

    async fn artifacts_for_session(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<StoredArtifact>, StoreError> {
        let g = self.inner.lock().unwrap();
        let mut out: Vec<StoredArtifact> = g
            .artifacts
            .iter()
            .filter(|((sid, _), _)| sid == session_id)
            .map(|(_, a)| a.clone())
            .collect();
        out.sort_by(|a, b| b.last_accessed_at.cmp(&a.last_accessed_at));
        out.truncate(limit as usize);
        Ok(out)
    }
}

fn chrono_now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

// Re-export Arc<dyn Trait> alias for convenience.
pub type ChangeTrackerStoreRef = Arc<dyn ChangeTrackerStore>;
```

- [ ] **Step 3: Wire export in `mod.rs`**

Add to `src/libs/colmena/src/crdt_documents/mod.rs` after the existing `pub mod` declarations:

```rust
pub mod change_tracker_store;
```

And add to the re-exports section:

```rust
pub use change_tracker_store::{
    ChangeTrackerStore, ChangeTrackerStoreRef, InMemoryChangeTrackerStore,
    NewEvent, StoredArtifact, StoredEvent, StoreError as ChangeTrackerStoreError,
};
```

- [ ] **Step 4: Run tests**

```bash
cargo test --lib -p colmena_dag_engine change_tracker_store
```
Expected: 6 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/change_tracker_store.rs \
        src/libs/colmena/src/crdt_documents/mod.rs
git commit -m "feat(crdt_documents): ChangeTrackerStore trait + InMemory impl (B-T2)"
```

---

## Task 3: SQLx `ChangeTrackerStore` impl (SQLite + Postgres)

**Files:**
- Modify: `src/libs/colmena/src/crdt_documents/change_tracker_store.rs`

- [ ] **Step 1: Check existing sqlx pool pattern in colmena**

Run: `grep -rn "sqlx::Pool\|sqlx::Any\|sqlx::Sqlite\|sqlx::Postgres" src/libs/colmena/src/ | head -10`
Identify the existing pool type used by `llm_node_history` or `conversation_attachments` repos — use the same.

- [ ] **Step 2: Append the SQLx impl to `change_tracker_store.rs`**

Append at the bottom of the file (before the `#[cfg(test)]` module):

```rust
// ── SQLx impl (sqlite + postgres via sqlx::Any) ──────────────────────────

use sqlx::{any::AnyPool, Row};

pub struct SqlxChangeTrackerStore {
    pool: AnyPool,
}

impl SqlxChangeTrackerStore {
    pub fn new(pool: AnyPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ChangeTrackerStore for SqlxChangeTrackerStore {
    async fn insert_event(&self, ev: NewEvent) -> Result<u64, StoreError> {
        // INSERT ... RETURNING differs between sqlite and postgres. Use a
        // two-step pattern that works on both: insert, then SELECT last_insert.
        // For sqlite: `SELECT last_insert_rowid()`. For postgres: `RETURNING id`.
        // sqlx::Any doesn't support RETURNING on sqlite reliably, so we split.
        let kind = self.pool.connect_options().database_url().to_string();
        let aid = ev.artifact_id.to_string();

        if kind.starts_with("postgres") || kind.starts_with("postgresql") {
            let row = sqlx::query(
                "INSERT INTO crdt_doc_events (artifact_id, sheet_id, origin, summary) \
                 VALUES ($1, $2, $3, $4) RETURNING id",
            )
            .bind(&aid)
            .bind(&ev.sheet_id)
            .bind(&ev.origin)
            .bind(&ev.summary)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Sql(e.to_string()))?;
            let id: i64 = row.try_get("id").map_err(|e| StoreError::Sql(e.to_string()))?;
            Ok(id as u64)
        } else {
            // sqlite
            sqlx::query(
                "INSERT INTO crdt_doc_events (artifact_id, sheet_id, origin, summary) \
                 VALUES (?, ?, ?, ?)",
            )
            .bind(&aid)
            .bind(&ev.sheet_id)
            .bind(&ev.origin)
            .bind(&ev.summary)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Sql(e.to_string()))?;
            let row = sqlx::query("SELECT last_insert_rowid() as id")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| StoreError::Sql(e.to_string()))?;
            let id: i64 = row.try_get("id").map_err(|e| StoreError::Sql(e.to_string()))?;
            Ok(id as u64)
        }
    }

    async fn events_since(
        &self,
        artifact_id: &ArtifactId,
        since_event_id: u64,
        sheet_id_filter: Option<&str>,
        exclude_origin: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        // Build query dynamically because the WHERE clauses are optional.
        let mut sql = String::from(
            "SELECT id, artifact_id, sheet_id, origin, summary, created_at \
             FROM crdt_doc_events WHERE artifact_id = ? AND id > ?",
        );
        if sheet_id_filter.is_some() {
            sql.push_str(" AND sheet_id = ?");
        }
        if exclude_origin.is_some() {
            sql.push_str(" AND origin != ?");
        }
        sql.push_str(" ORDER BY id ASC LIMIT ?");

        // Placeholder substitution for postgres ($N vs ?).
        let kind = self.pool.connect_options().database_url().to_string();
        let final_sql = if kind.starts_with("postgres") {
            // Replace ? with $1, $2, ... in order.
            let mut idx = 0;
            sql.chars()
                .map(|c| {
                    if c == '?' {
                        idx += 1;
                        format!("${idx}")
                    } else {
                        c.to_string()
                    }
                })
                .collect::<String>()
        } else {
            sql
        };

        let mut q = sqlx::query(&final_sql)
            .bind(artifact_id.to_string())
            .bind(since_event_id as i64);
        if let Some(s) = sheet_id_filter {
            q = q.bind(s.to_string());
        }
        if let Some(s) = exclude_origin {
            q = q.bind(s.to_string());
        }
        q = q.bind(limit as i64);

        let rows = q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Sql(e.to_string()))?;

        let events = rows
            .into_iter()
            .map(|r| StoredEvent {
                id: r.try_get::<i64, _>("id").unwrap_or(0) as u64,
                artifact_id: r.try_get("artifact_id").unwrap_or_default(),
                sheet_id: r.try_get("sheet_id").ok(),
                origin: r.try_get("origin").unwrap_or_default(),
                summary: r.try_get("summary").unwrap_or_default(),
                created_at: r
                    .try_get::<String, _>("created_at")
                    .unwrap_or_else(|_| String::new()),
            })
            .collect();
        Ok(events)
    }

    async fn cursor_for(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
    ) -> Result<Option<u64>, StoreError> {
        let kind = self.pool.connect_options().database_url().to_string();
        let sql = if kind.starts_with("postgres") {
            "SELECT last_event_id FROM crdt_doc_session_cursors \
             WHERE agent_session_id = $1 AND artifact_id = $2"
        } else {
            "SELECT last_event_id FROM crdt_doc_session_cursors \
             WHERE agent_session_id = ? AND artifact_id = ?"
        };
        let row = sqlx::query(sql)
            .bind(session_id)
            .bind(artifact_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::Sql(e.to_string()))?;
        Ok(row.map(|r| r.try_get::<i64, _>("last_event_id").unwrap_or(0) as u64))
    }

    async fn upsert_cursor(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        last_event_id: u64,
    ) -> Result<(), StoreError> {
        let kind = self.pool.connect_options().database_url().to_string();
        let sql = if kind.starts_with("postgres") {
            "INSERT INTO crdt_doc_session_cursors \
             (agent_session_id, artifact_id, last_event_id) VALUES ($1, $2, $3) \
             ON CONFLICT (agent_session_id, artifact_id) DO UPDATE \
             SET last_event_id = EXCLUDED.last_event_id, updated_at = now()"
        } else {
            "INSERT INTO crdt_doc_session_cursors \
             (agent_session_id, artifact_id, last_event_id) VALUES (?, ?, ?) \
             ON CONFLICT (agent_session_id, artifact_id) DO UPDATE \
             SET last_event_id = excluded.last_event_id, \
                 updated_at = CURRENT_TIMESTAMP"
        };
        sqlx::query(sql)
            .bind(session_id)
            .bind(artifact_id.to_string())
            .bind(last_event_id as i64)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Sql(e.to_string()))?;
        Ok(())
    }

    async fn touch_artifact(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        name: Option<&str>,
    ) -> Result<(), StoreError> {
        let name_or_default = name.unwrap_or("(untitled)");
        let kind = self.pool.connect_options().database_url().to_string();
        let sql = if kind.starts_with("postgres") {
            "INSERT INTO crdt_doc_session_artifacts \
             (agent_session_id, artifact_id, name) VALUES ($1, $2, $3) \
             ON CONFLICT (agent_session_id, artifact_id) DO UPDATE \
             SET last_accessed_at = now(), name = EXCLUDED.name"
        } else {
            "INSERT INTO crdt_doc_session_artifacts \
             (agent_session_id, artifact_id, name) VALUES (?, ?, ?) \
             ON CONFLICT (agent_session_id, artifact_id) DO UPDATE \
             SET last_accessed_at = CURRENT_TIMESTAMP, name = excluded.name"
        };
        sqlx::query(sql)
            .bind(session_id)
            .bind(artifact_id.to_string())
            .bind(name_or_default)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Sql(e.to_string()))?;
        Ok(())
    }

    async fn artifacts_for_session(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<StoredArtifact>, StoreError> {
        let kind = self.pool.connect_options().database_url().to_string();
        let sql = if kind.starts_with("postgres") {
            "SELECT artifact_id, name, created_at, last_accessed_at \
             FROM crdt_doc_session_artifacts WHERE agent_session_id = $1 \
             ORDER BY last_accessed_at DESC LIMIT $2"
        } else {
            "SELECT artifact_id, name, created_at, last_accessed_at \
             FROM crdt_doc_session_artifacts WHERE agent_session_id = ? \
             ORDER BY last_accessed_at DESC LIMIT ?"
        };
        let rows = sqlx::query(sql)
            .bind(session_id)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Sql(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|r| StoredArtifact {
                artifact_id: r.try_get("artifact_id").unwrap_or_default(),
                name: r.try_get("name").unwrap_or_default(),
                created_at: r.try_get::<String, _>("created_at").unwrap_or_default(),
                last_accessed_at: r
                    .try_get::<String, _>("last_accessed_at")
                    .unwrap_or_default(),
            })
            .collect())
    }
}
```

- [ ] **Step 3: Add SQLite integration test**

Append to the `tests` module in `change_tracker_store.rs`:

```rust
#[tokio::test]
async fn sqlx_sqlite_round_trip() {
    use sqlx::any::{AnyConnectOptions, AnyPoolOptions};
    use std::str::FromStr;

    sqlx::any::install_default_drivers();
    let url = format!("sqlite::memory:");
    let opts = AnyConnectOptions::from_str(&url).unwrap();
    let pool = AnyPoolOptions::new().connect_with(opts).await.unwrap();
    sqlx::migrate!("./migrations/sqlite")
        .run(&pool)
        .await
        .unwrap();
    let store = SqlxChangeTrackerStore::new(pool);
    let aid = ArtifactId::new();
    let id = store
        .insert_event(make_event(&aid, "agent:s1", "test"))
        .await
        .unwrap();
    assert!(id > 0);
    let evs = store.events_since(&aid, 0, None, None, 10).await.unwrap();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].summary, "test");

    store.upsert_cursor("s1", &aid, 42).await.unwrap();
    assert_eq!(store.cursor_for("s1", &aid).await.unwrap(), Some(42));

    store
        .touch_artifact("s1", &aid, Some("My Sheet"))
        .await
        .unwrap();
    let arts = store.artifacts_for_session("s1", 10).await.unwrap();
    assert_eq!(arts.len(), 1);
    assert_eq!(arts[0].name, "My Sheet");
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test --lib -p colmena_dag_engine change_tracker_store
```
Expected: 7 tests pass (6 in-memory + 1 sqlx_sqlite).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/change_tracker_store.rs
git commit -m "feat(crdt_documents): SqlxChangeTrackerStore for sqlite+postgres (B-T3)"
```

---

## Task 4: Refactor `ChangeTracker` to use `ChangeTrackerStore`

**Files:**
- Modify: `src/libs/colmena/src/crdt_documents/change_tracker.rs`

- [ ] **Step 1: Read the current `ChangeTracker` to understand callers**

Run: `cat src/libs/colmena/src/crdt_documents/change_tracker.rs`
Note the public API: `record(artifact_id, origin, summary)`, `since(artifact_id, since_event_id)`, `ChangeEvent` struct.

- [ ] **Step 2: Rewrite as wrapper preserving public API**

```rust
// src/libs/colmena/src/crdt_documents/change_tracker.rs

//! Façade over `ChangeTrackerStore`. Preserves the v1 public API
//! (`record`, `since`, `ChangeEvent`) so existing call sites keep working.
//! Behind the scenes, every operation goes through the configured store
//! (in-memory or SQL).

use crate::crdt_documents::{
    change_tracker_store::{ChangeTrackerStore, NewEvent, StoredEvent},
    ArtifactId,
};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ChangeEvent {
    pub event_id: u64,
    pub artifact_id: String,
    pub sheet_id: Option<String>,
    pub origin: String,
    pub summary: String,
    pub created_at: String,
}

impl From<StoredEvent> for ChangeEvent {
    fn from(e: StoredEvent) -> Self {
        ChangeEvent {
            event_id: e.id,
            artifact_id: e.artifact_id,
            sheet_id: e.sheet_id,
            origin: e.origin,
            summary: e.summary,
            created_at: e.created_at,
        }
    }
}

pub struct ChangeTracker {
    store: Arc<dyn ChangeTrackerStore>,
}

impl ChangeTracker {
    pub fn new(store: Arc<dyn ChangeTrackerStore>) -> Self {
        Self { store }
    }

    /// Record an event. v1 callers passed `(artifact_id, origin, summary)`;
    /// v2 extends to optional sheet_id. Returns the event id (used by
    /// `CrdtDocsContext::max_event_id_observed` to track the turn high-water).
    pub async fn record(
        &self,
        artifact_id: &ArtifactId,
        sheet_id: Option<&str>,
        origin: &str,
        summary: &str,
    ) -> u64 {
        self.store
            .insert_event(NewEvent {
                artifact_id: artifact_id.clone(),
                sheet_id: sheet_id.map(|s| s.to_string()),
                origin: origin.to_string(),
                summary: summary.to_string(),
            })
            .await
            .unwrap_or(0)
    }

    /// Query events after a cursor. Optional filters by sheet and excluded origin.
    pub async fn since(
        &self,
        artifact_id: &ArtifactId,
        since_event_id: Option<u64>,
        sheet_id: Option<&str>,
        exclude_origin: Option<&str>,
        limit: u32,
    ) -> Vec<ChangeEvent> {
        let cursor = since_event_id.unwrap_or(0);
        self.store
            .events_since(artifact_id, cursor, sheet_id, exclude_origin, limit)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(ChangeEvent::from)
            .collect()
    }
}
```

- [ ] **Step 3: Update existing callers (registry, server, tools)**

Run: `grep -rn "tracker.record\|tracker\.since" src/libs/colmena/src/`
For each call site, the signature changed:
- `tracker.record(artifact_id, origin, summary)` → `tracker.record(artifact_id, None, origin, summary).await`
- `tracker.since(artifact_id, since)` → `tracker.since(artifact_id, since, None, None, 100).await`

These are minimal mechanical changes. Touch each file, add `.await`, add `None` for the new params where you don't have the info yet (sheet_id and exclude_origin become explicit in later tasks).

The callers should also be `async fn`s already (the snapshot writer + tool dispatchers); if any are sync, mark them async and adapt the caller.

- [ ] **Step 4: Build to find any broken callers**

```bash
cargo build --bin dag_engine 2>&1 | tail -30
```
Fix any compile errors by adding `.await` or adapting the calling function to async.

- [ ] **Step 5: Run all existing crdt_documents tests**

```bash
cargo test --lib -p colmena_dag_engine crdt_documents
```
Expected: 41+ tests pass (existing + 7 new from Task 2/3).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/change_tracker.rs
git add <any caller files that were touched>
git commit -m "refactor(crdt_documents): ChangeTracker over ChangeTrackerStore + async (B-T4)"
```

---

## Task 5: `CrdtBackend` trait + DirectBackend + RestBackend

**Files:**
- Create: `src/libs/colmena/src/crdt_documents/crdt_backend.rs`
- Modify: `src/libs/colmena/src/crdt_documents/mod.rs`

- [ ] **Step 1: Write the DirectBackend tests first**

In a new file:

```rust
// src/libs/colmena/src/crdt_documents/crdt_backend.rs

//! Abstraction over "where SQL queries land" for the recent-changes
//! subsystem. Two impls: `DirectBackend` (local/shared mode → direct
//! `ChangeTrackerStore`) and `RestBackend` (ws_peer mode → HTTP to the
//! CRDT documents server).

use crate::crdt_documents::{
    change_tracker_store::{
        ChangeTrackerStore, NewEvent, StoredArtifact, StoredEvent, StoreError,
    },
    ArtifactId,
};
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("http: {0}")]
    Http(String),
    #[error("decode: {0}")]
    Decode(String),
}

#[async_trait]
pub trait CrdtBackend: Send + Sync {
    async fn record_event(&self, ev: NewEvent) -> Result<u64, BackendError>;

    async fn events_since(
        &self,
        artifact_id: &ArtifactId,
        since_event_id: u64,
        sheet_id_filter: Option<&str>,
        exclude_origin: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StoredEvent>, BackendError>;

    async fn upsert_cursor(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        last_event_id: u64,
    ) -> Result<(), BackendError>;

    async fn cursor_for(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
    ) -> Result<Option<u64>, BackendError>;

    async fn artifacts_for_session(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<StoredArtifact>, BackendError>;

    async fn touch_artifact(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        name: Option<&str>,
    ) -> Result<(), BackendError>;
}

// ── DirectBackend ────────────────────────────────────────────────────────

pub struct DirectBackend {
    pub store: Arc<dyn ChangeTrackerStore>,
}

#[async_trait]
impl CrdtBackend for DirectBackend {
    async fn record_event(&self, ev: NewEvent) -> Result<u64, BackendError> {
        Ok(self.store.insert_event(ev).await?)
    }
    async fn events_since(
        &self,
        artifact_id: &ArtifactId,
        since_event_id: u64,
        sheet_id_filter: Option<&str>,
        exclude_origin: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StoredEvent>, BackendError> {
        Ok(self
            .store
            .events_since(artifact_id, since_event_id, sheet_id_filter, exclude_origin, limit)
            .await?)
    }
    async fn upsert_cursor(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        last_event_id: u64,
    ) -> Result<(), BackendError> {
        Ok(self.store.upsert_cursor(session_id, artifact_id, last_event_id).await?)
    }
    async fn cursor_for(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
    ) -> Result<Option<u64>, BackendError> {
        Ok(self.store.cursor_for(session_id, artifact_id).await?)
    }
    async fn artifacts_for_session(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<StoredArtifact>, BackendError> {
        Ok(self.store.artifacts_for_session(session_id, limit).await?)
    }
    async fn touch_artifact(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        name: Option<&str>,
    ) -> Result<(), BackendError> {
        Ok(self.store.touch_artifact(session_id, artifact_id, name).await?)
    }
}

// ── RestBackend ──────────────────────────────────────────────────────────

pub struct RestBackend {
    pub client: reqwest::Client,
    /// Base URL like `http://crdt-service:8090` (NO trailing slash).
    pub base_url: String,
}

impl RestBackend {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }
}

#[async_trait]
impl CrdtBackend for RestBackend {
    async fn record_event(&self, ev: NewEvent) -> Result<u64, BackendError> {
        let url = format!("{}/documents/{}/events", self.base_url, ev.artifact_id);
        let body = serde_json::json!({
            "sheet_id": ev.sheet_id,
            "origin": ev.origin,
            "summary": ev.summary,
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| BackendError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(BackendError::Http(format!("status {}", resp.status())));
        }
        let val: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| BackendError::Decode(e.to_string()))?;
        Ok(val["id"].as_u64().unwrap_or(0))
    }

    async fn events_since(
        &self,
        artifact_id: &ArtifactId,
        since_event_id: u64,
        sheet_id_filter: Option<&str>,
        exclude_origin: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StoredEvent>, BackendError> {
        let mut url = format!(
            "{}/documents/{}/changes?since={}&limit={}",
            self.base_url, artifact_id, since_event_id, limit
        );
        if let Some(s) = sheet_id_filter {
            url.push_str(&format!("&sheet_id={s}"));
        }
        if let Some(s) = exclude_origin {
            url.push_str(&format!("&exclude_origin={s}"));
        }
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| BackendError::Http(e.to_string()))?;
        let val: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| BackendError::Decode(e.to_string()))?;
        let events = val["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|e| StoredEvent {
                id: e["id"].as_u64().unwrap_or(0),
                artifact_id: e["artifact_id"].as_str().unwrap_or("").to_string(),
                sheet_id: e["sheet_id"].as_str().map(String::from),
                origin: e["origin"].as_str().unwrap_or("").to_string(),
                summary: e["summary"].as_str().unwrap_or("").to_string(),
                created_at: e["created_at"].as_str().unwrap_or("").to_string(),
            })
            .collect();
        Ok(events)
    }

    async fn upsert_cursor(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        last_event_id: u64,
    ) -> Result<(), BackendError> {
        let url = format!("{}/documents/{}/cursor", self.base_url, artifact_id);
        let body = serde_json::json!({
            "agent_session_id": session_id,
            "last_event_id": last_event_id,
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| BackendError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(BackendError::Http(format!("status {}", resp.status())));
        }
        Ok(())
    }

    async fn cursor_for(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
    ) -> Result<Option<u64>, BackendError> {
        let url = format!(
            "{}/documents/{}/cursor?agent_session_id={}",
            self.base_url, artifact_id, session_id
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| BackendError::Http(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let val: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| BackendError::Decode(e.to_string()))?;
        Ok(val["last_event_id"].as_u64())
    }

    async fn artifacts_for_session(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<StoredArtifact>, BackendError> {
        let url = format!(
            "{}/documents/by-session/{}?limit={}",
            self.base_url, session_id, limit
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| BackendError::Http(e.to_string()))?;
        let val: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| BackendError::Decode(e.to_string()))?;
        Ok(val["artifacts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|a| StoredArtifact {
                artifact_id: a["artifact_id"].as_str().unwrap_or("").to_string(),
                name: a["name"].as_str().unwrap_or("").to_string(),
                created_at: a["created_at"].as_str().unwrap_or("").to_string(),
                last_accessed_at: a["last_accessed_at"].as_str().unwrap_or("").to_string(),
            })
            .collect())
    }

    async fn touch_artifact(
        &self,
        _session_id: &str,
        _artifact_id: &ArtifactId,
        _name: Option<&str>,
    ) -> Result<(), BackendError> {
        // ws_peer mode: touch is done by the server on POST /documents
        // (which includes agent_session_id in the body). The client
        // explicitly setting touch is a no-op.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::change_tracker_store::InMemoryChangeTrackerStore;

    #[tokio::test]
    async fn direct_backend_records_and_queries() {
        let store: Arc<dyn ChangeTrackerStore> = Arc::new(InMemoryChangeTrackerStore::new());
        let backend = DirectBackend { store };
        let aid = ArtifactId::new();
        let id = backend
            .record_event(NewEvent {
                artifact_id: aid.clone(),
                sheet_id: Some("sh_test".to_string()),
                origin: "agent:s1".to_string(),
                summary: "hello".to_string(),
            })
            .await
            .unwrap();
        assert!(id > 0);
        let evs = backend
            .events_since(&aid, 0, None, None, 10)
            .await
            .unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].summary, "hello");
    }
}
```

- [ ] **Step 2: Wire export**

In `src/libs/colmena/src/crdt_documents/mod.rs`:

```rust
pub mod crdt_backend;

pub use crdt_backend::{BackendError, CrdtBackend, DirectBackend, RestBackend};
```

- [ ] **Step 3: Run tests**

```bash
cargo test --lib -p colmena_dag_engine crdt_backend
```
Expected: 1 test passes.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/crdt_backend.rs \
        src/libs/colmena/src/crdt_documents/mod.rs
git commit -m "feat(crdt_documents): CrdtBackend trait + Direct + Rest impls (B-T5)"
```

---

## Task 6: Wire SQL store into `CrdtDocumentsRuntime`

**Files:**
- Modify: `src/libs/colmena/src/crdt_documents/runtime.rs`

- [ ] **Step 1: Add store field and from_config wiring**

Replace the existing `pub struct CrdtDocumentsRuntime` block + `from_config` method:

```rust
pub struct CrdtDocumentsRuntime {
    pub registry: Arc<DocRegistry>,
    pub storage: Arc<dyn ArtifactStorage>,
    pub tracker: Arc<crate::crdt_documents::ChangeTracker>,
    pub store: Arc<dyn crate::crdt_documents::ChangeTrackerStore>,
}

impl CrdtDocumentsRuntime {
    pub async fn from_config(cfg: &Value) -> Result<Self, RuntimeError> {
        // ... existing backend selection logic (localfs/gcs) unchanged ...
        // (keep all the existing code up to where `storage` is built)

        let storage: Arc<dyn ArtifactStorage> = storage_cfg.build()?;
        let registry = Arc::new(DocRegistry::new(storage.clone()));
        let _ = registry.load_from_disk().await?;

        // NEW: build the change tracker store.
        // If `database_url` in config OR DATABASE_URL env → SqlxChangeTrackerStore.
        // Otherwise → InMemoryChangeTrackerStore (dev with no DB).
        let database_url = cfg
            .get("database_url")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| std::env::var("DATABASE_URL").ok());

        let store: Arc<dyn crate::crdt_documents::ChangeTrackerStore> = match database_url {
            Some(url) => {
                use sqlx::any::{AnyConnectOptions, AnyPoolOptions};
                use std::str::FromStr;
                sqlx::any::install_default_drivers();
                let opts = AnyConnectOptions::from_str(&url).map_err(|e| {
                    RuntimeError::Config(format!("invalid DATABASE_URL: {e}"))
                })?;
                let pool = AnyPoolOptions::new()
                    .connect_with(opts)
                    .await
                    .map_err(|e| RuntimeError::Config(format!("db connect: {e}")))?;
                // Run migrations. The migration files live under
                // src/libs/colmena/migrations/{sqlite,postgres}/ and are
                // bundled by sqlx::migrate! into the binary.
                let kind = if url.starts_with("postgres") {
                    "postgres"
                } else {
                    "sqlite"
                };
                let migrator = if kind == "postgres" {
                    sqlx::migrate!("./migrations/postgres")
                } else {
                    sqlx::migrate!("./migrations/sqlite")
                };
                migrator
                    .run(&pool)
                    .await
                    .map_err(|e| RuntimeError::Config(format!("migrate: {e}")))?;
                Arc::new(crate::crdt_documents::change_tracker_store::SqlxChangeTrackerStore::new(pool))
            }
            None => Arc::new(crate::crdt_documents::InMemoryChangeTrackerStore::new()),
        };
        let tracker = Arc::new(crate::crdt_documents::ChangeTracker::new(store.clone()));

        Ok(Self {
            registry,
            storage,
            tracker,
            store,
        })
    }
    // ... shutdown() stays as-is from V2 ...
}
```

- [ ] **Step 2: Build and fix any remaining caller breakage**

```bash
cargo build --bin dag_engine 2>&1 | tail -20
```
Likely: `ChangeTracker::new(/* old: no args */)` calls might break. Update to `ChangeTracker::new(store.clone())`. The `tracker` field type is unchanged so most callers don't care.

- [ ] **Step 3: Run all crdt_documents tests**

```bash
cargo test --lib -p colmena_dag_engine crdt_documents
```
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/runtime.rs
git commit -m "feat(crdt_documents): wire ChangeTrackerStore into runtime via DATABASE_URL (B-T6)"
```

---

## Task 7: New REST endpoints on the server

**Files:**
- Modify: `src/libs/colmena/src/crdt_documents/server.rs`

- [ ] **Step 1: Inspect the existing server router structure**

Run: `grep -n "fn router\|.route(" src/libs/colmena/src/crdt_documents/server.rs | head -20`
Note where existing routes are registered (`/documents`, `/yjs/:id`, etc.).

- [ ] **Step 2: Add the four new endpoints**

In `server.rs` `router()`:

```rust
// (existing routes ...)
.route("/documents/:id/changes", get(changes_handler))
.route("/documents/:id/events", post(record_event_handler))
.route("/documents/:id/cursor", get(get_cursor_handler).post(set_cursor_handler))
.route("/documents/by-session/:sid", get(by_session_handler))
```

Modify the existing `POST /documents` handler to also accept `agent_session_id` in the body and call `runtime.store.touch_artifact(...)` if present.

Add the handler implementations:

```rust
async fn changes_handler(
    Path(id_str): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
) -> Response {
    let id = match ArtifactId::from_str(&id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid artifact id").into_response(),
    };
    let since: u64 = params.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);
    let limit: u32 = params.get("limit").and_then(|s| s.parse().ok()).unwrap_or(50);
    let sheet_id = params.get("sheet_id").map(String::as_str);
    let exclude_origin = params.get("exclude_origin").map(String::as_str);
    match runtime
        .store
        .events_since(&id, since, sheet_id, exclude_origin, limit)
        .await
    {
        Ok(evs) => {
            let max_id = evs.iter().map(|e| e.id).max().unwrap_or(since);
            Json(serde_json::json!({
                "current_event_id": max_id,
                "events": evs.iter().map(|e| serde_json::json!({
                    "id": e.id, "artifact_id": e.artifact_id,
                    "sheet_id": e.sheet_id, "origin": e.origin,
                    "summary": e.summary, "created_at": e.created_at,
                })).collect::<Vec<_>>(),
                "truncated": (evs.len() as u32) >= limit,
            })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct RecordEventBody {
    sheet_id: Option<String>,
    origin: String,
    summary: String,
}

async fn record_event_handler(
    Path(id_str): Path<String>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
    Json(body): Json<RecordEventBody>,
) -> Response {
    let id = match ArtifactId::from_str(&id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid artifact id").into_response(),
    };
    match runtime
        .store
        .insert_event(NewEvent {
            artifact_id: id,
            sheet_id: body.sheet_id,
            origin: body.origin,
            summary: body.summary,
        })
        .await
    {
        Ok(event_id) => Json(serde_json::json!({"id": event_id})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct CursorBody {
    agent_session_id: String,
    last_event_id: u64,
}

async fn set_cursor_handler(
    Path(id_str): Path<String>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
    Json(body): Json<CursorBody>,
) -> Response {
    let id = match ArtifactId::from_str(&id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid artifact id").into_response(),
    };
    match runtime
        .store
        .upsert_cursor(&body.agent_session_id, &id, body.last_event_id)
        .await
    {
        Ok(()) => (StatusCode::OK, "").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn get_cursor_handler(
    Path(id_str): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
) -> Response {
    let id = match ArtifactId::from_str(&id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid artifact id").into_response(),
    };
    let sid = match params.get("agent_session_id") {
        Some(s) => s,
        None => return (StatusCode::BAD_REQUEST, "agent_session_id required").into_response(),
    };
    match runtime.store.cursor_for(sid, &id).await {
        Ok(Some(c)) => Json(serde_json::json!({"last_event_id": c})).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no cursor").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn by_session_handler(
    Path(sid): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
) -> Response {
    let limit: u32 = params.get("limit").and_then(|s| s.parse().ok()).unwrap_or(50);
    match runtime.store.artifacts_for_session(&sid, limit).await {
        Ok(list) => Json(serde_json::json!({
            "artifacts": list.iter().map(|a| serde_json::json!({
                "artifact_id": a.artifact_id, "name": a.name,
                "created_at": a.created_at, "last_accessed_at": a.last_accessed_at,
            })).collect::<Vec<_>>(),
        })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}
```

Add `serde::Deserialize` to the existing `POST /documents` body struct:

```rust
#[derive(serde::Deserialize)]
struct CreateDocumentBody {
    name: Option<String>,
    agent_session_id: Option<String>,
}
```

And in that handler, after the artifact is created, conditionally:

```rust
if let Some(sid) = body.agent_session_id.as_ref() {
    let _ = runtime.store.touch_artifact(sid, &new_id, body.name.as_deref()).await;
}
```

- [ ] **Step 3: Add an integration test for the new endpoints**

In `src/libs/colmena/tests/crdt_documents_rest_test.rs` (existing file), add:

```rust
#[tokio::test]
async fn record_event_then_query_changes() {
    // (Use the existing helper from this file to spin up a test server.)
    let (base_url, _runtime, _tmp) = spawn_test_server().await;
    let client = reqwest::Client::new();

    // Create an artifact.
    let r: serde_json::Value = client
        .post(format!("{}/documents", base_url))
        .json(&serde_json::json!({"name": "X", "agent_session_id": "s1"}))
        .send().await.unwrap().json().await.unwrap();
    let aid = r["artifact_id"].as_str().unwrap();

    // POST an event.
    let r: serde_json::Value = client
        .post(format!("{}/documents/{}/events", base_url, aid))
        .json(&serde_json::json!({
            "sheet_id": "sh_a", "origin": "agent:s2", "summary": "set A1 = 42"
        }))
        .send().await.unwrap().json().await.unwrap();
    assert!(r["id"].as_u64().unwrap() > 0);

    // Query changes since 0.
    let r: serde_json::Value = client
        .get(format!("{}/documents/{}/changes?since=0", base_url, aid))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(r["events"].as_array().unwrap().len(), 1);

    // Filter by exclude_origin.
    let r: serde_json::Value = client
        .get(format!("{}/documents/{}/changes?since=0&exclude_origin=agent:s2", base_url, aid))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(r["events"].as_array().unwrap().len(), 0);

    // Cursor round-trip.
    client
        .post(format!("{}/documents/{}/cursor", base_url, aid))
        .json(&serde_json::json!({"agent_session_id": "s1", "last_event_id": 1}))
        .send().await.unwrap();
    let r: serde_json::Value = client
        .get(format!("{}/documents/{}/cursor?agent_session_id=s1", base_url, aid))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(r["last_event_id"].as_u64().unwrap(), 1);

    // by-session lists the artifact.
    let r: serde_json::Value = client
        .get(format!("{}/documents/by-session/s1", base_url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(r["artifacts"].as_array().unwrap().len(), 1);
}
```

If `spawn_test_server` doesn't exist, check the existing test file for its helper or create one inline. The pattern is the same as `tests/crdt_ws_peer_full_tools_test.rs`.

- [ ] **Step 4: Run the test**

```bash
cargo test --test crdt_documents_rest_test record_event_then_query_changes
```
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/server.rs \
        src/libs/colmena/tests/crdt_documents_rest_test.rs
git commit -m "feat(crdt_documents): REST endpoints for events, cursor, by-session (B-T7)"
```

---

## Task 8: Extend `CrdtDocsContext` with session_id, backend, and max_event_id

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_context.rs`

- [ ] **Step 1: Extend the enum variants and constructors**

Replace the current enum + impl:

```rust
use crate::crdt_documents::{
    crdt_backend::CrdtBackend, ArtifactId, CrdtDocumentsRuntime, DirectBackend, RestBackend,
    WsPeerArtifact,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use yrs::Doc;

pub enum CrdtDocsContext {
    Local {
        runtime: Arc<CrdtDocumentsRuntime>,
        artifact_id: ArtifactId,
        session_id: Option<String>,
        backend: Arc<dyn CrdtBackend>,
        max_event_id: Arc<AtomicU64>,
    },
    WsPeer {
        artifact_id: ArtifactId,
        doc: Arc<Doc>,
        alive: Arc<AtomicBool>,
        session_id: Option<String>,
        backend: Arc<dyn CrdtBackend>,
        max_event_id: Arc<AtomicU64>,
    },
}

impl CrdtDocsContext {
    pub fn new_local(
        runtime: Arc<CrdtDocumentsRuntime>,
        artifact_id: ArtifactId,
        session_id: Option<String>,
    ) -> Self {
        let backend: Arc<dyn CrdtBackend> = Arc::new(DirectBackend {
            store: runtime.store.clone(),
        });
        Self::Local {
            runtime,
            artifact_id,
            session_id,
            backend,
            max_event_id: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn new_ws_peer(
        peer: &WsPeerArtifact,
        session_id: Option<String>,
        server_base_url: impl Into<String>,
    ) -> Self {
        let backend: Arc<dyn CrdtBackend> = Arc::new(RestBackend::new(server_base_url));
        Self::WsPeer {
            artifact_id: peer.artifact_id.clone(),
            doc: peer.doc.clone(),
            alive: peer.alive.clone(),
            session_id,
            backend,
            max_event_id: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn artifact_id(&self) -> &ArtifactId {
        match self {
            Self::Local { artifact_id, .. } | Self::WsPeer { artifact_id, .. } => artifact_id,
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Local { session_id, .. } | Self::WsPeer { session_id, .. } => {
                session_id.as_deref()
            }
        }
    }

    pub fn backend(&self) -> &dyn CrdtBackend {
        match self {
            Self::Local { backend, .. } | Self::WsPeer { backend, .. } => backend.as_ref(),
        }
    }

    pub fn doc(&self) -> Option<Arc<Doc>> {
        match self {
            Self::Local { runtime, artifact_id, .. } => {
                runtime.registry.get(artifact_id).map(|e| e.doc.clone())
            }
            Self::WsPeer { doc, alive, .. } => {
                if alive.load(Ordering::Acquire) { Some(doc.clone()) } else { None }
            }
        }
    }

    pub fn mark_dirty(&self) {
        match self {
            Self::Local { runtime, artifact_id, .. } => {
                if let Some(e) = runtime.registry.get(artifact_id) {
                    e.mark_dirty();
                }
            }
            Self::WsPeer { .. } => {}
        }
    }

    /// Track the highest event_id observed during this turn. Called by tool
    /// dispatchers after `sink().record()`. The lifecycle owner (llm.rs)
    /// reads this with `max_event_id_observed()` to advance the cursor.
    pub fn record_event_id(&self, id: u64) {
        let atomic = match self {
            Self::Local { max_event_id, .. } | Self::WsPeer { max_event_id, .. } => max_event_id,
        };
        let mut cur = atomic.load(Ordering::Acquire);
        while id > cur {
            match atomic.compare_exchange_weak(cur, id, Ordering::Release, Ordering::Acquire) {
                Ok(_) => break,
                Err(observed) => cur = observed,
            }
        }
    }

    pub fn max_event_id_observed(&self) -> u64 {
        match self {
            Self::Local { max_event_id, .. } | Self::WsPeer { max_event_id, .. } => {
                max_event_id.load(Ordering::Acquire)
            }
        }
    }

    pub fn is_alive(&self) -> bool {
        match self {
            Self::Local { .. } => true,
            Self::WsPeer { alive, .. } => alive.load(Ordering::Acquire),
        }
    }
}
```

- [ ] **Step 2: Build and fix call-sites**

```bash
cargo build --bin dag_engine 2>&1 | tail -20
```

You'll need to update llm.rs constructors. Find: `CrdtDocsContext::new_local(...)` and `CrdtDocsContext::new_ws_peer(...)` call sites. They now need a `session_id` param (and `server_base_url` for ws_peer). Pass `agent_session_id_str.clone()` and for ws_peer mode derive `server_base_url` from `ws_url` (drop ws://, replace with http://, drop the /yjs suffix). Be lenient: if anything fails, fall back to empty session_id (will become "no session" branch downstream).

- [ ] **Step 3: Run crdt_documents tests + build**

```bash
cargo build --bin dag_engine
cargo test --lib -p colmena_dag_engine crdt
```
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_context.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "refactor(crdt_doc_context): add session_id + backend + max_event_id tracking (B-T8)"
```

---

## Task 9: Update tool dispatchers to record events via backend

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`

- [ ] **Step 1: Update `execute_set_cell`, `execute_set_range`, `execute_add_sheet`**

For each, replace the existing `ctx.tracker().record(...)` (which was sync via the old API) with a call to `ctx.backend().record_event(...)` + capture the returned id. The functions become async (they were sync; the call sites are in async dispatcher wrappers so this is fine).

Example for `execute_set_cell`:

```rust
pub async fn execute_set_cell(ctx: &CrdtDocsContext, args: SetCellArgs) -> serde_json::Value {
    let Some(doc) = ctx.doc() else {
        return serde_json::json!({ "error": "artifact_not_found" });
    };
    crate::crdt_documents::tool_executor::apply_set_cell_in_proc(
        &doc,
        &args.sheet_id,
        &args.addr,
        &args.value,
    );
    ctx.mark_dirty();

    // Build the origin including session for proper filtering downstream.
    let origin = ctx
        .session_id()
        .map(|s| format!("agent:{s}"))
        .unwrap_or_else(|| "agent:llm".to_string());

    let event_id = ctx
        .backend()
        .record_event(crate::crdt_documents::change_tracker_store::NewEvent {
            artifact_id: ctx.artifact_id().clone(),
            sheet_id: Some(args.sheet_id.clone()),
            origin,
            summary: format!("set {}!{} = {}", args.sheet_id, args.addr, args.value),
        })
        .await
        .unwrap_or(0);
    ctx.record_event_id(event_id);

    serde_json::json!({ "ok": true })
}
```

Apply the same pattern to `execute_set_range` and `execute_add_sheet`.

The dispatch wrappers (`dispatch_crdt_doc_set_cell` etc.) call these — they're already `async`, no signature change needed.

- [ ] **Step 2: Update `execute_get_recent_changes` to use backend + session filter**

```rust
pub async fn execute_get_recent_changes(
    ctx: &CrdtDocsContext,
    args: GetRecentChangesArgs,
) -> serde_json::Value {
    let since = match args.since_event_id {
        Some(s) => s,
        None => match ctx.session_id() {
            Some(sid) => ctx
                .backend()
                .cursor_for(sid, ctx.artifact_id())
                .await
                .ok()
                .flatten()
                .unwrap_or(0),
            None => 0,
        },
    };
    let limit = args.limit.unwrap_or(50);
    let own_origin = ctx.session_id().map(|s| format!("agent:{s}"));
    let events = ctx
        .backend()
        .events_since(
            ctx.artifact_id(),
            since,
            args.sheet_id.as_deref(),
            own_origin.as_deref(),
            limit,
        )
        .await
        .unwrap_or_default();
    let current_event_id = events.iter().map(|e| e.id).max();
    let truncated = (events.len() as u32) >= limit;
    serde_json::json!({
        "current_event_id": current_event_id,
        "events": events.iter().map(|e| serde_json::json!({
            "id": e.id, "origin": e.origin, "sheet_id": e.sheet_id,
            "summary": e.summary, "created_at": e.created_at,
        })).collect::<Vec<_>>(),
        "truncated": truncated,
    })
}
```

- [ ] **Step 3: Extend `GetRecentChangesArgs`**

```rust
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GetRecentChangesArgs {
    #[serde(default)]
    pub since_event_id: Option<u64>,
    #[serde(default)]
    pub sheet_id: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}
```

- [ ] **Step 4: Build + run dispatcher tests**

```bash
cargo build --bin dag_engine
cargo test --lib -p colmena_dag_engine crdt_doc_tools
```
Expected: existing tests still pass (the `lists_two_sheets` etc.); the `get_recent_changes_empty_then_populated` test may need to be updated to use async + the new signature.

If it fails because the test used `tracker.record` directly, update it to use `backend.record_event` via a fresh in-memory backend.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs
git commit -m "refactor(crdt_doc_tools): record via backend + extend get_recent_changes filters (B-T9)"
```

---

## Task 10: Two new tools — `crdt_doc_list_my_artifacts` + `crdt_doc_create_artifact`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

- [ ] **Step 1: Add `list_my_artifacts` tool**

Append to `crdt_doc_tools.rs`:

```rust
// ── list_my_artifacts ─────────────────────────────────────────────────────

pub const TOOL_LIST_MY_ARTIFACTS: &str = "crdt_doc_list_my_artifacts";

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListMyArtifactsArgs {
    #[serde(default)]
    pub limit: Option<u32>,
}

pub fn tool_list_my_artifacts() -> ToolDefinition {
    super::build_synthetic_tool::<ListMyArtifactsArgs>(
        TOOL_LIST_MY_ARTIFACTS,
        "List CRDT workbooks accessible to the current agent session. \
         Returns id, name, created_at, last_accessed_at for each.",
    )
}

pub async fn execute_list_my_artifacts(
    ctx: &CrdtDocsContext,
    args: ListMyArtifactsArgs,
) -> serde_json::Value {
    let Some(sid) = ctx.session_id() else {
        return serde_json::json!({"error": "session_required"});
    };
    let limit = args.limit.unwrap_or(50);
    let arts = ctx
        .backend()
        .artifacts_for_session(sid, limit)
        .await
        .unwrap_or_default();
    serde_json::json!({
        "artifacts": arts.iter().map(|a| serde_json::json!({
            "artifact_id": a.artifact_id,
            "name": a.name,
            "created_at": a.created_at,
            "last_accessed_at": a.last_accessed_at,
        })).collect::<Vec<_>>(),
    })
}

pub async fn dispatch_crdt_doc_list_my_artifacts(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<ListMyArtifactsArgs>(args) {
        Ok(a) => execute_list_my_artifacts(ctx, a).await,
        Err(e) => serde_json::json!({ "error": format!("invalid_args: {e}") }),
    }
}
```

- [ ] **Step 2: Add `create_artifact` tool**

Append:

```rust
// ── create_artifact ───────────────────────────────────────────────────────

pub const TOOL_CREATE_ARTIFACT: &str = "crdt_doc_create_artifact";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateArtifactArgs {
    pub name: String,
}

pub fn tool_create_artifact() -> ToolDefinition {
    super::build_synthetic_tool::<CreateArtifactArgs>(
        TOOL_CREATE_ARTIFACT,
        "Create a new CRDT workbook for this session. Returns the new \
         artifact_id. To mutate it you'll need a follow-up turn whose \
         config pins this artifact_id (current limitation; multi-artifact \
         write access is subsystem F).",
    )
}

pub async fn execute_create_artifact(
    ctx: &CrdtDocsContext,
    args: CreateArtifactArgs,
) -> serde_json::Value {
    let Some(sid) = ctx.session_id() else {
        return serde_json::json!({"error": "session_required"});
    };

    // In Local mode we can call registry directly. In WsPeer mode we
    // POST to the server's /documents endpoint.
    match ctx {
        CrdtDocsContext::Local { runtime, .. } => {
            let new_id = crate::crdt_documents::ArtifactId::new();
            let _ = runtime.registry.get_or_create(&new_id, &args.name);
            let _ = ctx
                .backend()
                .touch_artifact(sid, &new_id, Some(&args.name))
                .await;
            serde_json::json!({
                "artifact_id": new_id.to_string(),
                "name": args.name,
            })
        }
        CrdtDocsContext::WsPeer { backend, .. } => {
            // RestBackend doesn't have a direct "create artifact" method.
            // The standard way is to POST /documents with the session id.
            // For simplicity we expose this via a dedicated HTTP call here,
            // bypassing the backend trait abstraction.
            //
            // Casting: we know we're in ws_peer mode, so backend is a
            // RestBackend. Use its base_url for the POST.
            let rest = backend
                .as_ref() as &(dyn std::any::Any);
            let Some(rest) = rest.downcast_ref::<crate::crdt_documents::RestBackend>() else {
                return serde_json::json!({"error": "internal: wrong backend type"});
            };
            let url = format!("{}/documents", rest.base_url);
            let body = serde_json::json!({
                "name": args.name,
                "agent_session_id": sid,
            });
            match rest.client.post(&url).json(&body).send().await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(j) => j,
                        Err(e) => serde_json::json!({"error": format!("decode: {e}")}),
                    }
                }
                Ok(resp) => serde_json::json!({"error": format!("status {}", resp.status())}),
                Err(e) => serde_json::json!({"error": format!("http: {e}")}),
            }
        }
    }
}

pub async fn dispatch_crdt_doc_create_artifact(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<CreateArtifactArgs>(args) {
        Ok(a) => execute_create_artifact(ctx, a).await,
        Err(e) => serde_json::json!({ "error": format!("invalid_args: {e}") }),
    }
}
```

The `downcast_ref` requires `CrdtBackend` to extend `Any`. Add to `crdt_backend.rs`:

```rust
#[async_trait]
pub trait CrdtBackend: Send + Sync + std::any::Any {
    // ... existing methods ...
}
```

- [ ] **Step 3: Update `build_all_crdt_doc_tools`**

```rust
pub fn build_all_crdt_doc_tools() -> Vec<ToolDefinition> {
    vec![
        tool_list_sheets(),
        tool_read(),
        tool_set_cell(),
        tool_set_range(),
        tool_add_sheet(),
        tool_get_recent_changes(),
        tool_list_my_artifacts(),
        tool_create_artifact(),
    ]
}
```

- [ ] **Step 4: Wire dispatcher in `dag_tool_executor.rs`**

Find the existing dispatch table for `crdt_doc_*` tools and add the two new branches:

```rust
TOOL_LIST_MY_ARTIFACTS => dispatch_crdt_doc_list_my_artifacts(ctx, args).await,
TOOL_CREATE_ARTIFACT => dispatch_crdt_doc_create_artifact(ctx, args).await,
```

(Pattern matching existing tools — look at how `dispatch_crdt_doc_add_sheet` is wired and copy.)

- [ ] **Step 5: Update mod.rs re-exports**

In `llm_synthetic_tools/mod.rs`, extend the `crdt_doc_tools` re-export:

```rust
pub use crdt_doc_tools::{
    // ... existing ...
    dispatch_crdt_doc_create_artifact, dispatch_crdt_doc_list_my_artifacts,
    TOOL_CREATE_ARTIFACT as CRDT_DOC_CREATE_ARTIFACT_TOOL,
    TOOL_LIST_MY_ARTIFACTS as CRDT_DOC_LIST_MY_ARTIFACTS_TOOL,
};
```

- [ ] **Step 6: Add unit tests for the new tools**

Append to the `tests` module in `crdt_doc_tools.rs`:

```rust
#[tokio::test]
async fn list_my_artifacts_returns_session_artifacts() {
    let (ctx, _tmp) = fresh_ctx_with_session("s_list").await;
    // Touch two artifacts via the backend directly to seed.
    ctx.backend().touch_artifact("s_list", &ArtifactId::new(), Some("First")).await.unwrap();
    ctx.backend().touch_artifact("s_list", &ArtifactId::new(), Some("Second")).await.unwrap();
    let v = execute_list_my_artifacts(&ctx, ListMyArtifactsArgs { limit: None }).await;
    assert_eq!(v["artifacts"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn create_artifact_returns_new_id_local_mode() {
    let (ctx, _tmp) = fresh_ctx_with_session("s_create").await;
    let v = execute_create_artifact(
        &ctx,
        CreateArtifactArgs { name: "Inventory Q3".into() },
    ).await;
    assert!(v["artifact_id"].as_str().unwrap().starts_with("art_"));
    assert_eq!(v["name"].as_str().unwrap(), "Inventory Q3");
}

// Helper variant of fresh_ctx that takes a session_id
async fn fresh_ctx_with_session(session_id: &str) -> (CrdtDocsContext, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!("t_{}", ulid::Ulid::new()));
    let cfg = json!({ "storage_root": tmp.to_str().unwrap() });
    let rt = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
    let id = ArtifactId::new();
    let _ = rt.registry.get_or_create(&id, "t");
    (
        CrdtDocsContext::new_local(rt, id, Some(session_id.to_string())),
        tmp,
    )
}
```

- [ ] **Step 7: Run tests**

```bash
cargo test --lib -p colmena_dag_engine crdt_doc_tools
```
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs \
        src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs \
        src/libs/colmena/src/crdt_documents/crdt_backend.rs
git commit -m "feat(crdt_doc_tools): list_my_artifacts + create_artifact (B-T10)"
```

---

## Task 11: `crdt_summary` module — build the auto-injected block

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_summary.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`

- [ ] **Step 1: Write the formatter + tests**

```rust
// src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_summary.rs

//! Builds the "Recent changes since your last turn" block that
//! llm.rs prepends to the system_message when a `crdt_documents`
//! context is configured.

use crate::crdt_documents::{
    change_tracker_store::StoredEvent, ArtifactId,
};
use std::collections::HashMap;

use super::crdt_doc_context::CrdtDocsContext;

const MAX_SHEETS_IN_SUMMARY: usize = 10;
const MAX_EVENTS_TO_FETCH: u32 = 200; // hard cap on what we'll aggregate

/// Returns the block text or `None` if no events to surface.
pub async fn build_recent_changes_block(ctx: &CrdtDocsContext) -> Option<String> {
    let session_id = ctx.session_id()?;
    let cursor = ctx
        .backend()
        .cursor_for(session_id, ctx.artifact_id())
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    let own_origin = format!("agent:{session_id}");
    let events = ctx
        .backend()
        .events_since(
            ctx.artifact_id(),
            cursor,
            None,
            Some(&own_origin),
            MAX_EVENTS_TO_FETCH,
        )
        .await
        .ok()?;
    if events.is_empty() {
        return None;
    }
    Some(format_block(&events))
}

fn format_block(events: &[StoredEvent]) -> String {
    // Aggregate by (sheet_id, origin)
    let mut buckets: HashMap<(Option<String>, String), u32> = HashMap::new();
    let peers: std::collections::HashSet<String> =
        events.iter().map(|e| e.origin.clone()).collect();
    for e in events {
        *buckets.entry((e.sheet_id.clone(), e.origin.clone())).or_insert(0) += 1;
    }
    let mut lines: Vec<(String, String, u32)> = buckets
        .into_iter()
        .map(|((sheet, origin), n)| {
            let label = match sheet {
                Some(s) => s,
                None => "Workbook (sheet unknown)".to_string(),
            };
            (label, origin, n)
        })
        .collect();
    lines.sort_by(|a, b| b.2.cmp(&a.2)); // descending by count
    let total_lines = lines.len();
    let mut out = String::new();
    out.push_str("\n---\n");
    out.push_str(&format!(
        "Workbook changes since your last turn ({} events, {} peer{}):\n",
        events.len(),
        peers.len(),
        if peers.len() == 1 { "" } else { "s" }
    ));
    for (label, origin, n) in lines.iter().take(MAX_SHEETS_IN_SUMMARY) {
        out.push_str(&format!(
            "- {label}: {n} change{} by {origin}\n",
            if *n == 1 { "" } else { "s" }
        ));
    }
    if total_lines > MAX_SHEETS_IN_SUMMARY {
        out.push_str(&format!(
            "- ...and {} more sheet/peer groups changed\n",
            total_lines - MAX_SHEETS_IN_SUMMARY
        ));
    }
    out.push_str(
        "Use `crdt_doc_get_recent_changes(sheet_id?)` for cell-level detail.\n---\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: u64, sheet: Option<&str>, origin: &str, summary: &str) -> StoredEvent {
        StoredEvent {
            id,
            artifact_id: "art_x".into(),
            sheet_id: sheet.map(String::from),
            origin: origin.to_string(),
            summary: summary.to_string(),
            created_at: "now".into(),
        }
    }

    #[test]
    fn empty_events_returns_no_block_via_caller_check() {
        // build_recent_changes_block returns None on empty; we test
        // format_block here for shape; emptiness is upstream.
        let block = format_block(&[]);
        assert!(block.contains("0 events"));
    }

    #[test]
    fn single_sheet_single_peer() {
        let evs = vec![
            ev(1, Some("Inventory"), "peer:browser", "x"),
            ev(2, Some("Inventory"), "peer:browser", "y"),
            ev(3, Some("Inventory"), "peer:browser", "z"),
        ];
        let block = format_block(&evs);
        assert!(block.contains("3 events, 1 peer"));
        assert!(block.contains("Inventory: 3 changes by peer:browser"));
    }

    #[test]
    fn two_sheets_two_peers() {
        let evs = vec![
            ev(1, Some("Inventory"), "peer:browser", "x"),
            ev(2, Some("Inventory"), "peer:browser", "y"),
            ev(3, Some("Inventory"), "peer:browser", "z"),
            ev(4, Some("Pricing"), "agent:orchestrator", "a"),
            ev(5, Some("Pricing"), "agent:orchestrator", "b"),
        ];
        let block = format_block(&evs);
        assert!(block.contains("5 events, 2 peers"));
        assert!(block.contains("Inventory: 3 changes by peer:browser"));
        assert!(block.contains("Pricing: 2 changes by agent:orchestrator"));
    }

    #[test]
    fn workbook_level_when_sheet_unknown() {
        let evs = vec![
            ev(1, None, "peer:browser", "coarse"),
            ev(2, None, "peer:browser", "coarse"),
        ];
        let block = format_block(&evs);
        assert!(block.contains("Workbook (sheet unknown): 2 changes by peer:browser"));
    }

    #[test]
    fn caps_at_max_sheets_with_overflow_marker() {
        let evs: Vec<StoredEvent> = (0..15)
            .map(|i| ev(i, Some(&format!("Sheet{i}")), "peer:browser", "x"))
            .collect();
        let block = format_block(&evs);
        assert!(block.contains("...and 5 more sheet/peer groups changed"));
    }
}
```

- [ ] **Step 2: Export from mod.rs**

```rust
// In llm_synthetic_tools/mod.rs:
pub mod crdt_summary;

pub use crdt_summary::build_recent_changes_block;
```

- [ ] **Step 3: Run tests**

```bash
cargo test --lib -p colmena_dag_engine crdt_summary
```
Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_summary.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
git commit -m "feat(crdt_summary): formatter for recent-changes auto-injected block (B-T11)"
```

---

## Task 12: Integrate auto-summary + cursor update in `llm.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

- [ ] **Step 1: Append the summary block to system_message**

Find the section where `system_message` is built. After the existing temporal_geographic_context block append happens, add:

```rust
// CRDT recent-changes auto-context. Append AFTER the temporal block
// so the order is: instructions → temporal → workbook-changes.
if let Some(ctx) = crdt_docs_context.as_ref() {
    use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::build_recent_changes_block;
    if let Some(block) = build_recent_changes_block(ctx.as_ref()).await {
        system_message.push_str(&block);
    }
}
```

If `system_message` is a `&str`/`Cow` rather than a `String`, convert to owned first (the existing temporal block code path will show the pattern).

- [ ] **Step 2: Add the cursor update at end of execute()**

Find the existing CRDT cleanup block (the one that does `peer.shutdown().await` for ws_peer and `runtime.shutdown().await` for local-owned). RIGHT BEFORE that block, add:

```rust
// Advance the agent's cursor for this artifact so the NEXT turn's
// auto-summary block omits everything we already saw.
if let Some(ctx) = crdt_docs_context.as_ref() {
    if let Some(sid) = ctx.session_id() {
        let max = ctx.max_event_id_observed();
        if max > 0 {
            let _ = ctx
                .backend()
                .upsert_cursor(sid, ctx.artifact_id(), max)
                .await;
        }
    }
}
```

- [ ] **Step 3: Build and verify**

```bash
cargo build --bin dag_engine
```
Expected: build passes.

- [ ] **Step 4: Manual smoke (sqlite local, no server)**

Run an existing test graph that uses `crdt_documents` config. Verify it still completes:

```bash
set -a; source .env; set +a
DUMP=/tmp/crdt_b_smoke
rm -rf $DUMP && mkdir -p $DUMP
DATABASE_URL="sqlite::memory:" cargo run --bin dag_engine -- run \
  tests/graphs/crdt_documents/llm_agent_smoke.json \
  --agent-session-id b_smoke_test \
  --include-extra-info
```
Expected: exit=0, SSE shows tool calls + completion.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm_call): auto-inject recent-changes block + update cursor at turn end (B-T12)"
```

---

## Task 13: Wire `peer_type` + `session_id` query params on WS upgrade

**Files:**
- Modify: `src/libs/colmena/src/crdt_documents/ws_peer.rs`
- Modify: `src/libs/colmena/src/crdt_documents/server.rs`

- [ ] **Step 1: Update `WsPeerArtifact::connect` to accept peer_type + session_id**

```rust
pub async fn connect(
    server_url: &str,
    artifact_id: ArtifactId,
    peer_type: &str,        // "agent" typically
    session_id: Option<&str>,
) -> Result<Self, WsPeerError> {
    let mut full_url = format!(
        "{}/{}?peer_type={}",
        server_url.trim_end_matches('/'),
        artifact_id.as_str(),
        peer_type,
    );
    if let Some(sid) = session_id {
        full_url.push_str(&format!("&session_id={sid}"));
    }
    // ... rest unchanged ...
}
```

Update the two existing call sites:
- `llm.rs` WsPeer construction: pass `"agent"` and the agent_session_id.
- Test in `tests/crdt_ws_peer_full_tools_test.rs`: pass `"agent"` and `Some("test_session")`.

- [ ] **Step 2: Capture peer_type + session_id in `ws_handler`**

In `server.rs`'s `ws_handler`, extract query params:

```rust
async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(id_str): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
) -> Response {
    // ... existing code ...
    let peer_type = params
        .get("peer_type")
        .cloned()
        .unwrap_or_else(|| "browser".to_string());
    let session_id = params.get("session_id").cloned();
    // pass them through to the closure ...
}
```

In the `post_update` callback (the one that does `tracker.record(...)` today), build origin and call:

```rust
let post_update = move |update_bytes: &[u8]| {
    dirty.store(true, Ordering::Release);
    notify.notify_one();
    let summary = format!("peer update ({} bytes)", update_bytes.len());
    let origin = match peer_type.as_str() {
        "agent" => session_id
            .as_deref()
            .map(|s| format!("agent:{s}"))
            .unwrap_or_else(|| "agent:anonymous".to_string()),
        _ => "peer:browser".to_string(),
    };
    let id_for_cb = id_for_cb.clone();
    let store = runtime.store.clone();
    tokio::spawn(async move {
        let _ = store
            .insert_event(NewEvent {
                artifact_id: id_for_cb,
                sheet_id: None,
                origin,
                summary,
            })
            .await;
    });
};
```

(Switched from `tracker.record()` synchronous to direct store call, because the closure runs from a non-async context inside the WS dispatch thread.)

- [ ] **Step 3: Build + run existing tests**

```bash
cargo build --bin dag_engine
cargo test --lib -p colmena_dag_engine ws_peer
cargo test --test crdt_ws_peer_full_tools_test
```
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/crdt_documents/ws_peer.rs \
        src/libs/colmena/src/crdt_documents/server.rs \
        src/libs/colmena/tests/crdt_ws_peer_full_tools_test.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(crdt_documents): WS peer_type+session_id query for proper origin attribution (B-T13)"
```

---

## Task 14: Integration test — full B end-to-end

**Files:**
- Create: `src/libs/colmena/tests/crdt_doc_recent_changes_test.rs`

- [ ] **Step 1: Write the test**

```rust
//! End-to-end test for subsystem B (recent changes + discovery).
//!
//! Spins up a real CRDT documents server with in-memory sqlite,
//! connects an agent as ws_peer, simulates the auto-summary flow,
//! and verifies cursor advancement + drill-down filtering.

use colmena::crdt_documents::{
    process_runtime, server::router as server_router, ArtifactId, CrdtDocumentsRuntime,
    WsPeerArtifact,
};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    build_recent_changes_block,
    crdt_doc_context::CrdtDocsContext,
    crdt_doc_tools::{
        execute_create_artifact, execute_get_recent_changes, execute_list_my_artifacts,
        execute_set_cell, CreateArtifactArgs, GetRecentChangesArgs, ListMyArtifactsArgs,
        SetCellArgs,
    },
};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::test]
async fn recent_changes_round_trip_via_ws_peer() {
    // --- Server ---------------------------------------------------------
    let dump = std::env::temp_dir().join(format!("b_int_{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dump).unwrap();
    let cfg = json!({
        "storage_backend": "localfs",
        "storage_root": dump.to_str().unwrap(),
        "database_url": "sqlite::memory:",
    });
    let server_runtime = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
    let aid = ArtifactId::new();
    let _ = server_runtime.registry.get_or_create(&aid, "B test");

    let app = server_router(server_runtime.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move { let _ = axum::serve(listener, app).await; });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let session_id = "session_b_test".to_string();
    let server_url = format!("ws://{}/yjs", addr);

    // Simulate a prior turn: another peer (browser) made some changes.
    // Inject directly via the server's store.
    server_runtime.store.insert_event(
        colmena::crdt_documents::change_tracker_store::NewEvent {
            artifact_id: aid.clone(),
            sheet_id: Some("Inventory".into()),
            origin: "peer:browser".into(),
            summary: "set Inventory!A1 = hello".into(),
        }
    ).await.unwrap();
    server_runtime.store.insert_event(
        colmena::crdt_documents::change_tracker_store::NewEvent {
            artifact_id: aid.clone(),
            sheet_id: Some("Inventory".into()),
            origin: "peer:browser".into(),
            summary: "set Inventory!A2 = world".into(),
        }
    ).await.unwrap();

    // --- Peer (agent in worker) -----------------------------------------
    let peer = WsPeerArtifact::connect(&server_url, aid.clone(), "agent", Some(&session_id))
        .await
        .unwrap();
    let http_base = format!("http://{}", addr);
    let ctx = CrdtDocsContext::new_ws_peer(&peer, Some(session_id.clone()), &http_base);

    // Auto-summary should show 2 peer:browser changes on Inventory.
    let block = build_recent_changes_block(&ctx).await.expect("block");
    assert!(block.contains("2 events, 1 peer"));
    assert!(block.contains("Inventory: 2 changes by peer:browser"));

    // Drill-down via tool with sheet filter.
    let v = execute_get_recent_changes(
        &ctx,
        GetRecentChangesArgs { since_event_id: None, sheet_id: Some("Inventory".into()), limit: None },
    ).await;
    assert_eq!(v["events"].as_array().unwrap().len(), 2);

    // Agent does its own mutation.
    execute_set_cell(&ctx, SetCellArgs {
        sheet_id: "Inventory".into(),
        addr: "B1".into(),
        value: json!("agent wrote this"),
    }).await;

    // Verify the agent's own event is filtered out of the summary.
    // (Cursor hasn't advanced yet — this query starts at cursor=0)
    let block_again = build_recent_changes_block(&ctx).await.expect("block");
    assert!(block_again.contains("2 events, 1 peer"));
    assert!(!block_again.contains(&format!("agent:{}", session_id)));

    // Simulate end-of-turn cursor update.
    let max = ctx.max_event_id_observed();
    assert!(max > 0, "agent's own event should have been observed");
    ctx.backend().upsert_cursor(&session_id, &aid, max).await.unwrap();

    // Next turn: build the block again. Now cursor > all peer:browser events,
    // so the block should be None.
    let block_next = build_recent_changes_block(&ctx).await;
    assert!(block_next.is_none(), "no new events since cursor → no block");

    // Discovery tool.
    let v = execute_list_my_artifacts(&ctx, ListMyArtifactsArgs { limit: None }).await;
    // The peer hasn't been "touched" — list might be empty unless the
    // touch was triggered elsewhere. Acceptable empty for this assertion;
    // we just verify it doesn't error.
    assert!(v["artifacts"].is_array());

    // Cleanup.
    let mut peer = peer;
    peer.shutdown().await;
    server_runtime.shutdown().await;
    let _ = std::fs::remove_dir_all(&dump);
}
```

- [ ] **Step 2: Run the test**

```bash
cargo test --test crdt_doc_recent_changes_test
```
Expected: 1 test passes.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/tests/crdt_doc_recent_changes_test.rs
git commit -m "test(crdt_documents): full B end-to-end integration test (B-T14)"
```

---

## Task 15: Documentation — developer_guide/38_crdt_documents.md

**Files:**
- Modify: `docs/developer_guide/38_crdt_documents.md`

- [ ] **Step 1: Add a new §5.5 "Recent changes awareness + discovery" subsection**

Find §5 ("LLM tools (synthetic)") in the file. After the existing config block subsections, add:

```markdown
### 5.5 Recent changes awareness + discovery

#### Auto-injected summary block

Cuando `llm_call` corre con `crdt_documents` configurado AND con `agent_session_id`, el sistema auto-inyecta un bloque corto al `system_message` con los cambios que OTROS peers hicieron al workbook desde el último turn del agente:

\`\`\`
---
Workbook changes since your last turn (5 events, 2 peers):
- Inventory: 3 changes by peer:browser
- Pricing: 2 changes by agent:orchestrator
Use `crdt_doc_get_recent_changes(sheet_id?)` for cell-level detail.
---
\`\`\`

Reglas:
- Solo se inyecta si hay >0 eventos relevantes (filtra mutaciones del propio agente).
- Tope 10 sheets en el listado; overflow muestra `...and N more sheet/peer groups changed`.
- Eventos de browser tienen `sheet_id` null en v1 (limitación documentada en BACKLOG); aparecen como bucket "Workbook (sheet unknown)".
- El cursor del agente se actualiza al **final del turn** — si el turn falla mid-way, el cursor no se mueve.

#### Tools nuevos / extendidos

| Tool | Args | Returns |
|------|------|---------|
| `crdt_doc_get_recent_changes` | `since_event_id?`, `sheet_id?`, `limit?` | `{current_event_id, events[], truncated}` |
| `crdt_doc_list_my_artifacts` | `limit?` (default 50) | `{artifacts[{id, name, created_at, last_accessed_at}]}` |
| `crdt_doc_create_artifact` | `name` | `{artifact_id, name}` — la mutación al nuevo artifact requiere otro turn que lo pinee en `crdt_documents.artifact_id` |

#### Tablas SQL

Tres tablas se crean automáticamente al startup via migrations:

- `crdt_doc_events` — log append-only de mutaciones (id, artifact_id, sheet_id, origin, summary, created_at).
- `crdt_doc_session_cursors` — cursor por (agent_session_id, artifact_id) → last_event_id.
- `crdt_doc_session_artifacts` — ownership: qué artifacts pertenecen a qué session.

Source of truth: `src/libs/colmena/migrations/{sqlite,postgres}/20260603000000_crdt_doc_changes.sql`.
```

- [ ] **Step 2: Commit**

```bash
git add docs/developer_guide/38_crdt_documents.md
git commit -m "docs(crdt_documents): document recent-changes awareness + discovery tools (B-T15)"
```

---

## Task 16: Documentation — node_configurations.json + new tool entries

**Files:**
- Modify: `docs/node_configurations.json`

- [ ] **Step 1: Verify node_configurations covers the new tools**

Run: `grep -n "crdt_doc_get_recent_changes\|crdt_doc_list_my_artifacts\|crdt_doc_create_artifact" docs/node_configurations.json | head`

If the file documents tools as part of `llm_call.tool_configurations` schema or similar, ensure the new tool names are mentioned. If not (the schema only documents config field shape, not individual tools), the existing `crdt_documents` config block update from V2 is sufficient — no further changes needed in node_configurations.json.

- [ ] **Step 2: If tools need entries, add them**

(Skip this step if the file doesn't enumerate individual tool names — the schema only covers config blocks.)

- [ ] **Step 3: Commit only if there were changes**

```bash
git add docs/node_configurations.json
git commit -m "docs(schemas): add B's tools to node_configurations (B-T16)" || echo "no changes"
```

---

## Task 17: BACKLOG entries for deferred items

**Files:**
- Modify: `docs/BACKLOG.md`

- [ ] **Step 1: Add three new entries**

After the last "CRDT Documents v1.1 — ..." entry in `docs/BACKLOG.md`, append:

```markdown
---

## CRDT Documents v1.1 — Per-cell attribution para peer:browser events

- **Origen:** scope-cut al implementar subsistema B (2026-06-03). El server recibe updates Yjs binarios opacos de browsers; no puede saber qué sheet/celda cambió sin inferencia activa.
- **Problema:** los eventos de `peer:browser` quedan con `sheet_id: NULL` y summary "peer update (N bytes)". En el auto-summary aparecen como "Workbook (sheet unknown): N changes by peer:browser", lo cual es menos informativo que "Inventory: N changes by peer:browser".
- **Workaround actual:** acepta granularidad coarse. Si el agente necesita saber qué sheet cambió, debe leer el doc directamente.
- **Por qué está parqueado:** el v1 prioriza el flow end-to-end. La inferencia per-cell requiere un diff de projection antes/después del apply_update, lo cual es trabajo no trivial.
- **Fix propuesto:**
  1. En `handle_socket`, antes de cada `apply_update`, capturar la projection actual del Y.Doc.
  2. Aplicar el update.
  3. Diffear la projection nueva contra la vieja.
  4. Por cada celda cambiada, registrar un event con sheet_id, addr, value (antes/después).
- **Acceptance criteria:**
  - peer:browser events tienen sheet_id + addr poblados.
  - Auto-summary muestra "Inventory: 3 changes by peer:browser" en vez de "Workbook (sheet unknown)".
  - Performance: el diff per-update < 5ms para workbooks <1MB.
- **Estimación:** ~1-2 días. Mide impacto perf con benchmark.
- **Cuándo retomar:** cuando UX feedback indique que la atribución coarse es limitante (probable para flows colaborativos browser+agente).

---

## CRDT Documents v1.1 — Paginación de list_my_artifacts

- **Origen:** scope-cut B (2026-06-03).
- **Problema:** sesiones con >50 artifacts solo ven los 50 más recientes via `crdt_doc_list_my_artifacts`. No hay cursor de paginación.
- **Workaround actual:** los 50 más recientes alcanzan para la mayoría de flows. Cliente puede pasar `limit` mayor (sin tope técnico).
- **Fix propuesto:** agregar `offset` o `cursor: Option<String>` (timestamp-based). Devolver `next_cursor` cuando hay más.
- **Cuándo retomar:** cuando reportemos sesiones con >50 artifacts.

---

## CRDT Documents v1.1 — Retención TTL en `crdt_doc_events`

- **Origen:** decision durante design B.
- **Problema:** la tabla crece sin límite. Para una sesión de uso intenso (1 evento/min × 100 días) son 144k rows. Manejable, pero crece.
- **Fix propuesto:** scheduled job (Cloud Scheduler) que ejecuta `DELETE FROM crdt_doc_events WHERE created_at < now() - INTERVAL '90 days'`. Configurable.
- **Cuándo retomar:** cuando la tabla supere 1M rows en producción.
```

- [ ] **Step 2: Commit**

```bash
git add docs/BACKLOG.md
git commit -m "docs(backlog): defer per-cell attribution + pagination + TTL for B follow-ups (B-T17)"
```

---

## Task 18: CHANGELOG entry

**Files:**
- Create: `docs/CHANGELOG_2026-06.md`

- [ ] **Step 1: Create the June changelog with B as first entry**

```markdown
# Cambios recientes — 2026-06

> **Generado:** 2026-06-03 (subsystem B landed)
> **Alcance:** Commits sobre `feature/docs` desde el cierre de V2 (commit `88b3bc7`) hasta el merge eventual a `develop`.

## Cómo leer este documento

Una sección por feature. Cada sección contiene:
- **Qué cambió** — efecto observable.
- **Documentación de referencia** — spec, plan, dev guide, schema.
- **Commits** — rango o lista.
- **Estado** — done / partial.

---

## 1. CRDT Documents — Recent changes awareness + artifact discovery (subsistema B)

**Qué cambió.** Cada llm_call con `crdt_documents` config auto-recibe un bloque corto en el system_message describiendo qué cambiaron otros peers desde su último turn. Tool `crdt_doc_get_recent_changes` extendido con filtros (`sheet_id?`, `limit?`). Dos tools nuevos: `crdt_doc_list_my_artifacts` y `crdt_doc_create_artifact`. Toda la auditoría queda en SQL: 3 tablas nuevas (`crdt_doc_events`, `crdt_doc_session_cursors`, `crdt_doc_session_artifacts`).

**Por qué importa.** Antes de B, el agente no sabía qué editaba el humano entre sus turnos (a menos que llamara explícitamente al tool). Ahora la información llega como contexto persistente, gratis. Además el agente puede descubrir/crear workbooks desde adentro de su sesión, lo que abre el camino para subsistema F (compare two excels) y futuros agentes orquestadores.

**Documentación de referencia.**
- Spec: [`docs/superpowers/specs/2026-06-03-crdt-recent-changes-design.md`](superpowers/specs/2026-06-03-crdt-recent-changes-design.md)
- Plan: [`docs/superpowers/plans/2026-06-03-crdt-recent-changes.md`](superpowers/plans/2026-06-03-crdt-recent-changes.md)
- Dev guide §5.5: [`docs/developer_guide/38_crdt_documents.md`](developer_guide/38_crdt_documents.md)
- Items deferidos: [`docs/BACKLOG.md`](BACKLOG.md) (per-cell attribution, paginación, TTL)

**Commits (B-T1 a B-T18).** Ver `git log feature/docs --grep="(B-T"`.

**Estado.** done.

**Limitaciones conocidas v1.**
- Eventos de `peer:browser` tienen `sheet_id: NULL` (server no infiere semántica del Yjs update). Aparecen como "Workbook (sheet unknown)" en el auto-summary. Mejora: BACKLOG "Per-cell attribution para peer:browser events".
- `list_my_artifacts` cap 50 sin paginación. Mejora: BACKLOG.
- TTL de la tabla `crdt_doc_events` no implementado. Mejora: BACKLOG.

---
```

- [ ] **Step 2: Commit**

```bash
git add docs/CHANGELOG_2026-06.md
git commit -m "docs(changelog): June 2026 changelog with subsystem B entry (B-T18)"
```

---

## Task 19: Final sanity sweep + clippy

**Files:** (no new files)

- [ ] **Step 1: Run full test suite**

```bash
cargo test --lib -p colmena_dag_engine 2>&1 | tail -10
cargo test --tests -p colmena_dag_engine crdt 2>&1 | tail -10
```
Expected: 0 failures across all crdt tests.

- [ ] **Step 2: Clippy clean under -D warnings**

```bash
cargo clippy --tests --lib -p colmena_dag_engine 2>&1 | grep -E "warning|error" | head -5
```
Expected: empty output.

- [ ] **Step 3: cargo fmt**

```bash
cargo fmt --check 2>&1 | head -20
```
If diff, run `cargo fmt` and commit.

- [ ] **Step 4: Manual end-to-end browser smoke**

Same procedure as the V2 split smoke, but now verify the auto-summary block:

1. Terminal A: `cargo run --bin dag_engine -- crdt-yws --host 127.0.0.1 --port 8090 --dump-dir /tmp/crdt_b_final`. With `DATABASE_URL=sqlite::memory:` env so the store is durable for the session.
2. Terminal B: create artifact via curl, capture ID, edit cells in browser to populate the events table.
3. Terminal C: run `tests/graphs/crdt_documents/llm_agent_smoke_ws_peer.json` (sed-pin the ID + `agent_session_id agent_b_final_smoke`). Verify SSE includes the agent's tool calls.
4. Terminal C again (second turn, same `agent_session_id`): re-run the smoke graph. Verify the agent's system_message in the SSE includes the auto-injected "Workbook changes since your last turn" block referencing the cells edited in browser between turns.

- [ ] **Step 5: Commit any final tidy-ups**

```bash
git add -A
git commit -m "chore(crdt_documents): subsystem B sanity sweep — clippy + fmt + smoke pass (B-T19)" \
  || echo "no changes"
```

---

## Self-review checklist (run before handoff)

- [ ] **Spec coverage**: every section of `2026-06-03-crdt-recent-changes-design.md` mapped to a task above (§4 architecture → T1-T13; §5 code changes → T1-T13; §6 edge cases covered in tool error paths; §7 testing → T14; §9 deferred → T17).
- [ ] **Placeholder scan**: no "TBD", no "TODO", every step has concrete code or commands.
- [ ] **Type consistency**: `NewEvent`/`StoredEvent`/`StoredArtifact` field names match across T2, T3, T5, T7, T11.
- [ ] **Method consistency**: `events_since`, `cursor_for`, `upsert_cursor`, `touch_artifact`, `artifacts_for_session`, `insert_event`, `record_event` — used identically wherever they appear.
- [ ] **`ctx.session_id()` vs `&agent_session_id_str`**: context exposes `session_id() -> Option<&str>`; llm.rs has `agent_session_id_str: Option<String>`; the bridging is done at context construction (T8).
- [ ] **Migration files end in `.sql` and live under `migrations/sqlite/` or `migrations/postgres/`**: yes, T1.
- [ ] **No `cargo test` step is followed by a passing assertion without running it first**: every test step has a "Run X, expected: pass/fail" pattern. ✓

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-03-crdt-recent-changes.md`. Two execution options:

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
