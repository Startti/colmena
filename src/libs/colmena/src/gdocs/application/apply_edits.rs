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

    // Two-phase pipeline. PHASE A — resolve every sub-edit into one or more
    // `ResolvedEmit` records WITHOUT emitting any batchUpdate request yet.
    // Each record captures its anchor position in the original snapshot so
    // PHASE B can sort the union write-backwards across the WHOLE batch.
    //
    // Why two phases: Google's batchUpdate applies requests sequentially,
    // and each request sees the index changes from prior requests. The
    // safe pattern is to start from the END of the doc and walk backwards
    // — earlier indices stay valid because nothing upstream has moved.
    // The previous implementation sorted write-backwards per-sub-edit but
    // not globally, so sub-edit N+1's stale snapshot indices would land on
    // shifted positions in the post-N doc. That corrupted text in any
    // batch that combined multiple sub-edits.
    let mut resolved: Vec<ResolvedEmit> = Vec::new();
    let mut lossy = Vec::new();

    for op in input.edits.into_iter() {
        match op {
            ApplyEditOp::ReplaceText {
                find,
                replace,
                scope,
            } => {
                let hits = find_hits(snap, &find, &scope)?;
                for (n, off, len) in hits {
                    let p = lookup_para(snap, n);
                    let s = p.start_index + utf16_len(&p.text[..off]);
                    let e = s + utf16_len(&p.text[off..off + len]);
                    let para_tab = lookup_tab_id(snap, n);
                    let before_text = p.text[off..off + len].to_string();
                    resolved.push(ResolvedEmit {
                        paragraph: n,
                        byte_off: off,
                        byte_len: len,
                        requests: vec![
                            serde_json::json!({
                                "deleteContentRange": {
                                    "range": crate::gdocs::application::util::range(s, e, &para_tab),
                                }
                            }),
                            serde_json::json!({
                                "insertText": {
                                    "location": crate::gdocs::application::util::location(s, &para_tab),
                                    "text": replace.clone(),
                                }
                            }),
                        ],
                        change: ChangeRecord {
                            kind: ChangeKind::Replace,
                            paragraph: n,
                            before: Some(before_text),
                            after: Some(replace.clone()),
                            tab_id: para_tab,
                        },
                    });
                }
            }
            ApplyEditOp::DeleteText { find, scope } => {
                let hits = find_hits(snap, &find, &scope)?;
                for (n, off, len) in hits {
                    let p = lookup_para(snap, n);
                    let s = p.start_index + utf16_len(&p.text[..off]);
                    let e = s + utf16_len(&p.text[off..off + len]);
                    let para_tab = lookup_tab_id(snap, n);
                    let before_text = p.text[off..off + len].to_string();
                    resolved.push(ResolvedEmit {
                        paragraph: n,
                        byte_off: off,
                        byte_len: len,
                        requests: vec![serde_json::json!({
                            "deleteContentRange": {
                                "range": crate::gdocs::application::util::range(s, e, &para_tab),
                            }
                        })],
                        change: ChangeRecord {
                            kind: ChangeKind::Delete,
                            paragraph: n,
                            before: Some(before_text),
                            after: None,
                            tab_id: para_tab,
                        },
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
                resolved.push(ResolvedEmit {
                    paragraph: n,
                    byte_off: after_byte,
                    byte_len: 0,
                    requests: conv.requests,
                    change: ChangeRecord {
                        kind: ChangeKind::Insert,
                        paragraph: n,
                        before: None,
                        after: Some(new_markdown.chars().take(120).collect()),
                        tab_id,
                    },
                });
            }
        }
    }

    // PHASE B step 1 — detect overlapping byte ranges within the SAME
    // paragraph. Two replace/delete ranges that overlap cannot be safely
    // interleaved; an insert that lands inside another op's range is also
    // ambiguous. Fail fast with a structured error so the LLM can re-plan.
    check_no_overlaps_within_paragraph(&resolved)?;

    // PHASE B step 2 — global write-backwards sort. Within a paragraph,
    // higher byte_off goes first; across paragraphs, higher paragraph
    // number goes first. With this order each request operates on a part
    // of the doc that hasn't been touched yet, so the snapshot-derived
    // indices remain valid.
    resolved.sort_by_key(|r| std::cmp::Reverse((r.paragraph, r.byte_off)));

    // PHASE B step 3 — flatten into the final request + change vectors.
    // Within a single ResolvedEmit (e.g. a markdown insert that produced
    // many requests) we preserve the original order, since Google
    // interprets each request's location against the index AFTER prior
    // requests in the same batch.
    let mut all_requests: Vec<serde_json::Value> = Vec::new();
    let mut all_changes: Vec<ChangeRecord> = Vec::new();
    for r in resolved {
        all_requests.extend(r.requests);
        all_changes.push(r.change);
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
        changes: all_changes,
        revision_id_after: result.revision_id_after,
        outline_snapshot: build_outline(&fresh),
        lossy_conversions: lossy,
        pending_human_changes_outside_scope: guard.soft_warnings,
    })
}

// ── Helpers ────────────────────────────────────────────────────────

/// One atomic write that will land in the batchUpdate. Carries enough
/// snapshot-relative context to sort the whole batch write-backwards.
///
/// `byte_off` is the start of the touched region inside the paragraph
/// (for InsertAfterText: the byte offset right after the anchor).
/// `byte_len` is 0 for inserts and the matched length for
/// replace/delete. Together they describe the half-open `[byte_off,
/// byte_off + byte_len)` interval used by overlap detection.
///
/// `requests` may contain ONE element (delete-only) or MANY (markdown
/// insert that produced several insertText + paragraphStyle requests).
/// Their internal order is preserved because Google evaluates each
/// request's index against the state AFTER prior requests in the batch.
struct ResolvedEmit {
    paragraph: u32,
    byte_off: usize,
    byte_len: usize,
    requests: Vec<serde_json::Value>,
    change: ChangeRecord,
}

/// Scope-aware substring search shared by `ReplaceText` and `DeleteText`.
/// Empty results raise `TextNotFound` rather than silently succeeding so
/// the LLM gets a clear signal that the find string was wrong.
fn find_hits(
    snap: &DocumentSnapshot,
    find: &str,
    scope: &Scope,
) -> Result<Vec<(u32, usize, usize)>, DocsError> {
    let rs = crate::gdocs::application::scope_resolver::resolve(scope, snap)?;
    let re = RegexBuilder::new(&regex::escape(find))
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
            find: find.to_string(),
            fuzzy_suggestions: vec![],
        });
    }
    Ok(hits)
}

/// Reject any pair of `ResolvedEmit`s on the same paragraph whose touched
/// byte intervals intersect. Replace+delete overlap means the second op's
/// snapshot offset would not survive applying the first. Insert (zero-
/// length) sitting strictly inside another op's range is also rejected
/// because the insertion point evaporates after the surrounding range
/// gets deleted. Adjacent ops (touch at the boundary) are allowed.
fn check_no_overlaps_within_paragraph(emits: &[ResolvedEmit]) -> Result<(), DocsError> {
    use std::collections::HashMap;
    let mut by_para: HashMap<u32, Vec<&ResolvedEmit>> = HashMap::new();
    for e in emits {
        by_para.entry(e.paragraph).or_default().push(e);
    }
    for (para, ops) in &by_para {
        for i in 0..ops.len() {
            for j in (i + 1)..ops.len() {
                let a = ops[i];
                let b = ops[j];
                let a_end = a.byte_off + a.byte_len;
                let b_end = b.byte_off + b.byte_len;
                // Both zero-length inserts at the exact same point are
                // ambiguous (which one runs first?). Reject.
                if a.byte_len == 0 && b.byte_len == 0 && a.byte_off == b.byte_off {
                    return Err(DocsError::InvalidArgs(format!(
                        "apply_edits: two inserts share the same anchor in paragraph {} \
                         (byte offset {}). Resolve by combining them into a single \
                         InsertAfterText or moving one to a different anchor.",
                        para, a.byte_off
                    )));
                }
                // Half-open intervals overlap unless one ends at or before
                // the other starts.
                let disjoint = a_end <= b.byte_off || b_end <= a.byte_off;
                if !disjoint {
                    return Err(DocsError::InvalidArgs(format!(
                        "apply_edits: overlapping edits on paragraph {} — \
                         range {}..{} intersects {}..{}. Two replace/delete \
                         ranges that overlap cannot be reordered safely; \
                         split them into separate apply_edits calls or pick \
                         non-overlapping find strings.",
                        para, a.byte_off, a_end, b.byte_off, b_end
                    )));
                }
            }
        }
    }
    Ok(())
}

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

    fn make_batch_update_capture(
        client: &mut crate::gdocs::domain::traits::MockDocsClient,
        rev_after: &'static str,
    ) -> Arc<Mutex<Vec<serde_json::Value>>> {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let c = captured.clone();
        client.expect_batch_update().returning(move |_, reqs, _| {
            *c.lock().unwrap() = reqs.clone();
            Ok(BatchUpdateResult {
                revision_id_after: RevisionId(rev_after.into()),
                replies: vec![],
            })
        });
        captured
    }

    #[tokio::test]
    async fn apply_edits_empty_errors() {
        let rig = TestRig::new();
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let err = super::run(&ctx, &doc_id(), ApplyEditsInput { edits: vec![] })
            .await
            .unwrap_err();
        assert!(matches!(err, DocsError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn apply_edits_single_replace() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "Hola cliente", 1, 14)],
        );
        let s2 = snap(
            "r2",
            vec![(1, ParagraphKind::Paragraph, "Hola Acme", 1, 11)],
        );
        expect_get_sequence(&mut rig.client, vec![s, s2]);
        make_batch_update_capture(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let result = super::run(
            &ctx,
            &doc_id(),
            ApplyEditsInput {
                edits: vec![ApplyEditOp::ReplaceText {
                    find: "cliente".into(),
                    replace: "Acme".into(),
                    scope: Scope::All,
                }],
            },
        )
        .await
        .unwrap();
        assert_eq!(result.changes.len(), 1);
    }

    #[tokio::test]
    async fn apply_edits_compound_3_ops_concatenates_changes() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(
                1,
                ParagraphKind::Paragraph,
                "anchor uno cliente delete",
                1,
                28,
            )],
        );
        let s2 = snap(
            "r2",
            vec![(1, ParagraphKind::Paragraph, "anchor NUEVO uno Acme ", 1, 25)],
        );
        expect_get_sequence(&mut rig.client, vec![s, s2]);
        let captured = make_batch_update_capture(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let result = super::run(
            &ctx,
            &doc_id(),
            ApplyEditsInput {
                edits: vec![
                    ApplyEditOp::ReplaceText {
                        find: "cliente".into(),
                        replace: "Acme".into(),
                        scope: Scope::All,
                    },
                    ApplyEditOp::InsertAfterText {
                        anchor: "anchor".into(),
                        new_markdown: "NUEVO".into(),
                    },
                    ApplyEditOp::DeleteText {
                        find: "delete".into(),
                        scope: Scope::All,
                    },
                ],
            },
        )
        .await
        .unwrap();
        // 1 replace + 1 insert + 1 delete = 3 changes.
        assert_eq!(result.changes.len(), 3);
        // Single batchUpdate POST → captured contains many requests
        // (delete/insert pairs + markdown ops + delete range).
        let reqs = captured.lock().unwrap();
        assert!(reqs.len() >= 3);
    }

    #[tokio::test]
    async fn apply_edits_sub_edit_failure_aborts_compound() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "Hola cliente", 1, 14)],
        );
        expect_get_sequence(&mut rig.client, vec![s]);
        // No batch_update expected; the missing-find should abort first.
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let err = super::run(
            &ctx,
            &doc_id(),
            ApplyEditsInput {
                edits: vec![
                    ApplyEditOp::ReplaceText {
                        find: "cliente".into(),
                        replace: "Acme".into(),
                        scope: Scope::All,
                    },
                    ApplyEditOp::DeleteText {
                        find: "MISSING_TEXT".into(),
                        scope: Scope::All,
                    },
                ],
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DocsError::TextNotFound { .. }));
    }

    /// Regression for the gdocs apply_edits index-drift bug observed in
    /// agent_session_id=`cmq7kem1h003001s6mr36uwe8` (2026-06-10): a
    /// compound of 7 replace_text ops with mixed single-hit and multi-hit
    /// finds corrupted the doc because the previous implementation only
    /// sorted hits write-backwards INSIDE each sub-edit, not across the
    /// global batch. Once sub-edit 1's insertText shifted indices past
    /// its position, sub-edit 2's snapshot offsets landed on the wrong
    /// chars.
    ///
    /// This test pins the new invariant: the requests posted to
    /// `batchUpdate` must come out in strict (paragraph DESC, byte_off
    /// DESC) order, ignoring the sub-edit boundaries.
    #[tokio::test]
    async fn apply_edits_global_write_backwards_sort_across_sub_edits() {
        let mut rig = TestRig::new();
        // Three paragraphs each containing the same `Ejercicios:` token
        // plus a unique token per paragraph (to verify the multi-hit
        // expansion lands in the right places).
        let s = snap(
            "r1",
            vec![
                (1, ParagraphKind::Paragraph, "Día 1 Ejercicios: A", 1, 21),
                (2, ParagraphKind::Paragraph, "Día 2 Ejercicios: B", 21, 41),
                (3, ParagraphKind::Paragraph, "Día 3 Ejercicios: C", 41, 61),
            ],
        );
        let s2 = s.clone();
        expect_get_sequence(&mut rig.client, vec![s, s2]);
        let captured = make_batch_update_capture(&mut rig.client, "r1");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let _ = super::run(
            &ctx,
            &doc_id(),
            ApplyEditsInput {
                edits: vec![
                    // Sub-edit 1 — single hit on par 1.
                    ApplyEditOp::ReplaceText {
                        find: "A".into(),
                        replace: "AA".into(),
                        scope: Scope::All,
                    },
                    // Sub-edit 2 — multi-hit on pars 1, 2, 3. THIS is
                    // the case that broke before: with per-sub-edit
                    // sort, par 3's request comes AFTER par 2's, AFTER
                    // par 1's, but sub-edit 1's request for par 1 was
                    // already emitted first — so par 2 and par 3 then
                    // operate on stale indices.
                    ApplyEditOp::ReplaceText {
                        find: "Ejercicios:".into(),
                        replace: "**Ejercicios:**".into(),
                        scope: Scope::All,
                    },
                    // Sub-edit 3 — single hit on par 3.
                    ApplyEditOp::ReplaceText {
                        find: "C".into(),
                        replace: "CC".into(),
                        scope: Scope::All,
                    },
                ],
            },
        )
        .await
        .unwrap();

        // The batchUpdate request indices for each deleteContentRange
        // must be strictly descending. If any later request has a
        // start_index >= an earlier one, we're back in drift territory.
        let reqs = captured.lock().unwrap().clone();
        let starts: Vec<u32> = reqs
            .iter()
            .filter_map(|r| {
                r.get("deleteContentRange")
                    .and_then(|d| d.get("range"))
                    .and_then(|rng| rng.get("startIndex"))
                    .and_then(|i| i.as_u64())
                    .map(|i| i as u32)
            })
            .collect();
        assert!(!starts.is_empty(), "expected delete requests in capture");
        for w in starts.windows(2) {
            assert!(
                w[0] >= w[1],
                "delete startIndex order broke write-backwards invariant: {} → {} in {:?}",
                w[0],
                w[1],
                starts
            );
        }
        // Sanity: with 3 paragraphs and 5 total hits (1 + 3 + 1), we
        // expect 5 delete + 5 insert requests = 10.
        assert_eq!(
            reqs.len(),
            10,
            "expected 5 replace ops × 2 requests each, got {} requests",
            reqs.len()
        );
    }

    /// Reject two ops whose byte ranges overlap inside the same
    /// paragraph — even sorted globally, they can't both be applied
    /// against the same snapshot. The error must be structured
    /// `InvalidArgs` (not a silent corruption like before).
    #[tokio::test]
    async fn apply_edits_overlapping_ranges_in_same_paragraph_rejected() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "alpha beta gamma", 1, 18)],
        );
        // No second snapshot needed — the call must fail before any write.
        expect_get_sequence(&mut rig.client, vec![s]);
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        // Two replaces whose ranges overlap: "alpha beta" and "beta gamma"
        // both touch the substring "beta".
        let err = super::run(
            &ctx,
            &doc_id(),
            ApplyEditsInput {
                edits: vec![
                    ApplyEditOp::ReplaceText {
                        find: "alpha beta".into(),
                        replace: "X".into(),
                        scope: Scope::All,
                    },
                    ApplyEditOp::ReplaceText {
                        find: "beta gamma".into(),
                        replace: "Y".into(),
                        scope: Scope::All,
                    },
                ],
            },
        )
        .await
        .unwrap_err();
        match err {
            DocsError::InvalidArgs(msg) => {
                assert!(
                    msg.contains("overlapping"),
                    "expected overlapping-edits error, got: {msg}"
                );
            }
            other => panic!("expected InvalidArgs, got {other:?}"),
        }
    }

    /// Disjoint ranges on the same paragraph must succeed. This is the
    /// common case: the LLM scope-restricts two replaces to non-overlap-
    /// ping substrings inside one paragraph.
    #[tokio::test]
    async fn apply_edits_disjoint_ranges_same_paragraph_ok() {
        let mut rig = TestRig::new();
        let s = snap(
            "r1",
            vec![(1, ParagraphKind::Paragraph, "alpha beta gamma", 1, 18)],
        );
        let s2 = snap(
            "r2",
            vec![(1, ParagraphKind::Paragraph, "X beta Y", 1, 10)],
        );
        expect_get_sequence(&mut rig.client, vec![s, s2]);
        let captured = make_batch_update_capture(&mut rig.client, "r2");
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let _ = super::run(
            &ctx,
            &doc_id(),
            ApplyEditsInput {
                edits: vec![
                    ApplyEditOp::ReplaceText {
                        find: "alpha".into(),
                        replace: "X".into(),
                        scope: Scope::All,
                    },
                    ApplyEditOp::ReplaceText {
                        find: "gamma".into(),
                        replace: "Y".into(),
                        scope: Scope::All,
                    },
                ],
            },
        )
        .await
        .unwrap();
        let reqs = captured.lock().unwrap().clone();
        // 2 ops × 2 requests = 4. And the higher byte_off ("gamma" is
        // after "alpha") must be emitted first → its deleteContentRange
        // startIndex must be greater than alpha's.
        let starts: Vec<u32> = reqs
            .iter()
            .filter_map(|r| {
                r.get("deleteContentRange")
                    .and_then(|d| d.get("range"))
                    .and_then(|rng| rng.get("startIndex"))
                    .and_then(|i| i.as_u64())
                    .map(|i| i as u32)
            })
            .collect();
        assert_eq!(starts.len(), 2);
        assert!(
            starts[0] > starts[1],
            "expected write-backwards order, got {starts:?}"
        );
    }
}
