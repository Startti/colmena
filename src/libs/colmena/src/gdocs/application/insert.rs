//! `insert_after_text`, `insert_before_text`, `insert_between` use cases.
//!
//! All three are content-addressed: the agent specifies anchor text(s);
//! the dispatcher locates them in the snapshot, computes a UTF-16
//! insertion index, converts `new_markdown` via `markdown_to_docs_ops`,
//! and POSTs one batchUpdate.
//!
//! v1 limitation: markdown containing table syntax (`|...|...|`) is
//! rejected with `InvalidArgs`. The converter's table emission is not
//! API-shaped in v1; rewriting requires a snapshot round-trip we defer
//! to v1.1. See `markdown_to_docs_ops.rs` source comments for details.

use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::markdown_to_docs_ops::{
    markdown_to_requests, InsertionPoint,
};
use crate::gdocs::application::co_edit_guard::{run_guard, GuardContext};
use crate::gdocs::domain::{
    ChangeKind, ChangeRecord, DocsError, DocumentId, DocumentSnapshot, EditResult, OutlineEntry,
    ParagraphSnapshot, Scope, TabId,
};

#[derive(Debug, Clone)]
pub struct InsertAfterTextInput {
    pub anchor: String,
    pub new_markdown: String,
    pub occurrence: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct InsertBeforeTextInput {
    pub anchor: String,
    pub new_markdown: String,
    pub occurrence: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct InsertBetweenInput {
    /// Heading like `"## Section A"`. Required.
    pub after_heading: String,
    /// Heading like `"## Section B"`. `None` → EOF.
    pub before_heading: Option<String>,
    pub new_markdown: String,
}

/// Insert after the (occurrence-th) match of `anchor` in the doc.
pub async fn run_after_text(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    input: InsertAfterTextInput,
) -> Result<EditResult, DocsError> {
    reject_table_markdown(&input.new_markdown)?;
    let guard = run_guard(ctx, doc_id, &Scope::All).await?;
    let snap = &guard.snapshot;
    let (para_n, byte_off, byte_len, tab_id) =
        find_anchor(snap, &input.anchor, input.occurrence)?;
    let p = lookup_para(snap, para_n);
    // Insert AFTER the match: byte_off + byte_len within the paragraph.
    let after_byte = byte_off + byte_len;
    let prefix_utf16 = utf16_len(&p.text[..after_byte]);
    let index = p.start_index + prefix_utf16;

    let conversion = markdown_to_requests(
        &input.new_markdown,
        InsertionPoint::Index { index, tab_id: tab_id.clone() },
    );

    apply_and_finalize(
        ctx,
        doc_id,
        snap,
        conversion.requests,
        ChangeKind::Insert,
        para_n,
        &input.new_markdown,
        tab_id,
        conversion.lossy,
        guard.soft_warnings,
    )
    .await
}

/// Insert before the (occurrence-th) match of `anchor`.
pub async fn run_before_text(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    input: InsertBeforeTextInput,
) -> Result<EditResult, DocsError> {
    reject_table_markdown(&input.new_markdown)?;
    let guard = run_guard(ctx, doc_id, &Scope::All).await?;
    let snap = &guard.snapshot;
    let (para_n, byte_off, _byte_len, tab_id) =
        find_anchor(snap, &input.anchor, input.occurrence)?;
    let p = lookup_para(snap, para_n);
    let prefix_utf16 = utf16_len(&p.text[..byte_off]);
    let index = p.start_index + prefix_utf16;

    let conversion = markdown_to_requests(
        &input.new_markdown,
        InsertionPoint::Index { index, tab_id: tab_id.clone() },
    );

    apply_and_finalize(
        ctx,
        doc_id,
        snap,
        conversion.requests,
        ChangeKind::Insert,
        para_n,
        &input.new_markdown,
        tab_id,
        conversion.lossy,
        guard.soft_warnings,
    )
    .await
}

/// Insert between two headings. Uses scope_resolver's BetweenHeadings
/// for paragraph-range resolution; the insertion index is the
/// `start_index` of the first paragraph after the `after_heading`
/// (existing content gets pushed down).
pub async fn run_between(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    input: InsertBetweenInput,
) -> Result<EditResult, DocsError> {
    reject_table_markdown(&input.new_markdown)?;
    let scope = Scope::BetweenHeadings {
        after: input.after_heading.clone(),
        before: input.before_heading.clone(),
    };
    let guard = run_guard(ctx, doc_id, &scope).await?;
    let snap = &guard.snapshot;
    // Use the resolved scope's paragraph_start (which is the FIRST
    // paragraph AFTER the `after` heading). We insert at its
    // start_index, pushing existing content down.
    let n = guard.resolved_scope.paragraph_start;
    let p = lookup_para_or_eof(snap, n);
    let index = p.start_index;
    let tab_id = guard.resolved_scope.tab_id.clone();

    let conversion = markdown_to_requests(
        &input.new_markdown,
        InsertionPoint::Index { index, tab_id: tab_id.clone() },
    );

    apply_and_finalize(
        ctx,
        doc_id,
        snap,
        conversion.requests,
        ChangeKind::Insert,
        n,
        &input.new_markdown,
        tab_id,
        conversion.lossy,
        guard.soft_warnings,
    )
    .await
}

// ── Helpers ────────────────────────────────────────────────────────

/// Pre-scan markdown for table syntax; reject with a clear v1 message.
pub(crate) fn reject_table_markdown(md: &str) -> Result<(), DocsError> {
    // A CommonMark table requires a separator line of the form
    // `|---|---|` (with optional colons for alignment). The simplest
    // reliable check is matching that separator line — bash pipes and
    // text containing literal `|` won't trip this.
    let re = regex::Regex::new(r"(?m)^\s*\|?\s*:?-{3,}:?(\s*\|\s*:?-{3,}:?)+\s*\|?\s*$")
        .expect("static regex");
    if re.is_match(md) {
        return Err(DocsError::InvalidArgs(
            "v1 limitation: markdown tables are not supported in insert/replace use cases. \
             Use the dedicated `gdocs_insert_table` tool in v1.1, or emit the table content \
             as a regular paragraph list for now."
                .to_string(),
        ));
    }
    Ok(())
}

/// Locate the (occurrence-th) match of `anchor` in the doc. Returns
/// `(paragraph_n, byte_off_in_paragraph, byte_len, tab_id)`.
fn find_anchor(
    snap: &DocumentSnapshot,
    anchor: &str,
    occurrence: Option<u32>,
) -> Result<(u32, usize, usize, Option<TabId>), DocsError> {
    if anchor.is_empty() {
        return Err(DocsError::InvalidArgs(
            "anchor text must not be empty".to_string(),
        ));
    }
    let mut hits: Vec<(u32, usize, usize, Option<TabId>)> = Vec::new();
    for tab in &snap.tabs {
        for p in &tab.paragraphs {
            let mut start = 0;
            while let Some(rel) = p.text[start..].find(anchor) {
                let abs = start + rel;
                hits.push((p.n, abs, anchor.len(), tab.tab_id.clone()));
                start = abs + 1;
            }
        }
    }
    if hits.is_empty() {
        return Err(DocsError::TextNotFound {
            find: anchor.into(),
            fuzzy_suggestions: vec![],
        });
    }
    let n = occurrence.unwrap_or(1) as usize;
    if n == 0 || n > hits.len() {
        return Err(DocsError::TextNotFound {
            find: anchor.into(),
            fuzzy_suggestions: vec![],
        });
    }
    Ok(hits[n - 1].clone())
}

fn lookup_para(s: &DocumentSnapshot, n: u32) -> &ParagraphSnapshot {
    s.tabs
        .iter()
        .flat_map(|t| t.paragraphs.iter())
        .find(|p| p.n == n)
        .expect("paragraph exists")
}

/// Like `lookup_para`, but if `n` is past the last paragraph, returns
/// the last paragraph in the doc instead.
fn lookup_para_or_eof(s: &DocumentSnapshot, n: u32) -> &ParagraphSnapshot {
    s.tabs
        .iter()
        .flat_map(|t| t.paragraphs.iter())
        .find(|p| p.n == n)
        .or_else(|| s.tabs.iter().flat_map(|t| t.paragraphs.iter()).last())
        .expect("at least one paragraph exists")
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

#[allow(clippy::too_many_arguments)]
async fn apply_and_finalize(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    snap: &DocumentSnapshot,
    requests: Vec<serde_json::Value>,
    kind: ChangeKind,
    paragraph: u32,
    new_markdown: &str,
    tab_id: Option<TabId>,
    lossy: Vec<crate::gdocs::domain::LossyConversion>,
    soft_warnings: Vec<crate::gdocs::domain::HumanChange>,
) -> Result<EditResult, DocsError> {
    if requests.is_empty() {
        return Err(DocsError::InvalidArgs(
            "markdown produced no batchUpdate requests (all elements were lossy)".into(),
        ));
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
        changes: vec![ChangeRecord {
            kind,
            paragraph,
            before: None,
            after: Some(new_markdown.chars().take(120).collect()),
            tab_id,
        }],
        revision_id_after: result.revision_id_after,
        outline_snapshot: build_outline(&fresh),
        lossy_conversions: lossy,
        pending_human_changes_outside_scope: soft_warnings,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_markdown_with_table() {
        let md = "Some text\n\n| col1 | col2 |\n|------|------|\n| a    | b    |\n";
        let err = reject_table_markdown(md).unwrap_err();
        assert!(matches!(err, DocsError::InvalidArgs(_)));
    }

    #[test]
    fn accepts_markdown_without_table() {
        let md = "# Heading\n\nA paragraph.\n\n- one\n- two\n";
        assert!(reject_table_markdown(md).is_ok());
    }

    #[test]
    fn accepts_markdown_with_literal_pipes() {
        // Pipes outside tables should NOT trigger rejection.
        let md = "Use the bash pipe operator: `ls | grep foo`\n";
        assert!(reject_table_markdown(md).is_ok());
    }
}
