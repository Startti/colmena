//! Façade over [`ChangeTrackerStore`]. Preserves the v1 public API
//! (`record`, `since`, `ChangeEvent`) so existing call sites keep working.
//! Behind the scenes, every operation goes through the configured store
//! (in-memory or SQL).
//!
//! Previously this module owned an in-process `parking_lot::Mutex<HashMap<…>>`
//! buffer capped at 1000 events per artifact. That state has been lifted into
//! [`ChangeTrackerStore`] (see `change_tracker_store.rs`) so events survive
//! across runtime restarts and can be queried from multiple processes (REST
//! façade in B-T5/B-T7). The wrapper itself is now `async`: every method awaits
//! the store and gracefully degrades on errors (best-effort recording).

use crate::crdt_documents::{
    change_tracker_store::{ChangeTrackerStore, NewEvent, StoredEvent},
    ArtifactId,
};
use std::sync::Arc;

/// Event returned to callers of `ChangeTracker::since`. Shape preserved from
/// v1 except for `timestamp_ms` (gone — the store records ISO 8601 strings
/// via `created_at`) and the additional `artifact_id` / `sheet_id` fields
/// surfaced for richer narration in B-T11.
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

/// Thin async wrapper around a [`ChangeTrackerStore`]. Constructed by the
/// runtime (B-T6 wires the SQL store; until then callers pass an
/// [`crate::crdt_documents::InMemoryChangeTrackerStore`]).
pub struct ChangeTracker {
    store: Arc<dyn ChangeTrackerStore>,
}

impl ChangeTracker {
    /// Build a tracker backed by `store`. The tracker is a cheap façade — it
    /// just holds an `Arc<dyn ChangeTrackerStore>` and forwards every call.
    pub fn new(store: Arc<dyn ChangeTrackerStore>) -> Self {
        Self { store }
    }

    /// Record an event. Returns the new event id, or `0` if the store
    /// rejected the insert (the tracker is best-effort — callers use the
    /// returned id only to advance turn high-water cursors). `sheet_id` is
    /// optional so the tracker can keep recording document-level events
    /// (e.g. `added sheet 'X'`) without forcing a synthetic sheet id.
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

    /// Query events after `since_event_id` (or from the beginning when
    /// `None`). Optional filters by `sheet_id` and `exclude_origin` let
    /// callers ignore their own writes (B-T9). `limit` caps the response —
    /// pick a high enough value for the consumer (legacy callers use 100).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::InMemoryChangeTrackerStore;

    fn new_tracker() -> ChangeTracker {
        ChangeTracker::new(Arc::new(InMemoryChangeTrackerStore::new()))
    }

    #[tokio::test]
    async fn records_and_since_filters() {
        let t = new_tracker();
        let id = ArtifactId::new();
        let _ = t.record(&id, None, "agent:test", "set A1").await;
        let _ = t.record(&id, None, "agent:test", "set B1").await;
        let all = t.since(&id, None, None, None, 100).await;
        assert_eq!(all.len(), 2);
        let after_first = t.since(&id, Some(all[0].event_id), None, None, 100).await;
        assert_eq!(after_first.len(), 1);
        assert_eq!(after_first[0].summary, "set B1");
    }

    #[tokio::test]
    async fn empty_for_unknown_artifact() {
        let t = new_tracker();
        let id = ArtifactId::new();
        assert!(t.since(&id, None, None, None, 100).await.is_empty());
    }

    #[tokio::test]
    async fn exclude_origin_filters_out_self() {
        let t = new_tracker();
        let id = ArtifactId::new();
        let _ = t.record(&id, None, "agent:me", "mine").await;
        let _ = t.record(&id, None, "agent:other", "theirs").await;
        let evs = t.since(&id, None, None, Some("agent:me"), 100).await;
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].summary, "theirs");
    }

    #[tokio::test]
    async fn sheet_filter_scopes_results() {
        let t = new_tracker();
        let id = ArtifactId::new();
        let _ = t.record(&id, Some("sh_a"), "agent:llm", "a").await;
        let _ = t.record(&id, Some("sh_b"), "agent:llm", "b").await;
        let only_a = t.since(&id, None, Some("sh_a"), None, 100).await;
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].summary, "a");
    }
}
