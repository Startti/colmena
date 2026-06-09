//! `apply_edits` use case — atomic compound of N sub-edits.
//!
//! Runs the co-edit guard ONCE for the union scope, resolves each
//! sub-edit's batchUpdate fragments, concatenates them, and POSTs a
//! single `batchUpdate` to Google. If any sub-edit fails resolution
//! (`AmbiguousMatch`, `TextNotFound`, `ConfirmManyMatches`) the entire
//! compound is rejected before any write.
//!
//! v1 surface intentionally small — style + named-range variants are
//! deferred to v1.1.

use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::markdown_to_docs_ops::{
    markdown_to_requests, InsertionPoint,
};
use crate::gdocs::application::co_edit_guard::{run_guard, GuardContext};
use crate::gdocs::application::insert::reject_table_markdown;
use crate::gdocs::domain::{
    ChangeKind, ChangeRecord, DocsError, DocumentId, DocumentSnapshot, EditResult, OutlineEntry,
    ParagraphSnapshot, Scope, TabId,
};
use regex::RegexBuilder;

/// Sub-edit variants accepted in v1 `apply_edits`.
#[derive(Debug, Clone)]
pub enum ApplyEditOp {
    ReplaceText {
        find: String,
        replace: String,
        scope: Scope,
    },
    InsertAfterText {
        anchor: String,
        new_markdown: String,
    },
    DeleteText {
        find: String,
        scope: Scope,
    },
}

#[derive(Debug, Clone)]
pub struct ApplyEditsInput {
    pub edits: Vec<ApplyEditOp>,
}

pub async fn run(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    input: ApplyEditsInput,
) -> Result<EditResult, DocsError> {
    if input.edits.is_empty() {
        return Err(DocsError::InvalidArgs(
            "apply_edits: edits list is empty".into(),
        ));
    }

    // Pre-check: any markdown in an InsertAfterText must not contain a table.
    for e in &input.edits {
        if let ApplyEditOp::InsertAfterText { new_markdown, .. } = e {
            reject_table_markdown(new_markdown)?;
        }
    }

    // Run the guard ONCE with Scope::All — union scope for safety.
    let guard = run_guard(ctx, doc_id, &Scope::All).await?;
    let snap = &guard.snapshot;

    let mut all_requests: Vec<serde_json::Value> = Vec::new();
    let mut all_changes: Vec<ChangeRecord> = Vec::new();
    let mut lossy = Vec::new();

    for op in input.edits.into_iter() {
        match op {
            ApplyEditOp::ReplaceText {
                find,
                replace,
                scope,
            } => {
                let rs = crate::gdocs::application::scope_resolver::resolve(&scope, snap)?;
                let re = RegexBuilder::new(&regex::escape(&find))
                    .case_insensitive(false)
                    .build()
                    .map_err(|e| DocsError::InvalidArgs(format!("regex: {e}")))?;
                let mut hits: Vec<(u32, usize, usize)> = Vec::new();
                for tab in &snap.tabs {
                    if let Some(t) = &rs.tab_id {
                        if tab.tab_id.as_ref() != Some(t) {
                            continue;
                        }
                    }
                    for p in &tab.paragraphs {
                        if p.n < rs.paragraph_start || p.n > rs.paragraph_end {
                            continue;
                        }
                        for m in re.find_iter(&p.text) {
                            hits.push((p.n, m.start(), m.end() - m.start()));
                        }
                    }
                }
                if hits.is_empty() {
                    return Err(DocsError::TextNotFound {
                        find,
                        fuzzy_suggestions: vec![],
                    });
                }
                let mut sorted = hits.clone();
                sorted.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
                for (n, off, len) in &sorted {
                    let p = lookup_para(snap, *n);
                    let s = p.start_index + utf16_len(&p.text[..*off]);
                    let e = s + utf16_len(&p.text[*off..*off + *len]);
                    all_requests.push(serde_json::json!({
                        "deleteContentRange": {"range": {"startIndex": s, "endIndex": e}}
                    }));
                    all_requests.push(serde_json::json!({
                        "insertText": {"location": {"index": s}, "text": replace.clone()}
                    }));
                    all_changes.push(ChangeRecord {
                        kind: ChangeKind::Replace,
                        paragraph: *n,
                        before: Some(p.text[*off..*off + *len].to_string()),
                        after: Some(replace.clone()),
                        tab_id: lookup_tab_id(snap, *n),
                    });
                }
            }
            ApplyEditOp::DeleteText { find, scope } => {
                let rs = crate::gdocs::application::scope_resolver::resolve(&scope, snap)?;
                let re = RegexBuilder::new(&regex::escape(&find))
                    .case_insensitive(false)
                    .build()
                    .map_err(|e| DocsError::InvalidArgs(format!("regex: {e}")))?;
                let mut hits: Vec<(u32, usize, usize)> = Vec::new();
                for tab in &snap.tabs {
                    if let Some(t) = &rs.tab_id {
                        if tab.tab_id.as_ref() != Some(t) {
                            continue;
                        }
                    }
                    for p in &tab.paragraphs {
                        if p.n < rs.paragraph_start || p.n > rs.paragraph_end {
                            continue;
                        }
                        for m in re.find_iter(&p.text) {
                            hits.push((p.n, m.start(), m.end() - m.start()));
                        }
                    }
                }
                if hits.is_empty() {
                    return Err(DocsError::TextNotFound {
                        find,
                        fuzzy_suggestions: vec![],
                    });
                }
                let mut sorted = hits.clone();
                sorted.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
                for (n, off, len) in &sorted {
                    let p = lookup_para(snap, *n);
                    let s = p.start_index + utf16_len(&p.text[..*off]);
                    let e = s + utf16_len(&p.text[*off..*off + *len]);
                    all_requests.push(serde_json::json!({
                        "deleteContentRange": {"range": {"startIndex": s, "endIndex": e}}
                    }));
                    all_changes.push(ChangeRecord {
                        kind: ChangeKind::Delete,
                        paragraph: *n,
                        before: Some(p.text[*off..*off + *len].to_string()),
                        after: None,
                        tab_id: lookup_tab_id(snap, *n),
                    });
                }
            }
            ApplyEditOp::InsertAfterText {
                anchor,
                new_markdown,
            } => {
                let (n, off, len, tab_id) = find_anchor_first(snap, &anchor)?;
                let p = lookup_para(snap, n);
                let after_byte = off + len;
                let index = p.start_index + utf16_len(&p.text[..after_byte]);
                let conv = markdown_to_requests(
                    &new_markdown,
                    InsertionPoint::Index {
                        index,
                        tab_id: tab_id.clone(),
                    },
                );
                lossy.extend(conv.lossy);
                all_requests.extend(conv.requests);
                all_changes.push(ChangeRecord {
                    kind: ChangeKind::Insert,
                    paragraph: n,
                    before: None,
                    after: Some(new_markdown.chars().take(120).collect()),
                    tab_id,
                });
            }
        }
    }

    if all_requests.is_empty() {
        return Err(DocsError::InvalidArgs(
            "apply_edits: no batchUpdate requests produced".into(),
        ));
    }

    let result = ctx
        .client
        .batch_update(doc_id, all_requests, Some(&snap.revision_id))
        .await?;
    ctx.cache.invalidate(ctx.session_id, doc_id);
    ctx.revisions
        .put(ctx.session_id, doc_id, &result.revision_id_after)
        .await?;
    let fresh = ctx.client.get(doc_id).await?;
    ctx.cache.put(ctx.session_id, doc_id, fresh.clone());

    Ok(EditResult {
        changes: all_changes,
        revision_id_after: result.revision_id_after,
        outline_snapshot: build_outline(&fresh),
        lossy_conversions: lossy,
        pending_human_changes_outside_scope: guard.soft_warnings,
    })
}

// ── Helpers ────────────────────────────────────────────────────────

fn find_anchor_first(
    snap: &DocumentSnapshot,
    anchor: &str,
) -> Result<(u32, usize, usize, Option<TabId>), DocsError> {
    if anchor.is_empty() {
        return Err(DocsError::InvalidArgs(
            "anchor text must not be empty".into(),
        ));
    }
    for tab in &snap.tabs {
        for p in &tab.paragraphs {
            if let Some(idx) = p.text.find(anchor) {
                return Ok((p.n, idx, anchor.len(), tab.tab_id.clone()));
            }
        }
    }
    Err(DocsError::TextNotFound {
        find: anchor.into(),
        fuzzy_suggestions: vec![],
    })
}

fn lookup_para(s: &DocumentSnapshot, n: u32) -> &ParagraphSnapshot {
    s.tabs
        .iter()
        .flat_map(|t| t.paragraphs.iter())
        .find(|p| p.n == n)
        .expect("paragraph exists")
}

fn lookup_tab_id(s: &DocumentSnapshot, n: u32) -> Option<TabId> {
    s.tabs
        .iter()
        .find(|t| t.paragraphs.iter().any(|p| p.n == n))
        .and_then(|t| t.tab_id.clone())
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

fn build_outline(s: &DocumentSnapshot) -> Vec<OutlineEntry> {
    s.tabs
        .iter()
        .flat_map(|t| {
            t.paragraphs.iter().map(|p| OutlineEntry {
                paragraph: p.n,
                tab_id: t.tab_id.clone(),
                kind: p.kind,
                text_preview: p.text.chars().take(80).collect(),
            })
        })
        .collect()
}
