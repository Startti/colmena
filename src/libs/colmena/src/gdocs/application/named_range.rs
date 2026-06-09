//! `create_named_range` and `replace_named_range` use cases.
//!
//! Named ranges give the agent stable, name-addressable spans that
//! survive across edits. `create_named_range` registers a span; the
//! v1 surface only accepts `Scope::Paragraph` (other scopes must be
//! resolved by the caller first). `replace_named_range` overwrites the
//! span's contents via `replaceNamedRangeContent`.

use crate::gdocs::application::co_edit_guard::{run_guard, GuardContext};
use crate::gdocs::domain::{
    ChangeKind, ChangeRecord, DocsError, DocumentId, DocumentSnapshot, EditResult,
    NamedRangeMeta, OutlineEntry, Scope,
};

#[derive(Debug, Clone)]
pub struct CreateNamedRangeInput {
    pub name: String,
    /// v1 supports only `Scope::Paragraph { n }`. Other scopes return
    /// `InvalidArgs` at the client layer.
    pub scope: Scope,
}

#[derive(Debug, Clone)]
pub struct ReplaceNamedRangeInput {
    pub name: String,
    pub new_text: String,
}

pub async fn run_create(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    input: CreateNamedRangeInput,
) -> Result<NamedRangeMeta, DocsError> {
    // Guard the read; the create call itself is non-overlapping.
    let _ = run_guard(ctx, doc_id, &Scope::All).await?;
    ctx.client
        .create_named_range(doc_id, &input.name, input.scope)
        .await
}

pub async fn run_replace(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    input: ReplaceNamedRangeInput,
) -> Result<EditResult, DocsError> {
    let guard = run_guard(ctx, doc_id, &Scope::All).await?;
    let snap = &guard.snapshot;
    let ranges = ctx.client.list_named_ranges(doc_id).await?;
    let nr = ranges
        .into_iter()
        .find(|nr| nr.name == input.name)
        .ok_or_else(|| {
            DocsError::InvalidArgs(format!("named range '{}' not found", input.name))
        })?;
    let requests = vec![serde_json::json!({
        "replaceNamedRangeContent": {
            "namedRangeId": nr.named_range_id,
            "text": input.new_text,
        }
    })];
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
            paragraph: nr.paragraph_start,
            before: None,
            after: Some(input.new_text.chars().take(120).collect()),
            tab_id: None,
        }],
        revision_id_after: result.revision_id_after,
        outline_snapshot: build_outline(&fresh),
        lossy_conversions: vec![],
        pending_human_changes_outside_scope: guard.soft_warnings,
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
