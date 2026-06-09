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
    ChangeKind, ChangeRecord, DocsError, DocumentId, DocumentSnapshot, EditResult, MatchPreview,
    OutlineEntry, ParagraphSnapshot, Scope, TabId,
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
            if tab.tab_id.as_ref() != Some(scope_tab) {
                continue;
            }
        }
        for p in &tab.paragraphs {
            if p.n < scope.paragraph_start || p.n > scope.paragraph_end {
                continue;
            }
            // Anchor filter — both `find` AND `anchor` must appear in
            // the same paragraph text.
            if let Some(a) = &input.anchor {
                if !p.text.contains(a) {
                    continue;
                }
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
            changes: hits
                .iter()
                .map(|(n, off, len)| {
                    let text = &lookup_para(snap, *n).text;
                    let before: String = take_chars_in_byte_range(text, *off, *len);
                    ChangeRecord {
                        kind: ChangeKind::Replace,
                        paragraph: *n,
                        before: Some(before),
                        after: Some(input.replace.clone()),
                        tab_id: lookup_tab_id(snap, *n),
                    }
                })
                .collect(),
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
    hits_sorted.sort_by_key(|b| std::cmp::Reverse((b.0, b.1)));

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
        // Critical: range/location indices are SEGMENT-RELATIVE.
        // Include `tabId` whenever the paragraph lives in a non-default
        // tab — otherwise Google interprets the index against tab 1.
        let para_tab = lookup_tab_id(snap, *n);
        requests.push(serde_json::json!({
            "deleteContentRange": {
                "range": crate::gdocs::application::util::range(start, end, &para_tab),
            }
        }));
        requests.push(serde_json::json!({
            "insertText": {
                "location": crate::gdocs::application::util::location(start, &para_tab),
                "text": input.replace.clone(),
            }
        }));
        changes.push(ChangeRecord {
            kind: ChangeKind::Replace,
            paragraph: *n,
            before: Some(before),
            after: Some(input.replace.clone()),
            tab_id: para_tab,
        });
    }

    // 7. POST the batchUpdate with required_revision = the snapshot we
    // saw. If Google rejects with conflict, the guard will catch it on
    // the next call.
    let result = ctx
        .client
        .batch_update(doc_id, requests, Some(&snap.revision_id))
        .await?;

    // 8. Refresh cache + revision state.
    //
    // CRITICAL: Use the snapshot's revision_id (from documents.get) for
    // the stored cursor, NOT batch_update's writeControl id. They have
    // different formats — documents.get returns a long opaque token
    // (~111 chars), writeControl returns a short one (~25 chars).
    // Storing the short id makes the next guard's revisionId comparison
    // fail spuriously (long != short), firing false HumanChangesPending
    // after every write. The guard always reads via documents.get, so
    // store what THAT endpoint will return next.
    ctx.cache.invalidate(ctx.session_id, doc_id);
    let fresh = ctx.client.get(doc_id).await?;
    ctx.revisions
        .put(ctx.session_id, doc_id, &fresh.revision_id)
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
        .expect("para exists — caller filtered by scope")
}

fn lookup_tab_id(s: &DocumentSnapshot, n: u32) -> Option<TabId> {
    s.tabs
        .iter()
        .find(|t| t.paragraphs.iter().any(|p| p.n == n))
        .and_then(|t| t.tab_id.clone())
}

fn build_previews(snap: &DocumentSnapshot, hits: &[(u32, usize, usize)]) -> Vec<MatchPreview> {
    hits.iter()
        .enumerate()
        .map(|(i, (n, off, _len))| {
            let p = lookup_para(snap, *n);
            // ~30 chars before the match, ~50 after.
            let byte_start = off.saturating_sub(30);
            let byte_end = (*off + 50).min(p.text.len());
            // Slice on char boundaries.
            let preview = take_slice_around(&p.text, byte_start, byte_end);
            MatchPreview {
                n: (i + 1) as u32,
                paragraph: *n,
                preview,
            }
        })
        .collect()
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

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

fn take_chars_in_byte_range(s: &str, byte_off: usize, byte_len: usize) -> String {
    s.get(byte_off..byte_off + byte_len)
        .map(String::from)
        .unwrap_or_default()
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

#[cfg(test)]
mod app_tests {
    use super::*;
    use crate::gdocs::application::_test_helpers::*;
    use crate::gdocs::application::co_edit_guard::GuardContext;
    use crate::gdocs::domain::{BatchUpdateResult, ParagraphKind, RevisionId};
    use std::sync::Mutex;

    /// Have `expect_get()` return successive snapshots from a queue.
    /// When the queue is empty the LAST element is reused.
    fn expect_get_sequence(
        client: &mut crate::gdocs::domain::traits::MockDocsClient,
        snaps: Vec<DocumentSnapshot>,
    ) {
        let queue = std::sync::Arc::new(Mutex::new(snaps));
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

    fn default_input() -> ReplaceTextInput {
        ReplaceTextInput {
            find: "cliente".into(),
            replace: "Acme".into(),
            scope: Scope::All,
            case_sensitive: false,
            whole_word: false,
            dry_run: false,
            confirm_many: false,
            occurrence: None,
            anchor: None,
        }
    }

    #[tokio::test]
    async fn replace_text_happy_single_match() {
        let mut rig = TestRig::new();
        let snap0 = snap(
            "r1",
            vec![(
                1,
                ParagraphKind::Paragraph,
                "Hola cliente bienvenido",
                1,
                25,
            )],
        );
        let snap1 = snap(
            "r2",
            vec![(1, ParagraphKind::Paragraph, "Hola Acme bienvenido", 1, 22)],
        );
        expect_get_sequence(&mut rig.client, vec![snap0, snap1]);
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
        assert_eq!(result.revision_id_after.0, "r2");
        assert_eq!(result.changes[0].kind, ChangeKind::Replace);
    }

    #[tokio::test]
    async fn replace_text_text_not_found() {
        let mut rig = TestRig::new();
        let snap0 = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "Texto sin coincidencia", 1, 22)],
        );
        expect_get_sequence(&mut rig.client, vec![snap0]);
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
    async fn replace_text_confirm_many_threshold() {
        let mut rig = TestRig::new();
        // 5 occurrences of "x" — should trigger ConfirmManyMatches.
        let text = "x x x x x".to_string();
        let snap0 = snap("r1", vec![(1, ParagraphKind::Paragraph, &text, 1, 12)]);
        expect_get_sequence(&mut rig.client, vec![snap0]);
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = ReplaceTextInput {
            find: "x".into(),
            ..default_input()
        };
        let err = super::run(&ctx, &doc_id(), input).await.unwrap_err();
        match err {
            DocsError::ConfirmManyMatches { count, .. } => assert_eq!(count, 5),
            e => panic!("wrong error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn replace_text_confirm_many_flag_allows_write() {
        let mut rig = TestRig::new();
        let text = "x x x x x".to_string();
        let snap0 = snap("r1", vec![(1, ParagraphKind::Paragraph, &text, 1, 12)]);
        let snap1 = snap(
            "r2",
            vec![(1, ParagraphKind::Paragraph, "y y y y y", 1, 12)],
        );
        expect_get_sequence(&mut rig.client, vec![snap0, snap1]);
        make_batch_update_ok(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = ReplaceTextInput {
            find: "x".into(),
            replace: "y".into(),
            confirm_many: true,
            ..default_input()
        };
        let result = super::run(&ctx, &doc_id(), input).await.unwrap();
        assert_eq!(result.changes.len(), 5);
    }

    #[tokio::test]
    async fn replace_text_dry_run_skips_batch_update() {
        let mut rig = TestRig::new();
        let snap0 = snap(
            "r1",
            vec![(
                1,
                ParagraphKind::Paragraph,
                "Hola cliente bienvenido",
                1,
                25,
            )],
        );
        expect_get_sequence(&mut rig.client, vec![snap0]);
        // Note: no expect_batch_update — would panic if called.
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = ReplaceTextInput {
            dry_run: true,
            ..default_input()
        };
        let result = super::run(&ctx, &doc_id(), input).await.unwrap();
        assert_eq!(result.changes.len(), 1);
        // dry_run returns snap revision_id unchanged.
        assert_eq!(result.revision_id_after.0, "r1");
    }

    #[tokio::test]
    async fn replace_text_whole_word_filters_substring() {
        let mut rig = TestRig::new();
        let snap0 = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "clientela y clientes", 1, 22)],
        );
        expect_get_sequence(&mut rig.client, vec![snap0]);
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        // whole_word=true with "cliente" — "clientela" and "clientes" should NOT match.
        let input = ReplaceTextInput {
            whole_word: true,
            ..default_input()
        };
        let err = super::run(&ctx, &doc_id(), input).await.unwrap_err();
        assert!(matches!(err, DocsError::TextNotFound { .. }));
    }

    #[tokio::test]
    async fn replace_text_case_sensitive_filters_mismatched_case() {
        let mut rig = TestRig::new();
        let snap0 = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "Cliente y CLIENTE", 1, 18)],
        );
        expect_get_sequence(&mut rig.client, vec![snap0]);
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = ReplaceTextInput {
            case_sensitive: true,
            ..default_input()
        };
        let err = super::run(&ctx, &doc_id(), input).await.unwrap_err();
        assert!(matches!(err, DocsError::TextNotFound { .. }));
    }

    #[tokio::test]
    async fn replace_text_scope_paragraph_limits_search() {
        let mut rig = TestRig::new();
        let snap0 = snap(
            "r1",
            vec![
                (1, ParagraphKind::Paragraph, "cliente uno", 1, 14),
                (2, ParagraphKind::Paragraph, "cliente dos", 14, 27),
            ],
        );
        let snap1 = snap(
            "r2",
            vec![
                (1, ParagraphKind::Paragraph, "Acme uno", 1, 11),
                (2, ParagraphKind::Paragraph, "cliente dos", 11, 24),
            ],
        );
        expect_get_sequence(&mut rig.client, vec![snap0, snap1]);
        make_batch_update_ok(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = ReplaceTextInput {
            scope: Scope::Paragraph { n: 1 },
            ..default_input()
        };
        let result = super::run(&ctx, &doc_id(), input).await.unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].paragraph, 1);
    }

    #[tokio::test]
    async fn replace_text_occurrence_selects_nth() {
        let mut rig = TestRig::new();
        let snap0 = snap(
            "r1",
            vec![
                (1, ParagraphKind::Paragraph, "cliente uno", 1, 14),
                (2, ParagraphKind::Paragraph, "cliente dos", 14, 27),
            ],
        );
        let snap1 = snap(
            "r2",
            vec![
                (1, ParagraphKind::Paragraph, "cliente uno", 1, 14),
                (2, ParagraphKind::Paragraph, "Acme dos", 14, 24),
            ],
        );
        expect_get_sequence(&mut rig.client, vec![snap0, snap1]);
        make_batch_update_ok(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = ReplaceTextInput {
            occurrence: Some(2),
            ..default_input()
        };
        let result = super::run(&ctx, &doc_id(), input).await.unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].paragraph, 2);
    }

    #[tokio::test]
    async fn replace_text_anchor_filters_to_matching_paragraph() {
        let mut rig = TestRig::new();
        let snap0 = snap(
            "r1",
            vec![
                (1, ParagraphKind::Paragraph, "cliente uno", 1, 14),
                (2, ParagraphKind::Paragraph, "cliente dos especial", 14, 35),
            ],
        );
        let snap1 = snap(
            "r2",
            vec![
                (1, ParagraphKind::Paragraph, "cliente uno", 1, 14),
                (2, ParagraphKind::Paragraph, "Acme dos especial", 14, 32),
            ],
        );
        expect_get_sequence(&mut rig.client, vec![snap0, snap1]);
        make_batch_update_ok(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = ReplaceTextInput {
            anchor: Some("especial".into()),
            ..default_input()
        };
        let result = super::run(&ctx, &doc_id(), input).await.unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].paragraph, 2);
    }

    #[tokio::test]
    async fn replace_text_occurrence_out_of_range_errors() {
        let mut rig = TestRig::new();
        let snap0 = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "cliente uno", 1, 14)],
        );
        expect_get_sequence(&mut rig.client, vec![snap0]);
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let input = ReplaceTextInput {
            occurrence: Some(99),
            ..default_input()
        };
        let err = super::run(&ctx, &doc_id(), input).await.unwrap_err();
        assert!(matches!(err, DocsError::TextNotFound { .. }));
    }
}
