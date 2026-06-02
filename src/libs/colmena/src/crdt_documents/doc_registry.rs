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
    ///
    /// Uses atomic `DashMap.entry()` semantics to ensure concurrent calls with
    /// the same id always return the same Arc — preventing TOCTOU races where
    /// two threads could each create a Doc and leave divergent Arcs.
    pub fn get_or_create(&self, artifact_id: &str) -> Arc<Doc> {
        self.docs
            .entry(artifact_id.to_string())
            .or_insert_with(|| Arc::new(Doc::new()))
            .clone()
    }

    pub fn get(&self, artifact_id: &str) -> Option<Arc<Doc>> {
        self.docs.get(artifact_id).map(|r| r.value().clone())
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
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

    #[test]
    fn get_or_create_is_idempotent_across_many_calls() {
        let reg = DocRegistry::new();
        let first = reg.get_or_create("art1");
        for _ in 0..100 {
            let again = reg.get_or_create("art1");
            assert!(Arc::ptr_eq(&first, &again));
        }
        assert_eq!(reg.len(), 1);
    }
}
