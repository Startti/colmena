//! `create_named_range` and `replace_named_range` use cases.
//!
//! Named ranges give the agent stable, name-addressable spans that
//! survive across edits. `create_named_range` registers a span; the
//! v1 surface only accepts `Scope::Paragraph` (other scopes must be
//! resolved by the caller first). `replace_named_range` overwrites the
//! span's contents via `replaceNamedRangeContent`.

use crate::gdocs::application::co_edit_guard::{run_guard, GuardContext};
use crate::gdocs::domain::{
    ChangeKind, ChangeRecord, DocsError, DocumentId, DocumentSnapshot, EditResult, NamedRangeMeta,
    OutlineEntry, Scope,
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
        .ok_or_else(|| DocsError::InvalidArgs(format!("named range '{}' not found", input.name)))?;
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
    // CRITICAL: store the snapshot's revision_id (documents.get format)
    // — not batch_update's writeControl id — so the next guard's
    // revisionId comparison succeeds. See replace_text.rs for the full
    // explanation.
    ctx.cache.invalidate(ctx.session_id, doc_id);
    let fresh = ctx.client.get(doc_id).await?;
    ctx.revisions
        .put_with_snapshot(ctx.session_id, doc_id, &fresh.revision_id, Some(&fresh))
        .await?;
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
    async fn create_named_range_paragraph_scope_happy() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "Texto del párrafo", 1, 20)],
        );
        expect_get_sequence(&mut rig.client, vec![s]);
        rig.client
            .expect_create_named_range()
            .returning(|_, name, _| {
                Ok(NamedRangeMeta {
                    named_range_id: format!("nr_{}", name),
                    name: name.into(),
                    paragraph_start: 1,
                    paragraph_end: 1,
                })
            });
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let nr = super::run_create(
            &ctx,
            &doc_id(),
            CreateNamedRangeInput {
                name: "intro".into(),
                scope: Scope::Paragraph { n: 1 },
            },
        )
        .await
        .unwrap();
        assert_eq!(nr.name, "intro");
        assert_eq!(nr.paragraph_start, 1);
    }

    #[tokio::test]
    async fn create_named_range_passes_arbitrary_scope_to_client() {
        // The application layer doesn't reject non-paragraph scopes
        // itself — that's the client/dispatcher responsibility. This
        // test pins the current behavior: whatever scope is passed in
        // is forwarded to the client.
        let mut rig = TestRig::new();
        let s = snap("r1", vec![(1, ParagraphKind::Paragraph, "x", 1, 3)]);
        expect_get_sequence(&mut rig.client, vec![s]);
        let captured_scope: Arc<Mutex<Option<Scope>>> = Arc::new(Mutex::new(None));
        let cs = captured_scope.clone();
        rig.client
            .expect_create_named_range()
            .returning(move |_, name, scope| {
                *cs.lock().unwrap() = Some(scope.clone());
                Ok(NamedRangeMeta {
                    named_range_id: format!("nr_{}", name),
                    name: name.into(),
                    paragraph_start: 1,
                    paragraph_end: 1,
                })
            });
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        super::run_create(
            &ctx,
            &doc_id(),
            CreateNamedRangeInput {
                name: "all".into(),
                scope: Scope::All,
            },
        )
        .await
        .unwrap();
        let scope = captured_scope.lock().unwrap();
        assert!(matches!(*scope, Some(Scope::All)));
    }

    #[tokio::test]
    async fn replace_named_range_happy() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "Texto viejo", 1, 13)],
        );
        let s2 = snap(
            "r2",
            vec![(1, ParagraphKind::Paragraph, "Texto nuevo", 1, 13)],
        );
        expect_get_sequence(&mut rig.client, vec![s, s2]);
        rig.client.expect_list_named_ranges().returning(|_| {
            Ok(vec![NamedRangeMeta {
                named_range_id: "nr_intro".into(),
                name: "intro".into(),
                paragraph_start: 1,
                paragraph_end: 1,
            }])
        });
        make_batch_update_ok(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let result = super::run_replace(
            &ctx,
            &doc_id(),
            ReplaceNamedRangeInput {
                name: "intro".into(),
                new_text: "Texto nuevo".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, ChangeKind::Replace);
    }

    #[tokio::test]
    async fn replace_named_range_not_found() {
        let mut rig = TestRig::new();
        let s = snap("r1", vec![(1, ParagraphKind::Paragraph, "x", 1, 3)]);
        expect_get_sequence(&mut rig.client, vec![s]);
        rig.client
            .expect_list_named_ranges()
            .returning(|_| Ok(vec![]));
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let err = super::run_replace(
            &ctx,
            &doc_id(),
            ReplaceNamedRangeInput {
                name: "missing".into(),
                new_text: "x".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DocsError::InvalidArgs(_)));
    }
}
