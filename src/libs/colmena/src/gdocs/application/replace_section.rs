//! `replace_section` and `append_markdown` use cases.
//!
//! `replace_section` finds a heading and replaces everything until the
//! next same-or-higher heading (or EOF) with new markdown.
//! `append_markdown` appends new markdown at end-of-segment.
//!
//! Both pre-check for table markdown via `reject_table_markdown` (v1
//! limitation shared with `insert.rs`).

use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::markdown_to_docs_ops::{
    markdown_to_requests, InsertionPoint,
};
use crate::gdocs::application::co_edit_guard::{run_guard, GuardContext};
use crate::gdocs::application::insert::reject_table_markdown;
use crate::gdocs::domain::{
    ChangeKind, ChangeRecord, DocsError, DocumentId, DocumentSnapshot, EditResult, OutlineEntry,
    ParagraphSnapshot, Scope, TabId,
};

#[derive(Debug, Clone)]
pub struct ReplaceSectionInput {
    /// Heading that marks the section start, e.g. `"## Resumen"`.
    pub heading: String,
    pub new_markdown: String,
}

#[derive(Debug, Clone)]
pub struct AppendMarkdownInput {
    pub new_markdown: String,
    pub tab_id: Option<TabId>,
}

/// Replace everything between `heading` and the next same-or-higher
/// heading (or EOF) with the new markdown.
pub async fn run_replace_section(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    input: ReplaceSectionInput,
) -> Result<EditResult, DocsError> {
    reject_table_markdown(&input.new_markdown)?;
    let scope = Scope::UnderHeading {
        heading: input.heading.clone(),
    };
    let guard = run_guard(ctx, doc_id, &scope).await?;
    let snap = &guard.snapshot;
    let rs = &guard.resolved_scope;

    // Section is paragraphs [paragraph_start .. paragraph_end] inclusive.
    let first = lookup_para_opt(snap, rs.paragraph_start);
    let last = lookup_para_opt(snap, rs.paragraph_end);
    let (Some(first), Some(last)) = (first, last) else {
        // Empty section — fall back to append at end-of-segment.
        return run_append_markdown(
            ctx,
            doc_id,
            AppendMarkdownInput {
                new_markdown: input.new_markdown,
                tab_id: rs.tab_id.clone(),
            },
        )
        .await;
    };
    let start = first.start_index;
    let end = last.end_index;

    let mut requests = vec![serde_json::json!({
        "deleteContentRange": {"range": {"startIndex": start, "endIndex": end}}
    })];
    let conv = markdown_to_requests(
        &input.new_markdown,
        InsertionPoint::Index {
            index: start,
            tab_id: rs.tab_id.clone(),
        },
    );
    requests.extend(conv.requests);

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
            kind: ChangeKind::Replace,
            paragraph: rs.paragraph_start,
            before: None,
            after: Some(input.new_markdown.chars().take(120).collect()),
            tab_id: rs.tab_id.clone(),
        }],
        revision_id_after: result.revision_id_after,
        outline_snapshot: build_outline(&fresh),
        lossy_conversions: conv.lossy,
        pending_human_changes_outside_scope: guard.soft_warnings,
    })
}

/// Append markdown at end-of-segment (optionally scoped to a tab).
pub async fn run_append_markdown(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    input: AppendMarkdownInput,
) -> Result<EditResult, DocsError> {
    reject_table_markdown(&input.new_markdown)?;
    let guard = run_guard(ctx, doc_id, &Scope::All).await?;
    let snap = &guard.snapshot;

    let conv = markdown_to_requests(
        &input.new_markdown,
        InsertionPoint::EndOfSegment {
            tab_id: input.tab_id.clone(),
        },
    );
    if conv.requests.is_empty() {
        return Err(DocsError::InvalidArgs(
            "markdown produced no batchUpdate requests".into(),
        ));
    }

    let result = ctx
        .client
        .batch_update(doc_id, conv.requests, Some(&snap.revision_id))
        .await?;
    ctx.cache.invalidate(ctx.session_id, doc_id);
    ctx.revisions
        .put(ctx.session_id, doc_id, &result.revision_id_after)
        .await?;
    let fresh = ctx.client.get(doc_id).await?;
    ctx.cache.put(ctx.session_id, doc_id, fresh.clone());

    let last_para = snap
        .tabs
        .iter()
        .flat_map(|t| t.paragraphs.iter())
        .count() as u32
        + 1;

    Ok(EditResult {
        changes: vec![ChangeRecord {
            kind: ChangeKind::Insert,
            paragraph: last_para,
            before: None,
            after: Some(input.new_markdown.chars().take(120).collect()),
            tab_id: input.tab_id,
        }],
        revision_id_after: result.revision_id_after,
        outline_snapshot: build_outline(&fresh),
        lossy_conversions: conv.lossy,
        pending_human_changes_outside_scope: guard.soft_warnings,
    })
}

// ── Helpers ────────────────────────────────────────────────────────

fn lookup_para_opt(s: &DocumentSnapshot, n: u32) -> Option<&ParagraphSnapshot> {
    s.tabs
        .iter()
        .flat_map(|t| t.paragraphs.iter())
        .find(|p| p.n == n)
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
