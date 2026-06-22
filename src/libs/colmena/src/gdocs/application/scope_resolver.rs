//! Translate an LLM-facing [`Scope`] into a concrete paragraph-range
//! `(paragraph_start, paragraph_end, tab_id)` against a snapshot.
//!
//! Used by every content-addressed edit use case (replace_text,
//! insert_after_text, style_text, etc.) to bound the search.

use crate::gdocs::domain::{
    DocsError, DocumentSnapshot, ParagraphKind, ParagraphSnapshot, Scope, TabId,
};

/// Result of scope resolution. `paragraph_start` and `paragraph_end`
/// are 1-based and inclusive on both ends.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedScope {
    pub tab_id: Option<TabId>,
    pub paragraph_start: u32,
    pub paragraph_end: u32,
}

impl ResolvedScope {
    /// True if `(tab, n)` falls within this resolved scope.
    ///
    /// - When `self.tab_id` is `None` the scope is doc-wide; any tab matches.
    /// - When `self.tab_id` is `Some(x)` the change's tab must equal `x`.
    /// - `n` must be in `[paragraph_start, paragraph_end]` (inclusive).
    pub fn contains_paragraph(&self, tab: Option<&TabId>, n: u32) -> bool {
        let tab_ok = match (&self.tab_id, tab) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(a), Some(b)) => a == b,
        };
        tab_ok && n >= self.paragraph_start && n <= self.paragraph_end
    }
}

/// Translate `scope` against `snap` into a `ResolvedScope`.
pub fn resolve(scope: &Scope, snap: &DocumentSnapshot) -> Result<ResolvedScope, DocsError> {
    use Scope::*;
    match scope {
        All => {
            let first = snap
                .tabs
                .iter()
                .flat_map(|t| t.paragraphs.iter())
                .map(|p| p.n)
                .min()
                .unwrap_or(1);
            let last = snap
                .tabs
                .iter()
                .flat_map(|t| t.paragraphs.iter())
                .map(|p| p.n)
                .max()
                .unwrap_or(first);
            Ok(ResolvedScope {
                tab_id: None,
                paragraph_start: first,
                paragraph_end: last,
            })
        }
        Tab { tab_id } => {
            let tab = snap
                .tabs
                .iter()
                .find(|t| t.tab_id.as_ref() == Some(tab_id))
                .ok_or_else(|| DocsError::TabNotFound(tab_id.0.clone()))?;
            let first = tab.paragraphs.iter().map(|p| p.n).min().unwrap_or(1);
            let last = tab.paragraphs.iter().map(|p| p.n).max().unwrap_or(first);
            Ok(ResolvedScope {
                tab_id: Some(tab_id.clone()),
                paragraph_start: first,
                paragraph_end: last,
            })
        }
        Paragraph { n } => {
            let n = *n;
            let tab = snap
                .tabs
                .iter()
                .find(|t| t.paragraphs.iter().any(|p| p.n == n));
            let tab =
                tab.ok_or_else(|| DocsError::InvalidArgs(format!("paragraph {n} not found")))?;
            Ok(ResolvedScope {
                tab_id: tab.tab_id.clone(),
                paragraph_start: n,
                paragraph_end: n,
            })
        }
        UnderHeading { heading } => {
            let level = heading_level(heading);
            // Find the heading paragraph.
            let mut found: Option<(u32, Option<TabId>)> = None;
            'outer: for tab in &snap.tabs {
                for p in &tab.paragraphs {
                    if matches_heading(p, heading) {
                        found = Some((p.n, tab.tab_id.clone()));
                        break 'outer;
                    }
                }
            }
            let (start, tab_id) = found
                .ok_or_else(|| DocsError::InvalidArgs(format!("heading '{heading}' not found")))?;
            // End at the next paragraph that is a heading of same-or-higher
            // level (lower or equal `level` number) AFTER `start`.
            let mut end_excl = u32::MAX;
            for tab in &snap.tabs {
                for p in &tab.paragraphs {
                    if p.n <= start {
                        continue;
                    }
                    if let Some(l2) = paragraph_heading_level(p) {
                        if l2 <= level {
                            end_excl = end_excl.min(p.n);
                            break;
                        }
                    }
                }
            }
            let last_p = snap
                .tabs
                .iter()
                .flat_map(|t| t.paragraphs.iter())
                .map(|p| p.n)
                .max()
                .unwrap_or(start);
            let paragraph_end = if end_excl == u32::MAX {
                last_p
            } else {
                end_excl - 1
            };
            // If start == last_p, scope is empty — invalid for the
            // use cases that read it, but harmless to return.
            Ok(ResolvedScope {
                tab_id,
                paragraph_start: start + 1,
                paragraph_end,
            })
        }
        BetweenHeadings { after, before } => {
            let start = find_heading_paragraph(snap, after)
                .ok_or_else(|| DocsError::InvalidArgs(format!("heading '{after}' not found")))?;
            let end = match before {
                Some(b) => find_heading_paragraph(snap, b)
                    .ok_or_else(|| DocsError::InvalidArgs(format!("heading '{b}' not found")))?,
                None => {
                    snap.tabs
                        .iter()
                        .flat_map(|t| t.paragraphs.iter())
                        .map(|p| p.n)
                        .max()
                        .unwrap_or(start)
                        + 1
                }
            };
            if end <= start + 1 {
                return Err(DocsError::InvalidArgs(
                    "between_headings: 'before' is not after 'after'".into(),
                ));
            }
            Ok(ResolvedScope {
                tab_id: None,
                paragraph_start: start + 1,
                paragraph_end: end - 1,
            })
        }
    }
}

/// Strip leading `#`s from a heading reference and compare against a
/// paragraph's plain text. Returns true if the paragraph IS the target
/// heading.
fn matches_heading(p: &ParagraphSnapshot, target: &str) -> bool {
    let target = target.trim_start_matches('#').trim();
    p.text.trim() == target
        && matches!(
            p.kind,
            ParagraphKind::Heading1
                | ParagraphKind::Heading2
                | ParagraphKind::Heading3
                | ParagraphKind::Heading4
                | ParagraphKind::Heading5
                | ParagraphKind::Heading6
                | ParagraphKind::Title
                | ParagraphKind::Subtitle
        )
}

/// Return 1..=6 for headings; None for non-heading paragraphs.
fn paragraph_heading_level(p: &ParagraphSnapshot) -> Option<u8> {
    match p.kind {
        ParagraphKind::Heading1 => Some(1),
        ParagraphKind::Heading2 => Some(2),
        ParagraphKind::Heading3 => Some(3),
        ParagraphKind::Heading4 => Some(4),
        ParagraphKind::Heading5 => Some(5),
        ParagraphKind::Heading6 => Some(6),
        // Title/Subtitle don't participate in heading nesting in v1.
        _ => None,
    }
}

/// Count the leading `#` chars (clamped to 1..=6).
fn heading_level(s: &str) -> u8 {
    let hashes = s.chars().take_while(|c| *c == '#').count() as u8;
    hashes.clamp(1, 6)
}

fn find_heading_paragraph(snap: &DocumentSnapshot, h: &str) -> Option<u32> {
    snap.tabs
        .iter()
        .flat_map(|t| t.paragraphs.iter())
        .find(|p| matches_heading(p, h))
        .map(|p| p.n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::domain::{DocumentId, RevisionId, TabSnapshot};

    fn snap(paras: Vec<(u32, ParagraphKind, &str)>) -> DocumentSnapshot {
        DocumentSnapshot {
            doc_id: DocumentId("d".into()),
            revision_id: RevisionId("r".into()),
            title: "T".into(),
            tabs: vec![TabSnapshot {
                tab_id: None,
                paragraphs: paras
                    .into_iter()
                    .map(|(n, kind, t)| ParagraphSnapshot {
                        n,
                        kind,
                        text: t.into(),
                        start_index: 0,
                        end_index: 0,
                    })
                    .collect(),
                tables: vec![],
            }],
        }
    }

    #[test]
    fn all_returns_full_range() {
        let s = snap(vec![
            (1, ParagraphKind::Paragraph, "a"),
            (2, ParagraphKind::Paragraph, "b"),
            (3, ParagraphKind::Paragraph, "c"),
        ]);
        let r = resolve(&Scope::All, &s).unwrap();
        assert_eq!(r.paragraph_start, 1);
        assert_eq!(r.paragraph_end, 3);
    }

    #[test]
    fn paragraph_scope_returns_single() {
        let s = snap(vec![
            (1, ParagraphKind::Paragraph, "a"),
            (2, ParagraphKind::Paragraph, "b"),
        ]);
        let r = resolve(&Scope::Paragraph { n: 2 }, &s).unwrap();
        assert_eq!(r.paragraph_start, 2);
        assert_eq!(r.paragraph_end, 2);
    }

    #[test]
    fn paragraph_scope_missing_paragraph_errors() {
        let s = snap(vec![(1, ParagraphKind::Paragraph, "a")]);
        let r = resolve(&Scope::Paragraph { n: 5 }, &s);
        assert!(matches!(r, Err(DocsError::InvalidArgs(_))));
    }

    #[test]
    fn under_heading_extends_to_next_same_level() {
        let s = snap(vec![
            (1, ParagraphKind::Heading2, "Sección 2"),
            (2, ParagraphKind::Paragraph, "primer texto"),
            (3, ParagraphKind::Paragraph, "segundo texto"),
            (4, ParagraphKind::Heading2, "Sección 3"),
            (5, ParagraphKind::Paragraph, "fuera del scope"),
        ]);
        let r = resolve(
            &Scope::UnderHeading {
                heading: "## Sección 2".into(),
            },
            &s,
        )
        .unwrap();
        assert_eq!(r.paragraph_start, 2);
        assert_eq!(r.paragraph_end, 3);
    }

    #[test]
    fn under_heading_extends_to_eof_if_no_next() {
        let s = snap(vec![
            (1, ParagraphKind::Heading2, "Solo Sección"),
            (2, ParagraphKind::Paragraph, "después"),
            (3, ParagraphKind::Paragraph, "y más"),
        ]);
        let r = resolve(
            &Scope::UnderHeading {
                heading: "## Solo Sección".into(),
            },
            &s,
        )
        .unwrap();
        assert_eq!(r.paragraph_start, 2);
        assert_eq!(r.paragraph_end, 3);
    }

    #[test]
    fn between_headings_inclusive_exclusive() {
        let s = snap(vec![
            (1, ParagraphKind::Heading1, "A"),
            (2, ParagraphKind::Paragraph, "x"),
            (3, ParagraphKind::Heading1, "B"),
            (4, ParagraphKind::Paragraph, "y"),
        ]);
        let r = resolve(
            &Scope::BetweenHeadings {
                after: "# A".into(),
                before: Some("# B".into()),
            },
            &s,
        )
        .unwrap();
        assert_eq!(r.paragraph_start, 2);
        assert_eq!(r.paragraph_end, 2);
    }

    #[test]
    fn between_headings_none_before_means_eof() {
        let s = snap(vec![
            (1, ParagraphKind::Heading1, "Inicio"),
            (2, ParagraphKind::Paragraph, "x"),
            (3, ParagraphKind::Paragraph, "y"),
        ]);
        let r = resolve(
            &Scope::BetweenHeadings {
                after: "# Inicio".into(),
                before: None,
            },
            &s,
        )
        .unwrap();
        assert_eq!(r.paragraph_start, 2);
        assert_eq!(r.paragraph_end, 3);
    }

    #[test]
    fn tab_scope_unknown_id_errors() {
        let s = snap(vec![(1, ParagraphKind::Paragraph, "a")]);
        let r = resolve(
            &Scope::Tab {
                tab_id: TabId("missing".into()),
            },
            &s,
        );
        assert!(matches!(r, Err(DocsError::TabNotFound(_))));
    }

    #[test]
    fn heading_level_count() {
        assert_eq!(heading_level("# A"), 1);
        assert_eq!(heading_level("## A"), 2);
        assert_eq!(heading_level("### A"), 3);
        assert_eq!(heading_level("#### A"), 4);
        assert_eq!(heading_level("###### A"), 6);
        assert_eq!(heading_level("####### A"), 6); // clamped
        assert_eq!(heading_level("plain"), 1); // no hashes → clamped
    }

    #[test]
    fn contains_paragraph_respects_tab_and_range() {
        let rs = ResolvedScope {
            tab_id: Some(TabId("plan".into())),
            paragraph_start: 5,
            paragraph_end: 10,
        };
        // In tab, in range.
        assert!(rs.contains_paragraph(Some(&TabId("plan".into())), 5));
        assert!(rs.contains_paragraph(Some(&TabId("plan".into())), 10));
        assert!(rs.contains_paragraph(Some(&TabId("plan".into())), 7));
        // In tab, out of range.
        assert!(!rs.contains_paragraph(Some(&TabId("plan".into())), 4));
        assert!(!rs.contains_paragraph(Some(&TabId("plan".into())), 11));
        // Different tab.
        assert!(!rs.contains_paragraph(Some(&TabId("anexo".into())), 7));
        // Scope tab is None (doc-wide) — any tab matches.
        let rs_no_tab = ResolvedScope {
            tab_id: None,
            paragraph_start: 1,
            paragraph_end: 3,
        };
        assert!(rs_no_tab.contains_paragraph(None, 2));
        assert!(rs_no_tab.contains_paragraph(Some(&TabId("anexo".into())), 2));
        // Out-of-range still rejected even when tab matches.
        assert!(!rs_no_tab.contains_paragraph(None, 4));
        // Scope wants a specific tab but change has no tab — reject.
        assert!(!rs.contains_paragraph(None, 7));
    }
}
