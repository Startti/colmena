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

// ── SQLx impl (sqlite + postgres via sqlx::Any) ──────────────────────────

use sqlx::{AnyPool, Row};

/// Which dialect this SQLx store talks. Determines placeholder syntax (`?` vs
/// `$N`), RETURNING clause usage, and a couple of upsert syntax details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlxDialect {
    Sqlite,
    Postgres,
}

impl SqlxDialect {
    /// Detect the dialect from a database URL (the prefix scheme).
    pub fn from_url(url: &str) -> Option<Self> {
        if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            Some(Self::Postgres)
        } else if url.starts_with("sqlite:") {
            Some(Self::Sqlite)
        } else {
            None
        }
    }
}

pub struct SqlxChangeTrackerStore {
    pool: AnyPool,
    dialect: SqlxDialect,
}

impl SqlxChangeTrackerStore {
    pub fn new(pool: AnyPool, dialect: SqlxDialect) -> Self {
        Self { pool, dialect }
    }

    fn is_postgres(&self) -> bool {
        self.dialect == SqlxDialect::Postgres
    }
}

#[async_trait]
impl ChangeTrackerStore for SqlxChangeTrackerStore {
    async fn insert_event(&self, ev: NewEvent) -> Result<u64, StoreError> {
        let aid = ev.artifact_id.to_string();

        if self.is_postgres() {
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
            let id: i64 = row
                .try_get("id")
                .map_err(|e| StoreError::Sql(e.to_string()))?;
            Ok(id as u64)
        } else {
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
            let id: i64 = row
                .try_get("id")
                .map_err(|e| StoreError::Sql(e.to_string()))?;
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

        let final_sql = if self.is_postgres() {
            let mut idx = 0usize;
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
            .map(|r| -> Result<StoredEvent, StoreError> {
                let id: i64 = r
                    .try_get("id")
                    .map_err(|e| StoreError::Sql(e.to_string()))?;
                let artifact_id: String = r
                    .try_get("artifact_id")
                    .map_err(|e| StoreError::Sql(e.to_string()))?;
                // sheet_id is nullable — .ok() correctly maps NULL → None
                let sheet_id: Option<String> = r.try_get("sheet_id").ok();
                let origin: String = r
                    .try_get("origin")
                    .map_err(|e| StoreError::Sql(e.to_string()))?;
                let summary: String = r
                    .try_get("summary")
                    .map_err(|e| StoreError::Sql(e.to_string()))?;
                let created_at: String = r
                    .try_get("created_at")
                    .map_err(|e| StoreError::Sql(e.to_string()))?;
                Ok(StoredEvent {
                    id: id as u64,
                    artifact_id,
                    sheet_id,
                    origin,
                    summary,
                    created_at,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(events)
    }

    async fn cursor_for(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
    ) -> Result<Option<u64>, StoreError> {
        let sql = if self.is_postgres() {
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
        match row {
            Some(r) => {
                let v: i64 = r
                    .try_get("last_event_id")
                    .map_err(|e| StoreError::Sql(e.to_string()))?;
                Ok(Some(v as u64))
            }
            None => Ok(None),
        }
    }

    async fn upsert_cursor(
        &self,
        session_id: &str,
        artifact_id: &ArtifactId,
        last_event_id: u64,
    ) -> Result<(), StoreError> {
        let sql = if self.is_postgres() {
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
        let sql = if self.is_postgres() {
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
        let sql = if self.is_postgres() {
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
        rows.into_iter()
            .map(|r| -> Result<StoredArtifact, StoreError> {
                let artifact_id: String = r
                    .try_get("artifact_id")
                    .map_err(|e| StoreError::Sql(e.to_string()))?;
                let name: String = r
                    .try_get("name")
                    .map_err(|e| StoreError::Sql(e.to_string()))?;
                let created_at: String = r
                    .try_get("created_at")
                    .map_err(|e| StoreError::Sql(e.to_string()))?;
                let last_accessed_at: String = r
                    .try_get("last_accessed_at")
                    .map_err(|e| StoreError::Sql(e.to_string()))?;
                Ok(StoredArtifact {
                    artifact_id,
                    name,
                    created_at,
                    last_accessed_at,
                })
            })
            .collect()
    }
}

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

    #[tokio::test]
    async fn sqlx_sqlite_round_trip() {
        use sqlx::any::{AnyConnectOptions, AnyPoolOptions};
        use std::str::FromStr;

        sqlx::any::install_default_drivers();
        let url = "sqlite::memory:".to_string();
        let opts = AnyConnectOptions::from_str(&url).unwrap();
        let pool = AnyPoolOptions::new()
            .max_connections(1) // single connection — :memory: db is per-connection
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("./migrations/sqlite")
            .run(&pool)
            .await
            .unwrap();
        let store = SqlxChangeTrackerStore::new(pool, SqlxDialect::Sqlite);
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
}
