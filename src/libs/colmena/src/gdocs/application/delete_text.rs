//! `delete_text` use case — content-addressed scoped delete.
//!
//! Mirrors `replace_text` (T16) but emits only `deleteContentRange`
//! requests — no `insertText` follow-up. Ambiguity gates (TextNotFound,
//! ConfirmManyMatches) and write-backwards ordering are identical.

use crate::gdocs::application::co_edit_guard::{run_guard, GuardContext};
use crate::gdocs::domain::{
    ChangeKind, ChangeRecord, DocsError, DocumentId, DocumentSnapshot, EditResult, MatchPreview,
    OutlineEntry, ParagraphSnapshot, Scope, TabId,
};
use regex::RegexBuilder;

#[derive(Debug, Clone)]
pub struct DeleteTextInput {
    pub find: String,
    pub scope: Scope,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub occurrence: Option<u32>,
    pub confirm_many: bool,
}

pub async fn run(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    input: DeleteTextInput,
) -> Result<EditResult, DocsError> {
    let guard = run_guard(ctx, doc_id, &input.scope).await?;
    let snap = &guard.snapshot;
    let scope = &guard.resolved_scope;

    let pattern = if input.whole_word {
        format!(r"\b{}\b", regex::escape(&input.find))
    } else {
        regex::escape(&input.find)
    };
    let re = RegexBuilder::new(&pattern)
        .case_insensitive(!input.case_sensitive)
        .build()
        .map_err(|e| DocsError::InvalidArgs(format!("regex: {e}")))?;

    let mut hits: Vec<(u32, usize, usize)> = Vec::new();
    for tab in &snap.tabs {
        if let Some(t) = &scope.tab_id {
            if tab.tab_id.as_ref() != Some(t) {
                continue;
            }
        }
        for p in &tab.paragraphs {
            if p.n < scope.paragraph_start || p.n > scope.paragraph_end {
                continue;
            }
            for m in re.find_iter(&p.text) {
                hits.push((p.n, m.start(), m.end() - m.start()));
            }
        }
    }

    if let Some(n) = input.occurrence {
        if n == 0 || (n as usize) > hits.len() {
            return Err(DocsError::TextNotFound {
                find: input.find,
                fuzzy_suggestions: vec![],
            });
        }
        hits = vec![hits[(n - 1) as usize]];
    }
    if hits.is_empty() {
        return Err(DocsError::TextNotFound {
            find: input.find,
            fuzzy_suggestions: vec![],
        });
    }
    if hits.len() >= 5 && !input.confirm_many && input.occurrence.is_none() {
        return Err(DocsError::ConfirmManyMatches {
            find: input.find,
            count: hits.len() as u32,
            preview: build_previews(snap, &hits),
        });
    }

    let mut requests: Vec<serde_json::Value> = Vec::new();
    let mut changes: Vec<ChangeRecord> = Vec::new();
    let mut hits_sorted = hits.clone();
    hits_sorted.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));

    for (n, off, len) in &hits_sorted {
        let p = lookup_para(snap, *n);
        let prefix_utf16 = utf16_len(&p.text[..*off]);
        let match_utf16 = utf16_len(&p.text[*off..*off + *len]);
        let start = p.start_index + prefix_utf16;
        let end = start + match_utf16;
        let before: String = p.text[*off..*off + *len].to_string();
        requests.push(serde_json::json!({
            "deleteContentRange": {"range": {"startIndex": start, "endIndex": end}}
        }));
        changes.push(ChangeRecord {
            kind: ChangeKind::Delete,
            paragraph: *n,
            before: Some(before),
            after: None,
            tab_id: lookup_tab_id(snap, *n),
        });
    }

    let result = ctx
        .client
        .batch_update(doc_id, requests, Some(&snap.revision_id))
        .await?;
    ctx.cache.invalidate(ctx.session_id, doc_id);
    ctx.revisions
        .put(ctx.session_id, doc_id, &result.revision_id_after)
        .await?;
    let fresh = ctx.client.get(doc_id).await?;
    ctx.cache.put(ctx.session_id, doc_id, fresh.clone());

    Ok(EditResult {
        changes,
        revision_id_after: result.revision_id_after,
        outline_snapshot: build_outline(&fresh),
        lossy_conversions: vec![],
        pending_human_changes_outside_scope: guard.soft_warnings,
    })
}

// ── Helpers ────────────────────────────────────────────────────────

fn lookup_para(s: &DocumentSnapshot, n: u32) -> &ParagraphSnapshot {
    s.tabs
        .iter()
        .flat_map(|t| t.paragraphs.iter())
        .find(|p| p.n == n)
        .expect("paragraph exists — caller filtered by scope")
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

fn build_previews(snap: &DocumentSnapshot, hits: &[(u32, usize, usize)]) -> Vec<MatchPreview> {
    hits.iter()
        .enumerate()
        .map(|(i, (n, off, _))| {
            let p = lookup_para(snap, *n);
            let byte_start = off.saturating_sub(30);
            let byte_end = (*off + 50).min(p.text.len());
            let preview = take_slice_around(&p.text, byte_start, byte_end);
            MatchPreview {
                n: (i + 1) as u32,
                paragraph: *n,
                preview,
            }
        })
        .collect()
}

fn take_slice_around(s: &str, start: usize, end: usize) -> String {
    let mut start = start;
    let mut end = end.min(s.len());
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    while end < s.len() && !s.is_char_boundary(end) {
        end += 1;
    }
    s.get(start..end).unwrap_or(s).chars().take(80).collect()
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
