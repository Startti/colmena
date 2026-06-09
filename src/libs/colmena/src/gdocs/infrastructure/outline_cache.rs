//! Per-(session, doc) in-memory cache of `DocumentSnapshot`. Used by
//! the co-edit guard to absorb tool-call bursts — the same doc fetched
//! within a 5-second window returns the cached snapshot instead of
//! re-hitting `documents.get`. Not persistent across process restarts
//! (cold cache → one extra GET on the first call after startup).

use crate::gdocs::domain::{DocumentId, DocumentSnapshot};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct Entry {
    snapshot: DocumentSnapshot,
    fetched_at: Instant,
}

/// In-process snapshot cache keyed on `(agent_session_id, DocumentId)`.
/// TTL is operator-configured (see `GDocsConfig::revision_cache_ttl`).
///
/// Construct ONE per process and share via `Arc` — the dispatcher layer
/// wires this through `OnceCell` in `gdocs_tools.rs`.
pub struct OutlineCache {
    ttl: Duration,
    map: Mutex<HashMap<(String, DocumentId), Entry>>,
}

impl OutlineCache {
    /// Construct an empty cache with the given entry TTL.
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            map: Mutex::new(HashMap::new()),
        }
    }

    /// Return the cached snapshot if it is younger than the TTL.
    /// `None` means "fetch fresh".
    pub fn get_fresh(&self, session_id: &str, doc_id: &DocumentId) -> Option<DocumentSnapshot> {
        let guard = self.map.lock().expect("outline cache mutex poisoned");
        guard
            .get(&(session_id.to_string(), doc_id.clone()))
            .and_then(|e| {
                if e.fetched_at.elapsed() < self.ttl {
                    Some(e.snapshot.clone())
                } else {
                    None
                }
            })
    }

    /// Insert or replace the cached snapshot. Call after `documents.get`
    /// AND after every successful `batch_update` (with `invalidate` then
    /// `put` of the post-write snapshot).
    pub fn put(&self, session_id: &str, doc_id: &DocumentId, snapshot: DocumentSnapshot) {
        let mut guard = self.map.lock().expect("outline cache mutex poisoned");
        guard.insert(
            (session_id.to_string(), doc_id.clone()),
            Entry {
                snapshot,
                fetched_at: Instant::now(),
            },
        );
    }

    /// Drop the cached snapshot. Use after a write so the next read
    /// is forced to fetch fresh data.
    pub fn invalidate(&self, session_id: &str, doc_id: &DocumentId) {
        let mut guard = self.map.lock().expect("outline cache mutex poisoned");
        guard.remove(&(session_id.to_string(), doc_id.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::domain::{RevisionId, TabSnapshot};

    fn fake_snapshot() -> DocumentSnapshot {
        DocumentSnapshot {
            doc_id: DocumentId("d1".into()),
            revision_id: RevisionId("r1".into()),
            title: "Test".into(),
            tabs: vec![TabSnapshot {
                tab_id: None,
                paragraphs: vec![],
            }],
        }
    }

    #[test]
    fn put_and_get_within_ttl() {
        let c = OutlineCache::new(Duration::from_secs(5));
        c.put("s1", &DocumentId("d1".into()), fake_snapshot());
        let got = c.get_fresh("s1", &DocumentId("d1".into()));
        assert!(got.is_some());
        assert_eq!(got.unwrap().title, "Test");
    }

    #[test]
    fn miss_when_session_or_doc_differs() {
        let c = OutlineCache::new(Duration::from_secs(5));
        c.put("s1", &DocumentId("d1".into()), fake_snapshot());
        assert!(c.get_fresh("s2", &DocumentId("d1".into())).is_none());
        assert!(c.get_fresh("s1", &DocumentId("d2".into())).is_none());
    }

    #[test]
    fn expires_after_ttl() {
        let c = OutlineCache::new(Duration::from_millis(10));
        c.put("s1", &DocumentId("d1".into()), fake_snapshot());
        std::thread::sleep(Duration::from_millis(30));
        assert!(c.get_fresh("s1", &DocumentId("d1".into())).is_none());
    }

    #[test]
    fn invalidate_removes_entry() {
        let c = OutlineCache::new(Duration::from_secs(5));
        c.put("s1", &DocumentId("d1".into()), fake_snapshot());
        c.invalidate("s1", &DocumentId("d1".into()));
        assert!(c.get_fresh("s1", &DocumentId("d1".into())).is_none());
    }
}
