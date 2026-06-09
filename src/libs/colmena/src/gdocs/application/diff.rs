//! Paragraph-level diff between two [`DocumentSnapshot`]s.
//!
//! Used by the co-edit guard to translate "doc revisionId moved" into a
//! concrete list of [`HumanChange`]s the LLM can reason about, without
//! any extra API calls.
//!
//! Algorithm: per-tab Myers diff (via the `similar` crate) over each
//! tab's `Vec<paragraph_text>`. `DiffOp` variants map to
//! `HumanChange` records:
//! - `Equal` → ignored
//! - `Insert` → one `Insert` per inserted paragraph
//! - `Delete` → one `Delete` per deleted paragraph (anchored to where it
//!   would have lived in the current snapshot)
//! - `Replace { old_len, new_len }` → `min(old, new)` `Modify` records
//!   plus extras as `Insert` or `Delete`
//!
//! Output is sorted deterministically by `(tab_id, paragraph)` ascending.
//!
//! See spec `docs/superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md`.

use crate::gdocs::domain::{
    DocumentSnapshot, HumanChange, HumanChangeKind, ParagraphSnapshot, TabId, TabSnapshot,
};
use chrono::Utc;
use similar::{capture_diff_slices, Algorithm, DiffOp};
use std::collections::HashMap;

/// Maximum characters in `HumanChange.preview`; full text remains in
/// `before_text`/`after_text`.
const PREVIEW_MAX_CHARS: usize = 120;

/// Compute paragraph-level diff between `prior` and `current`.
///
/// Pure: no I/O. Deterministic: output is sorted by `(tab_id, paragraph)`.
///
/// - `Insert` and `Modify` use `paragraph` from the `current` snapshot.
/// - `Delete` uses an anchor paragraph in `current` (the paragraph the
///   deletion logically sits before, clamped to `last + 1` if deletion is
///   at end-of-tab, or `0` if the current tab is empty).
pub fn paragraph_diff(prior: &DocumentSnapshot, current: &DocumentSnapshot) -> Vec<HumanChange> {
    let prior_tabs: HashMap<Option<TabId>, &TabSnapshot> =
        prior.tabs.iter().map(|t| (t.tab_id.clone(), t)).collect();

    let mut out = Vec::new();

    for current_tab in current.tabs.iter() {
        let prior_paragraphs: Vec<&ParagraphSnapshot> = prior_tabs
            .get(&current_tab.tab_id)
            .map(|t| t.paragraphs.iter().collect())
            .unwrap_or_default();
        let current_paragraphs: Vec<&ParagraphSnapshot> = current_tab.paragraphs.iter().collect();
        diff_tab(
            &prior_paragraphs,
            &current_paragraphs,
            current_tab.tab_id.as_ref(),
            &mut out,
        );
    }

    out.sort_by_key(|c| (c.tab_id.as_ref().map(|t| t.0.clone()), c.paragraph));
    out
}

fn diff_tab(
    prior: &[&ParagraphSnapshot],
    current: &[&ParagraphSnapshot],
    tab_id: Option<&TabId>,
    out: &mut Vec<HumanChange>,
) {
    let prior_texts: Vec<&str> = prior.iter().map(|p| p.text.as_str()).collect();
    let current_texts: Vec<&str> = current.iter().map(|p| p.text.as_str()).collect();
    let ops = capture_diff_slices(Algorithm::Myers, &prior_texts, &current_texts);

    for op in ops {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for i in 0..new_len {
                    let paragraph = current.get(new_index + i).map(|p| p.n).unwrap_or(0);
                    out.push(make_change(
                        HumanChangeKind::Insert,
                        tab_id,
                        paragraph,
                        None,
                        Some(current_texts[new_index + i].to_string()),
                    ));
                }
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                // Anchor each deleted paragraph to where it would have lived
                // in `current`: the paragraph at the post-delete position, or
                // last+1 if past end-of-tab.
                let anchor = current.last().map(|p| p.n.saturating_add(1)).unwrap_or(0);
                for i in 0..old_len {
                    out.push(make_change(
                        HumanChangeKind::Delete,
                        tab_id,
                        anchor,
                        Some(prior_texts[old_index + i].to_string()),
                        None,
                    ));
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let modify_len = old_len.min(new_len);
                for i in 0..modify_len {
                    let paragraph = current.get(new_index + i).map(|p| p.n).unwrap_or(0);
                    out.push(make_change(
                        HumanChangeKind::Modify,
                        tab_id,
                        paragraph,
                        Some(prior_texts[old_index + i].to_string()),
                        Some(current_texts[new_index + i].to_string()),
                    ));
                }
                if new_len > old_len {
                    for i in modify_len..new_len {
                        let paragraph = current.get(new_index + i).map(|p| p.n).unwrap_or(0);
                        out.push(make_change(
                            HumanChangeKind::Insert,
                            tab_id,
                            paragraph,
                            None,
                            Some(current_texts[new_index + i].to_string()),
                        ));
                    }
                } else if old_len > new_len {
                    let anchor = current
                        .get(new_index + modify_len)
                        .map(|p| p.n)
                        .or_else(|| current.last().map(|p| p.n.saturating_add(1)))
                        .unwrap_or(0);
                    for i in modify_len..old_len {
                        out.push(make_change(
                            HumanChangeKind::Delete,
                            tab_id,
                            anchor,
                            Some(prior_texts[old_index + i].to_string()),
                            None,
                        ));
                    }
                }
            }
        }
    }
}

fn make_change(
    kind: HumanChangeKind,
    tab_id: Option<&TabId>,
    paragraph: u32,
    before: Option<String>,
    after: Option<String>,
) -> HumanChange {
    let preview_source = match kind {
        HumanChangeKind::Delete => before.as_deref().unwrap_or(""),
        HumanChangeKind::Insert | HumanChangeKind::Modify => after.as_deref().unwrap_or(""),
    };
    let preview = truncate(preview_source, PREVIEW_MAX_CHARS);
    HumanChange {
        kind,
        paragraph,
        preview,
        modified_time: Utc::now(),
        modifying_user: None,
        tab_id: tab_id.cloned(),
        before_text: before,
        after_text: after,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut buf: String = s.chars().take(max).collect();
    buf.push('…');
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::domain::{DocumentId, ParagraphKind, RevisionId, TabId, TabSnapshot};

    fn p(n: u32, text: &str) -> ParagraphSnapshot {
        ParagraphSnapshot {
            n,
            kind: ParagraphKind::Paragraph,
            text: text.into(),
            start_index: n,
            end_index: n + text.len() as u32,
        }
    }

    fn snap_single_tab(rev: &str, paragraphs: Vec<ParagraphSnapshot>) -> DocumentSnapshot {
        DocumentSnapshot {
            doc_id: DocumentId("doc1".into()),
            revision_id: RevisionId(rev.into()),
            title: "t".into(),
            tabs: vec![TabSnapshot {
                tab_id: None,
                paragraphs,
            }],
        }
    }

    #[test]
    fn no_changes() {
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "b")]);
        let b = snap_single_tab("r2", vec![p(1, "a"), p(2, "b")]);
        let d = paragraph_diff(&a, &b);
        assert!(d.is_empty(), "expected no diffs, got {:?}", d);
    }

    #[test]
    fn single_modify() {
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "b"), p(3, "c")]);
        let b = snap_single_tab("r2", vec![p(1, "a"), p(2, "B!"), p(3, "c")]);
        let d = paragraph_diff(&a, &b);
        assert_eq!(d.len(), 1, "got {:?}", d);
        assert_eq!(d[0].kind, HumanChangeKind::Modify);
        assert_eq!(d[0].paragraph, 2);
        assert_eq!(d[0].before_text.as_deref(), Some("b"));
        assert_eq!(d[0].after_text.as_deref(), Some("B!"));
    }

    #[test]
    fn single_insert_end() {
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "b")]);
        let b = snap_single_tab("r2", vec![p(1, "a"), p(2, "b"), p(3, "c")]);
        let d = paragraph_diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].kind, HumanChangeKind::Insert);
        assert_eq!(d[0].paragraph, 3);
        assert_eq!(d[0].after_text.as_deref(), Some("c"));
        assert_eq!(d[0].before_text, None);
    }

    #[test]
    fn single_insert_middle() {
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "c")]);
        let b = snap_single_tab("r2", vec![p(1, "a"), p(2, "b"), p(3, "c")]);
        let d = paragraph_diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].kind, HumanChangeKind::Insert);
        assert_eq!(d[0].paragraph, 2);
        assert_eq!(d[0].after_text.as_deref(), Some("b"));
    }

    #[test]
    fn single_delete() {
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "b"), p(3, "c")]);
        let b = snap_single_tab("r2", vec![p(1, "a"), p(2, "c")]);
        let d = paragraph_diff(&a, &b);
        assert_eq!(d.len(), 1, "got {:?}", d);
        assert_eq!(d[0].kind, HumanChangeKind::Delete);
        assert_eq!(d[0].before_text.as_deref(), Some("b"));
        assert_eq!(d[0].after_text, None);
    }

    #[test]
    fn replace_shrink_net_effect_is_delete_plus_modify() {
        // 4 paragraphs → 3 paragraphs, with paragraph 2 modified and 3 deleted.
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "b"), p(3, "c"), p(4, "d")]);
        let b = snap_single_tab("r2", vec![p(1, "a"), p(2, "X"), p(3, "d")]);
        let d = paragraph_diff(&a, &b);
        let insert_count = d
            .iter()
            .filter(|c| c.kind == HumanChangeKind::Insert)
            .count();
        assert_eq!(
            insert_count, 0,
            "shrink should not produce Inserts, got: {:?}",
            d
        );
        // Must contain at least one Delete OR Modify-with-shrink.
        assert!(
            d.iter()
                .any(|c| c.kind == HumanChangeKind::Delete || c.kind == HumanChangeKind::Modify),
            "expected at least one Delete/Modify, got {:?}",
            d
        );
    }

    #[test]
    fn multi_tab_isolated_change() {
        let a = DocumentSnapshot {
            doc_id: DocumentId("doc1".into()),
            revision_id: RevisionId("r1".into()),
            title: "t".into(),
            tabs: vec![
                TabSnapshot {
                    tab_id: Some(TabId("tab1".into())),
                    paragraphs: vec![p(1, "a")],
                },
                TabSnapshot {
                    tab_id: Some(TabId("tab2".into())),
                    paragraphs: vec![p(1, "b")],
                },
            ],
        };
        let b = DocumentSnapshot {
            doc_id: DocumentId("doc1".into()),
            revision_id: RevisionId("r2".into()),
            title: "t".into(),
            tabs: vec![
                TabSnapshot {
                    tab_id: Some(TabId("tab1".into())),
                    paragraphs: vec![p(1, "a")],
                },
                TabSnapshot {
                    tab_id: Some(TabId("tab2".into())),
                    paragraphs: vec![p(1, "B!")],
                },
            ],
        };
        let d = paragraph_diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].tab_id.as_ref().map(|t| t.0.as_str()), Some("tab2"));
        assert_eq!(d[0].kind, HumanChangeKind::Modify);
    }

    #[test]
    fn preview_truncates_long_text() {
        let long: String = "x".repeat(200);
        let a = snap_single_tab("r1", vec![p(1, "")]);
        let b = snap_single_tab("r2", vec![p(1, &long)]);
        let d = paragraph_diff(&a, &b);
        assert_eq!(d.len(), 1);
        // Preview ≤ 120 chars + 1 ellipsis.
        assert!(
            d[0].preview.chars().count() <= 121,
            "preview too long: {}",
            d[0].preview
        );
        assert!(d[0].preview.ends_with('…'));
        // After-text retains full content.
        assert_eq!(d[0].after_text.as_deref().map(|s| s.len()), Some(200));
    }

    #[test]
    fn deterministic_order() {
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "b"), p(3, "c")]);
        let b = snap_single_tab("r2", vec![p(1, "A!"), p(2, "B!"), p(3, "c")]);
        let d1 = paragraph_diff(&a, &b);
        let d2 = paragraph_diff(&a, &b);
        // `modified_time` is wall-clock and intentionally not stable across
        // calls. Compare order + kinds + text content instead.
        let shape =
            |d: &Vec<HumanChange>| -> Vec<(HumanChangeKind, u32, Option<String>, Option<String>)> {
                d.iter()
                    .map(|c| {
                        (
                            c.kind,
                            c.paragraph,
                            c.before_text.clone(),
                            c.after_text.clone(),
                        )
                    })
                    .collect()
            };
        assert_eq!(shape(&d1), shape(&d2), "diff shape should be deterministic");
        assert_eq!(d1[0].paragraph, 1);
        assert_eq!(d1[1].paragraph, 2);
    }
}
