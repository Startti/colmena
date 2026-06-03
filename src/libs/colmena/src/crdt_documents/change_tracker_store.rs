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
