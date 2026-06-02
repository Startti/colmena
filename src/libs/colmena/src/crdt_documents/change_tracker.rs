//! In-memory rotative buffer of recent `Y.Doc` mutations per artifact.
//!
//! Used by `crdt_doc_get_recent_changes` (added in Task 19) to give the LLM
//! a human-readable summary of what changed since a previous turn. v1 caps
//! at 1000 events per artifact (oldest dropped on overflow).

use crate::crdt_documents::ArtifactId;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};

const CAP_PER_ARTIFACT: usize = 1000;

#[derive(Debug, Clone)]
pub struct ChangeEvent {
    pub event_id: u64,
    pub timestamp_ms: i64,
    pub origin: String,
    pub summary: String,
}

#[derive(Default)]
struct PerArtifact {
    events: VecDeque<ChangeEvent>,
    next_id: u64,
}

pub struct ChangeTracker {
    by_artifact: Mutex<HashMap<String, PerArtifact>>,
}

impl ChangeTracker {
    pub fn new() -> Self {
        Self {
            by_artifact: Mutex::new(HashMap::new()),
        }
    }

    pub fn record(&self, id: &ArtifactId, origin: &str, summary: &str) {
        let mut guard = self.by_artifact.lock();
        let entry = guard.entry(id.to_string()).or_default();
        let ev = ChangeEvent {
            event_id: entry.next_id,
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            origin: origin.to_string(),
            summary: summary.to_string(),
        };
        entry.next_id += 1;
        if entry.events.len() == CAP_PER_ARTIFACT {
            entry.events.pop_front();
        }
        entry.events.push_back(ev);
    }

    pub fn since(&self, id: &ArtifactId, since: Option<u64>) -> Vec<ChangeEvent> {
        let guard = self.by_artifact.lock();
        let Some(entry) = guard.get(id.as_str()) else {
            return Vec::new();
        };
        match since {
            None => entry.events.iter().cloned().collect(),
            Some(s) => entry
                .events
                .iter()
                .filter(|e| e.event_id > s)
                .cloned()
                .collect(),
        }
    }
}

impl Default for ChangeTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_since_filters() {
        let t = ChangeTracker::new();
        let id = ArtifactId::new();
        t.record(&id, "agent:test", "set A1");
        t.record(&id, "agent:test", "set B1");
        let all = t.since(&id, None);
        assert_eq!(all.len(), 2);
        let after_first = t.since(&id, Some(all[0].event_id));
        assert_eq!(after_first.len(), 1);
        assert_eq!(after_first[0].summary, "set B1");
    }

    #[test]
    fn caps_at_1000() {
        let t = ChangeTracker::new();
        let id = ArtifactId::new();
        for i in 0..1500 {
            t.record(&id, "x", &format!("ev{i}"));
        }
        let all = t.since(&id, None);
        assert_eq!(all.len(), 1000);
    }

    #[test]
    fn empty_for_unknown_artifact() {
        let t = ChangeTracker::new();
        let id = ArtifactId::new();
        assert!(t.since(&id, None).is_empty());
    }
}
