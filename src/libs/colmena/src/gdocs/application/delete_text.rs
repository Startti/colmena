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
    hits_sorted.sort_by_key(|b| std::cmp::Reverse((b.0, b.1)));

    for (n, off, len) in &hits_sorted {
        let p = lookup_para(snap, *n);
        let prefix_utf16 = utf16_len(&p.text[..*off]);
        let match_utf16 = utf16_len(&p.text[*off..*off + *len]);
        let start = p.start_index + prefix_utf16;
        let end = start + match_utf16;
        let before: String = p.text[*off..*off + *len].to_string();
        let para_tab = lookup_tab_id(snap, *n);
        requests.push(serde_json::json!({
            "deleteContentRange": {
                "range": crate::gdocs::application::util::range(start, end, &para_tab),
            }
        }));
        changes.push(ChangeRecord {
            kind: ChangeKind::Delete,
            paragraph: *n,
            before: Some(before),
            after: None,
            tab_id: para_tab,
        });
    }

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

    fn default_input() -> DeleteTextInput {
        DeleteTextInput {
            find: "deleteme".into(),
            scope: Scope::All,
            case_sensitive: false,
            whole_word: false,
            occurrence: None,
            confirm_many: false,
        }
    }

    #[tokio::test]
    async fn delete_text_happy() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "Hola deleteme final", 1, 22)],
        );
        let s2 = snap(
            "r2",
            vec![(1, ParagraphKind::Paragraph, "Hola  final", 1, 13)],
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
        let result = super::run(&ctx, &doc_id(), default_input()).await.unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, ChangeKind::Delete);
    }

    #[tokio::test]
    async fn delete_text_not_found() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "Hola final", 1, 12)],
        );
        expect_get_sequence(&mut rig.client, vec![s]);
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let err = super::run(&ctx, &doc_id(), default_input())
            .await
            .unwrap_err();
        assert!(matches!(err, DocsError::TextNotFound { .. }));
    }

    #[tokio::test]
    async fn delete_text_confirm_many_threshold() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "x x x x x", 1, 12)],
        );
        expect_get_sequence(&mut rig.client, vec![s]);
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = DeleteTextInput {
            find: "x".into(),
            ..default_input()
        };
        let err = super::run(&ctx, &doc_id(), input).await.unwrap_err();
        match err {
            DocsError::ConfirmManyMatches { count, .. } => assert_eq!(count, 5),
            e => panic!("wrong: {:?}", e),
        }
    }

    #[tokio::test]
    async fn delete_text_multi_match_deletes_all() {
        let mut rig = TestRig::new();
        // 3 matches → below 5-threshold, no confirm_many needed.
        let s = snap(
            "r1",
            vec![(
                1,
                ParagraphKind::Paragraph,
                "deleteme y deleteme y deleteme",
                1,
                33,
            )],
        );
        let s2 = snap("r2", vec![(1, ParagraphKind::Paragraph, " y  y ", 1, 9)]);
        expect_get_sequence(&mut rig.client, vec![s, s2]);
        make_batch_update_ok(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let result = super::run(&ctx, &doc_id(), default_input()).await.unwrap();
        assert_eq!(result.changes.len(), 3);
        assert!(result.changes.iter().all(|c| c.kind == ChangeKind::Delete));
    }

    #[tokio::test]
    async fn delete_text_whole_word_filters() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(
                1,
                ParagraphKind::Paragraph,
                "deletemepre deletemepost",
                1,
                26,
            )],
        );
        expect_get_sequence(&mut rig.client, vec![s]);
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = DeleteTextInput {
            whole_word: true,
            ..default_input()
        };
        let err = super::run(&ctx, &doc_id(), input).await.unwrap_err();
        assert!(matches!(err, DocsError::TextNotFound { .. }));
    }
}
