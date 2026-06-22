//! Shared fixtures + helpers for application-layer tests (Task 19).
//!
//! Every test in this layer instantiates a [`TestRig`] for the
//! `MockDocsClient` + cache + revision-store wiring, then builds
//! `DocumentSnapshot`s with the [`snap`] / [`snap_multi_tab`] helpers
//! to avoid typing the full struct each time.

#![cfg(test)]

use crate::gdocs::domain::traits::MockDocsClient;
use crate::gdocs::domain::{
    DocumentId, DocumentSnapshot, ParagraphKind, ParagraphSnapshot, RevisionId, TabId, TabSnapshot,
};
use crate::gdocs::infrastructure::outline_cache::OutlineCache;
use crate::gdocs::infrastructure::revision_store::InMemoryRevisionStore;
use std::time::Duration;

/// One-stop wiring for application tests.
pub struct TestRig {
    pub client: MockDocsClient,
    pub cache: OutlineCache,
    pub revisions: InMemoryRevisionStore,
}

impl Default for TestRig {
    fn default() -> Self {
        Self {
            client: MockDocsClient::new(),
            cache: OutlineCache::new(Duration::from_secs(5)),
            revisions: InMemoryRevisionStore::new(),
        }
    }
}

impl TestRig {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Build a single-tab `DocumentSnapshot` from `(n, kind, text, start, end)` tuples.
pub fn snap(rev: &str, paras: Vec<(u32, ParagraphKind, &str, u32, u32)>) -> DocumentSnapshot {
    DocumentSnapshot {
        doc_id: DocumentId("doc1".into()),
        revision_id: RevisionId(rev.into()),
        title: "T".into(),
        tabs: vec![TabSnapshot {
            tab_id: None,
            paragraphs: paras
                .into_iter()
                .map(|(n, kind, t, s, e)| ParagraphSnapshot {
                    n,
                    kind,
                    text: t.into(),
                    start_index: s,
                    end_index: e,
                })
                .collect(),
            tables: vec![],
        }],
    }
}

/// Build a multi-tab `DocumentSnapshot`.
#[allow(clippy::type_complexity, dead_code)]
pub fn snap_multi_tab(
    rev: &str,
    tabs: Vec<(Option<&str>, Vec<(u32, ParagraphKind, &str, u32, u32)>)>,
) -> DocumentSnapshot {
    DocumentSnapshot {
        doc_id: DocumentId("doc1".into()),
        revision_id: RevisionId(rev.into()),
        title: "T".into(),
        tabs: tabs
            .into_iter()
            .map(|(tab_id, paras)| TabSnapshot {
                tab_id: tab_id.map(|s| TabId(s.into())),
                paragraphs: paras
                    .into_iter()
                    .map(|(n, kind, t, s, e)| ParagraphSnapshot {
                        n,
                        kind,
                        text: t.into(),
                        start_index: s,
                        end_index: e,
                    })
                    .collect(),
                tables: vec![],
            })
            .collect(),
    }
}

pub fn doc_id() -> DocumentId {
    DocumentId("doc1".into())
}
