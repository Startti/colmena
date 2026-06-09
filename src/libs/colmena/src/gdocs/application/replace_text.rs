//! `replace_text` use case — content-addressed scoped find/replace.
//!
//! The LLM specifies WHAT text to change (a substring + optional
//! disambiguators); the dispatcher resolves UTF-16 positions, runs the
//! co-edit guard, and emits a write-backwards batchUpdate. All ambiguity
//! errors (AmbiguousMatch, TextNotFound, ConfirmManyMatches) surface
//! through `DocsError` and become structured JSON in the dispatcher
//! layer.

use crate::gdocs::application::co_edit_guard::{run_guard, GuardContext};
use crate::gdocs::domain::{
    ChangeKind, ChangeRecord, DocsError, DocumentId, DocumentSnapshot, EditResult,
    MatchPreview, OutlineEntry, ParagraphSnapshot, Scope, TabId,
};
use regex::RegexBuilder;

/// What the dispatcher passes through. Defaults documented in spec §6.
#[derive(Debug, Clone)]
pub struct ReplaceTextInput {
    pub find: String,
    pub replace: String,
    pub scope: Scope,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub dry_run: bool,
    pub confirm_many: bool,
    /// 1-based ordinal of the match to act on. When set, all other
    /// matches are ignored.
    pub occurrence: Option<u32>,
    /// Text that must ALSO appear in the same paragraph for a match
    /// to be considered. Used to disambiguate without forcing the
    /// LLM to invent a longer `find`.
    pub anchor: Option<String>,
}

pub async fn run(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    input: ReplaceTextInput,
) -> Result<EditResult, DocsError> {
    let guard = run_guard(ctx, doc_id, &input.scope).await?;
    let snap = &guard.snapshot;
    let scope = &guard.resolved_scope;

    // 1. Build the regex.
    let pattern = if input.whole_word {
        format!(r"\b{}\b", regex::escape(&input.find))
    } else {
        regex::escape(&input.find)
    };
    let re = RegexBuilder::new(&pattern)
        .case_insensitive(!input.case_sensitive)
        .build()
        .map_err(|e| DocsError::InvalidArgs(format!("regex: {e}")))?;

    // 2. Walk scope, collect (paragraph_n, byte_off_in_text, len).
    let mut hits: Vec<(u32, usize, usize)> = Vec::new();
    for tab in &snap.tabs {
        if let Some(scope_tab) = &scope.tab_id {
            if tab.tab_id.as_ref() != Some(scope_tab) { continue; }
        }
        for p in &tab.paragraphs {
            if p.n < scope.paragraph_start || p.n > scope.paragraph_end { continue; }
            // Anchor filter — both `find` AND `anchor` must appear in
            // the same paragraph text.
            if let Some(a) = &input.anchor {
                if !p.text.contains(a) { continue; }
            }
            for m in re.find_iter(&p.text) {
                hits.push((p.n, m.start(), m.end() - m.start()));
            }
        }
    }

    // 3. Apply occurrence filter.
    if let Some(n) = input.occurrence {
        if n == 0 || (n as usize) > hits.len() {
            return Err(DocsError::TextNotFound {
                find: input.find,
                fuzzy_suggestions: vec![],
            });
        }
        hits = vec![hits[(n - 1) as usize]];
    }

    // 4. Match-count gates.
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

    // 5. dry_run short-circuits before any write.
    if input.dry_run {
        return Ok(EditResult {
            changes: hits.iter().map(|(n, off, len)| {
                let text = &lookup_para(snap, *n).text;
                let before: String = take_chars_in_byte_range(text, *off, *len);
                ChangeRecord {
                    kind: ChangeKind::Replace,
                    paragraph: *n,
                    before: Some(before),
                    after: Some(input.replace.clone()),
                    tab_id: lookup_tab_id(snap, *n),
                }
            }).collect(),
            revision_id_after: snap.revision_id.clone(),
            outline_snapshot: vec![],
            lossy_conversions: vec![],
            pending_human_changes_outside_scope: guard.soft_warnings.clone(),
        });
    }

    // 6. Emit batchUpdate requests (write-backwards by paragraph then offset).
    let mut requests: Vec<serde_json::Value> = Vec::new();
    let mut changes: Vec<ChangeRecord> = Vec::new();
    let mut hits_sorted = hits.clone();
    hits_sorted.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));

    for (n, off, len) in &hits_sorted {
        let p = lookup_para(snap, *n);
        // Convert byte offset within paragraph text to a UTF-16
        // segment-relative absolute index by adding p.start_index +
        // utf16 length of the text BEFORE the byte offset.
        let prefix_utf16 = utf16_len(&p.text[..*off]);
        let match_utf16 = utf16_len(&p.text[*off..*off + *len]);
        let start = p.start_index + prefix_utf16;
        let end = start + match_utf16;
        let before: String = p.text[*off..*off + *len].to_string();
        requests.push(serde_json::json!({
            "deleteContentRange": {"range": {"startIndex": start, "endIndex": end}}
        }));
        requests.push(serde_json::json!({
            "insertText": {"location": {"index": start}, "text": input.replace.clone()}
        }));
        changes.push(ChangeRecord {
            kind: ChangeKind::Replace,
            paragraph: *n,
            before: Some(before),
            after: Some(input.replace.clone()),
            tab_id: lookup_tab_id(snap, *n),
        });
    }

    // 7. POST the batchUpdate with required_revision = the snapshot we
    // saw. If Google rejects with conflict, the guard will catch it on
    // the next call.
    let result = ctx.client
        .batch_update(doc_id, requests, Some(&snap.revision_id))
        .await?;

    // 8. Refresh cache + revision state.
    ctx.cache.invalidate(ctx.session_id, doc_id);
    ctx.revisions.put(ctx.session_id, doc_id, &result.revision_id_after).await?;
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

fn lookup_para<'a>(s: &'a DocumentSnapshot, n: u32) -> &'a ParagraphSnapshot {
    s.tabs.iter().flat_map(|t| t.paragraphs.iter()).find(|p| p.n == n)
        .expect("para exists — caller filtered by scope")
}

fn lookup_tab_id(s: &DocumentSnapshot, n: u32) -> Option<TabId> {
    s.tabs.iter().find(|t| t.paragraphs.iter().any(|p| p.n == n))
        .and_then(|t| t.tab_id.clone())
}

fn build_previews(snap: &DocumentSnapshot, hits: &[(u32, usize, usize)]) -> Vec<MatchPreview> {
    hits.iter().enumerate().map(|(i, (n, off, _len))| {
        let p = lookup_para(snap, *n);
        // ~30 chars before the match, ~50 after.
        let byte_start = off.saturating_sub(30);
        let byte_end = (*off + 50).min(p.text.len());
        // Slice on char boundaries.
        let preview = take_slice_around(&p.text, byte_start, byte_end);
        MatchPreview { n: (i + 1) as u32, paragraph: *n, preview }
    }).collect()
}

fn build_outline(s: &DocumentSnapshot) -> Vec<OutlineEntry> {
    s.tabs.iter().flat_map(|t| t.paragraphs.iter().map(|p| OutlineEntry {
        paragraph: p.n,
        tab_id: t.tab_id.clone(),
        kind: p.kind,
        text_preview: p.text.chars().take(80).collect(),
    })).collect()
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

fn take_chars_in_byte_range(s: &str, byte_off: usize, byte_len: usize) -> String {
    s.get(byte_off..byte_off + byte_len).map(String::from).unwrap_or_default()
}

fn take_slice_around(s: &str, start: usize, end: usize) -> String {
    let mut start = start;
    let mut end = end.min(s.len());
    while start < s.len() && !s.is_char_boundary(start) { start += 1; }
    while end < s.len() && !s.is_char_boundary(end) { end += 1; }
    s.get(start..end).unwrap_or(s).chars().take(80).collect()
}
