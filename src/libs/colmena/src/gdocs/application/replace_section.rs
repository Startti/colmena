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
        "deleteContentRange": {
            "range": crate::gdocs::application::util::range(start, end, &rs.tab_id),
        }
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
    // CRITICAL: store the snapshot's revision_id (documents.get format)
    // — not batch_update's writeControl id — so the next guard's
    // revisionId comparison succeeds. See replace_text.rs for the full
    // explanation.
    ctx.cache.invalidate(ctx.session_id, doc_id);
    let fresh = ctx.client.get(doc_id).await?;
    ctx.revisions
        .put(ctx.session_id, doc_id, &fresh.revision_id)
        .await?;
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

    // Resolve EndOfSegment to a concrete index. The Docs API requires
    // insertText.location.index to be INSIDE an existing paragraph
    // (i.e. before its terminating newline). For a tab with content
    // that's `last_paragraph.end_index - 1`. For an empty doc (no
    // paragraphs at all), index 1 is the only legal position.
    let tab_paras: Vec<_> = snap
        .tabs
        .iter()
        .filter(|t| input.tab_id.is_none() || t.tab_id == input.tab_id)
        .flat_map(|t| t.paragraphs.iter())
        .collect();
    let insert_index = tab_paras
        .iter()
        .map(|p| p.end_index)
        .max()
        .map(|e| e.saturating_sub(1).max(1))
        .unwrap_or(1);

    let conv = markdown_to_requests(
        &input.new_markdown,
        InsertionPoint::Index {
            index: insert_index,
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
    // CRITICAL: store the snapshot's revision_id (documents.get format)
    // — not batch_update's writeControl id — so the next guard's
    // revisionId comparison succeeds. See replace_text.rs for the full
    // explanation.
    ctx.cache.invalidate(ctx.session_id, doc_id);
    let fresh = ctx.client.get(doc_id).await?;
    ctx.revisions
        .put(ctx.session_id, doc_id, &fresh.revision_id)
        .await?;
    ctx.cache.put(ctx.session_id, doc_id, fresh.clone());

    let last_para = snap.tabs.iter().flat_map(|t| t.paragraphs.iter()).count() as u32 + 1;

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

#[cfg(test)]
mod app_tests {
    use super::*;
    use crate::gdocs::application::_test_helpers::*;
    use crate::gdocs::application::co_edit_guard::GuardContext;
    use crate::gdocs::domain::{BatchUpdateResult, ParagraphKind, RevisionId};
    use std::sync::{Arc, Mutex};

    fn expect_get_sequence(
        client: &mut crate::gdocs::domain::traits::MockDocsClient,
        snaps: Vec<DocumentSnapshot>,
    ) {
        let queue = Arc::new(Mutex::new(snaps));
        client.expect_get().returning(move |_| {
            let mut q = queue.lock().unwrap();
            let s = if q.len() > 1 {
                q.remove(0)
            } else {
                q[0].clone()
            };
            Ok(s)
        });
    }

    fn make_batch_update_ok(
        client: &mut crate::gdocs::domain::traits::MockDocsClient,
        rev_after: &'static str,
    ) {
        client.expect_batch_update().returning(move |_, _, _| {
            Ok(BatchUpdateResult {
                revision_id_after: RevisionId(rev_after.into()),
                replies: vec![],
            })
        });
    }

    #[tokio::test]
    async fn replace_section_happy() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![
                (1, ParagraphKind::Heading2, "Resumen", 1, 9),
                (2, ParagraphKind::Paragraph, "contenido viejo", 9, 25),
                (3, ParagraphKind::Heading2, "Otro", 25, 30),
            ],
        );
        let s2 = snap(
            "r2",
            vec![
                (1, ParagraphKind::Heading2, "Resumen", 1, 9),
                (2, ParagraphKind::Paragraph, "contenido nuevo", 9, 25),
                (3, ParagraphKind::Heading2, "Otro", 25, 30),
            ],
        );
        expect_get_sequence(&mut rig.client, vec![s, s2]);
        make_batch_update_ok(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let result = super::run_replace_section(
            &ctx,
            &doc_id(),
            ReplaceSectionInput {
                heading: "## Resumen".into(),
                new_markdown: "contenido nuevo".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(result.changes.len(), 1);
    }

    #[tokio::test]
    async fn replace_section_heading_not_found() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![
                (1, ParagraphKind::Heading2, "Otro", 1, 6),
                (2, ParagraphKind::Paragraph, "txt", 6, 10),
            ],
        );
        expect_get_sequence(&mut rig.client, vec![s]);
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let err = super::run_replace_section(
            &ctx,
            &doc_id(),
            ReplaceSectionInput {
                heading: "## Resumen".into(),
                new_markdown: "x".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DocsError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn replace_section_rejects_table() {
        let rig = TestRig::new();
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let md = "| c1 | c2 |\n|----|----|\n| a  | b  |";
        let err = super::run_replace_section(
            &ctx,
            &doc_id(),
            ReplaceSectionInput {
                heading: "## R".into(),
                new_markdown: md.into(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DocsError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn append_markdown_happy() {
        let mut rig = TestRig::new();
        let s = snap("r1", vec![(1, ParagraphKind::Paragraph, "Original", 1, 10)]);
        let s2 = snap(
            "r2",
            vec![
                (1, ParagraphKind::Paragraph, "Original", 1, 10),
                (2, ParagraphKind::Paragraph, "Apéndice", 10, 19),
            ],
        );
        expect_get_sequence(&mut rig.client, vec![s, s2]);
        make_batch_update_ok(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let result = super::run_append_markdown(
            &ctx,
            &doc_id(),
            AppendMarkdownInput {
                new_markdown: "Apéndice".into(),
                tab_id: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, ChangeKind::Insert);
    }

    #[tokio::test]
    async fn append_markdown_rejects_table() {
        let rig = TestRig::new();
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let md = "| c1 | c2 |\n|----|----|\n| a  | b  |";
        let err = super::run_append_markdown(
            &ctx,
            &doc_id(),
            AppendMarkdownInput {
                new_markdown: md.into(),
                tab_id: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DocsError::InvalidArgs(_)));
    }
}
