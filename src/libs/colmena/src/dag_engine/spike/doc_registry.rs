//! In-memory registry of `yrs::Doc` keyed by artifact id.
//!
//! Used by the spike server to look up (or lazily create) the CRDT
//! document for an artifact. Backed by `DashMap` for concurrent access
//! without an outer lock.

use dashmap::DashMap;
use std::sync::Arc;
use yrs::Doc;

#[derive(Default)]
pub struct DocRegistry {
    docs: DashMap<String, Arc<Doc>>,
}

impl DocRegistry {
    pub fn new() -> Self {
        Self {
            docs: DashMap::new(),
        }
    }

    /// Returns the doc for `artifact_id`, creating an empty one if absent.
    pub fn get_or_create(&self, artifact_id: &str) -> Arc<Doc> {
        if let Some(existing) = self.docs.get(artifact_id) {
            return existing.clone();
        }
        let doc = Arc::new(Doc::new());
        self.docs.insert(artifact_id.to_string(), doc.clone());
        doc
    }

    pub fn get(&self, artifact_id: &str) -> Option<Arc<Doc>> {
        self.docs.get(artifact_id).map(|r| r.value().clone())
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_or_create_returns_same_arc_on_repeated_calls() {
        let reg = DocRegistry::new();
        let a = reg.get_or_create("art1");
        let b = reg.get_or_create("art1");
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn different_ids_get_different_docs() {
        let reg = DocRegistry::new();
        let a = reg.get_or_create("art1");
        let b = reg.get_or_create("art2");
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn get_returns_none_for_missing_id() {
        let reg = DocRegistry::new();
        assert!(reg.get("missing").is_none());
    }
}
