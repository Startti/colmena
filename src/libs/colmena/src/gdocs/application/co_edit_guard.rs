//! Co-edit safety pipeline. Runs before every edit:
//! 1. Fetch the doc snapshot (cache or fresh).
//! 2. Resolve the intended scope against that snapshot.
//! 3. Compare the snapshot revision_id to what the agent last saw
//!    (from postgres). If they match → proceed.
//! 4. If they differ, fetch revisions in (known, current], filter
//!    out the SA's own revisions → human_revisions.
//! 5. If human_revisions is empty (all changes were ours) → proceed.
//! 6. Otherwise compute paragraph-level diff between prior text
//!    and current text, partition by overlap with the intended scope.
//! 7. If anything overlaps → block with `HumanChangesPending`.
//!    Else → proceed with soft_warnings (the outside-scope changes).
//!
//! See spec `docs/superpowers/specs/2026-06-08-google-docs-design.md` §8.

use crate::gdocs::application::scope_resolver::{self, ResolvedScope};
use crate::gdocs::domain::{
    DocsClient, DocsError, DocumentId, DocumentSnapshot, HumanChange, HumanChangeKind,
    RevisionMeta, Scope,
};
use crate::gdocs::infrastructure::outline_cache::OutlineCache;
use crate::gdocs::infrastructure::revision_store::RevisionStore;

/// What the guard hands back when an edit is allowed to proceed.
#[derive(Debug)]
pub struct GuardOk {
    /// Fresh snapshot of the doc post-guard. Use case can consume
    /// without an extra `client.get`.
    pub snapshot: DocumentSnapshot,
    /// Concrete paragraph range to apply the edit within.
    pub resolved_scope: ResolvedScope,
    /// Human edits outside the requested scope — informational only.
    /// The use case may surface these to the agent.
    pub soft_warnings: Vec<HumanChange>,
}

/// Wiring carried through every edit use case.
pub struct GuardContext<'a> {
    /// Production HTTP client (or mock in tests).
    pub client: &'a dyn DocsClient,
    /// Per-(session, doc) snapshot cache — 5s TTL.
    pub cache: &'a OutlineCache,
    /// Postgres-backed `(agent_session_id, doc_id) → last_revision_id`.
    pub revisions: &'a dyn RevisionStore,
    /// Stable identifier of the current chat session. Required.
    pub session_id: &'a str,
    /// SA email used to identify which revisions are ours (so we
    /// don't mistake our own writes for human changes). `None` is
    /// conservative — every revision treated as human.
    pub sa_email: Option<&'a str>,
}

/// Run the pipeline. Errors with [`DocsError::HumanChangesPending`] when
/// human edits overlap the intended scope.
pub async fn run_guard(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    scope: &Scope,
) -> Result<GuardOk, DocsError> {
    // 1. Snapshot (cache hit or fresh fetch).
    let snapshot = match ctx.cache.get_fresh(ctx.session_id, doc_id) {
        Some(s) => s,
        None => {
            let s = ctx.client.get(doc_id).await?;
            ctx.cache.put(ctx.session_id, doc_id, s.clone());
            s
        }
    };

    // 2. Scope resolution.
    let resolved_scope = scope_resolver::resolve(scope, &snapshot)?;

    // 3. Compare revision ids.
    let known = ctx.revisions.get(ctx.session_id, doc_id).await?;
    let current = snapshot.revision_id.clone();
    let known = match known {
        Some(k) if k != current => k,
        _ => {
            return Ok(GuardOk {
                snapshot,
                resolved_scope,
                soft_warnings: vec![],
            });
        }
    };

    // 4. Fetch revisions since `known`, filter to non-SA.
    let revs = ctx.client.list_revisions_since(doc_id, &known).await?;
    let human_revs: Vec<RevisionMeta> = revs
        .into_iter()
        .filter(|r| {
            match (ctx.sa_email, r.modifying_user_email.as_deref()) {
                (Some(sa), Some(user)) => user != sa,
                // Unattributable revisions are treated as human
                // (conservative).
                (_, None) => true,
                // No SA email known → also conservative.
                (None, _) => true,
            }
        })
        .collect();

    if human_revs.is_empty() {
        // All revisions were ours. Sync our known cursor forward.
        ctx.revisions.put(ctx.session_id, doc_id, &current).await?;
        return Ok(GuardOk {
            snapshot,
            resolved_scope,
            soft_warnings: vec![],
        });
    }

    // 5. Diff prior vs current as text, attribute changes to paragraphs.
    let prior_text = ctx.client.get_at_revision(doc_id, &known).await?;
    let current_text = ctx.client.get_at_revision(doc_id, &current).await?;
    let human_changes = diff_to_paragraph_changes(&prior_text, &current_text, &human_revs);

    // 6. Partition by scope overlap.
    let (overlap, outside): (Vec<_>, Vec<_>) = human_changes.into_iter().partition(|h| {
        h.paragraph >= resolved_scope.paragraph_start && h.paragraph <= resolved_scope.paragraph_end
    });

    if !overlap.is_empty() {
        let since = human_revs
            .iter()
            .map(|r| r.modified_time)
            .min()
            .unwrap_or_else(chrono::Utc::now);
        return Err(DocsError::HumanChangesPending {
            since,
            changes_overlapping_scope: overlap,
            changes_outside_scope: outside,
        });
    }

    // Outside-scope changes return as soft warnings (informational).
    Ok(GuardOk {
        snapshot,
        resolved_scope,
        soft_warnings: outside,
    })
}

/// Compute paragraph-level diff between two plain-text snapshots and
/// attribute each change to a paragraph number in the CURRENT doc.
fn diff_to_paragraph_changes(
    prior: &str,
    current: &str,
    revs: &[RevisionMeta],
) -> Vec<HumanChange> {
    use similar::TextDiff;

    let last_rev = revs.last();
    let modified_time = last_rev
        .map(|r| r.modified_time)
        .unwrap_or_else(chrono::Utc::now);
    let modifying_user = last_rev.and_then(|r| r.modifying_user_email.clone());

    // Split into paragraphs (blank-line-separated would be more accurate
    // but Google's plain-text export joins paragraphs by single newlines
    // — we operate at the line level for v1).
    let prior_lines: Vec<&str> = prior.lines().collect();
    let current_lines: Vec<&str> = current.lines().collect();
    let diff = TextDiff::from_slices(&prior_lines, &current_lines);

    let mut out = Vec::new();
    let mut current_n: u32 = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Equal => {
                current_n += 1;
            }
            similar::ChangeTag::Insert => {
                current_n += 1;
                out.push(HumanChange {
                    kind: HumanChangeKind::Insert,
                    paragraph: current_n,
                    preview: change.value().chars().take(80).collect(),
                    modified_time,
                    modifying_user: modifying_user.clone(),
                });
            }
            similar::ChangeTag::Delete => {
                // Attribute to the next paragraph slot in the CURRENT
                // doc (where the deletion's empty space sits).
                out.push(HumanChange {
                    kind: HumanChangeKind::Delete,
                    paragraph: current_n.saturating_add(1),
                    preview: change.value().chars().take(80).collect(),
                    modified_time,
                    modifying_user: modifying_user.clone(),
                });
            }
        }
    }

    merge_adjacent_to_modify(out)
}

/// Adjacent `Delete` followed by `Insert` at the same paragraph
/// position is most naturally read as a Modify. Collapse them.
fn merge_adjacent_to_modify(changes: Vec<HumanChange>) -> Vec<HumanChange> {
    let mut out = Vec::with_capacity(changes.len());
    let mut iter = changes.into_iter().peekable();
    while let Some(c) = iter.next() {
        if let Some(next) = iter.peek() {
            if c.kind == HumanChangeKind::Delete
                && next.kind == HumanChangeKind::Insert
                && next.paragraph == c.paragraph
            {
                let after = iter.next().unwrap();
                out.push(HumanChange {
                    kind: HumanChangeKind::Modify,
                    paragraph: c.paragraph,
                    preview: format!("- {} → + {}", c.preview, after.preview),
                    modified_time: c.modified_time,
                    modifying_user: c.modifying_user,
                });
                continue;
            }
        }
        out.push(c);
    }
    out
}

// Note: full unit tests for this module live in Task 19 alongside the
// `MockDocsClient`. We add a small smoke test here that exercises the
// pure helpers without any I/O.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::domain::RevisionId;

    fn fake_revs() -> Vec<RevisionMeta> {
        vec![RevisionMeta {
            revision_id: RevisionId("r1".into()),
            modified_time: chrono::Utc::now(),
            modifying_user_email: Some("user@example.com".into()),
        }]
    }

    #[test]
    fn diff_detects_inserted_line() {
        let prior = "Hola\nMundo\n";
        let current = "Hola\nMundo\nNuevo\n";
        let revs = fake_revs();
        let out = diff_to_paragraph_changes(prior, current, &revs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, HumanChangeKind::Insert);
        assert_eq!(out[0].paragraph, 3);
        assert!(out[0].preview.contains("Nuevo"));
    }

    #[test]
    fn diff_collapses_delete_insert_to_modify() {
        let prior = "A\nB\nC\n";
        let current = "A\nX\nC\n";
        let revs = fake_revs();
        let out = diff_to_paragraph_changes(prior, current, &revs);
        // Could be a single Modify at paragraph 2.
        assert!(out.iter().any(|h| h.kind == HumanChangeKind::Modify));
    }
}

#[cfg(test)]
mod guard_tests {
    use super::*;
    use crate::gdocs::application::_test_helpers::*;
    use crate::gdocs::domain::{ParagraphKind, RevisionId};
    use std::sync::{Arc, Mutex};

    fn expect_get_snapshot(
        client: &mut crate::gdocs::domain::traits::MockDocsClient,
        snap: DocumentSnapshot,
    ) {
        client.expect_get().returning(move |_| Ok(snap.clone()));
    }

    #[tokio::test]
    async fn guard_no_known_revision_proceeds() {
        let mut rig = TestRig::new();
        let s = snap("r1", vec![(1, ParagraphKind::Paragraph, "Hola", 1, 6)]);
        expect_get_snapshot(&mut rig.client, s);
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let ok = run_guard(&ctx, &doc_id(), &Scope::All).await.unwrap();
        assert_eq!(ok.snapshot.revision_id.0, "r1");
        assert!(ok.soft_warnings.is_empty());
    }

    #[tokio::test]
    async fn guard_known_equals_current_proceeds_without_listing_revisions() {
        let mut rig = TestRig::new();
        let s = snap("rX", vec![(1, ParagraphKind::Paragraph, "Hola", 1, 6)]);
        expect_get_snapshot(&mut rig.client, s);
        // Seed the same revision the snapshot will report.
        rig.revisions
            .put("s1", &doc_id(), &RevisionId("rX".into()))
            .await
            .unwrap();
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        // No expect_list_revisions_since — would panic if called.
        let ok = run_guard(&ctx, &doc_id(), &Scope::All).await.unwrap();
        assert_eq!(ok.snapshot.revision_id.0, "rX");
    }

    #[tokio::test]
    async fn guard_all_revisions_are_ours_proceeds() {
        let mut rig = TestRig::new();
        let s = snap("r2", vec![(1, ParagraphKind::Paragraph, "Hola", 1, 6)]);
        expect_get_snapshot(&mut rig.client, s);
        rig.revisions
            .put("s1", &doc_id(), &RevisionId("r1".into()))
            .await
            .unwrap();
        rig.client.expect_list_revisions_since().returning(|_, _| {
            Ok(vec![RevisionMeta {
                revision_id: RevisionId("r2".into()),
                modified_time: chrono::Utc::now(),
                modifying_user_email: Some("sa@proj.iam.gserviceaccount.com".into()),
            }])
        });
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: Some("sa@proj.iam.gserviceaccount.com"),
        };
        let ok = run_guard(&ctx, &doc_id(), &Scope::All).await.unwrap();
        assert!(ok.soft_warnings.is_empty());
        // Cursor advanced.
        let cursor = rig.revisions.get("s1", &doc_id()).await.unwrap().unwrap();
        assert_eq!(cursor.0, "r2");
    }

    #[tokio::test]
    async fn guard_human_overlap_blocks() {
        let mut rig = TestRig::new();
        let s = snap(
            "r2",
            vec![
                (1, ParagraphKind::Paragraph, "Linea uno", 1, 11),
                (2, ParagraphKind::Paragraph, "Linea dos modificada", 11, 33),
            ],
        );
        expect_get_snapshot(&mut rig.client, s);
        rig.revisions
            .put("s1", &doc_id(), &RevisionId("r1".into()))
            .await
            .unwrap();
        rig.client.expect_list_revisions_since().returning(|_, _| {
            Ok(vec![RevisionMeta {
                revision_id: RevisionId("r2".into()),
                modified_time: chrono::Utc::now(),
                modifying_user_email: Some("user@example.com".into()),
            }])
        });
        // Diff inputs: prior vs current differ on line 2.
        let prior_txt = "Linea uno\nLinea dos original\n".to_string();
        let curr_txt = "Linea uno\nLinea dos modificada\n".to_string();
        let prior_clone = prior_txt.clone();
        let curr_clone = curr_txt.clone();
        let texts = Arc::new(Mutex::new(vec![prior_clone, curr_clone]));
        rig.client.expect_get_at_revision().returning(move |_, _| {
            let mut t = texts.lock().unwrap();
            Ok(t.remove(0))
        });
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: Some("sa@proj.iam.gserviceaccount.com"),
        };
        let err = run_guard(&ctx, &doc_id(), &Scope::All).await.unwrap_err();
        match err {
            DocsError::HumanChangesPending {
                changes_overlapping_scope,
                ..
            } => {
                assert!(!changes_overlapping_scope.is_empty());
            }
            e => panic!("expected HumanChangesPending, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn guard_human_outside_scope_soft_warns() {
        let mut rig = TestRig::new();
        // Scope is paragraph 1; the human change lands on paragraph 2.
        let s = snap(
            "r2",
            vec![
                (1, ParagraphKind::Paragraph, "Linea uno", 1, 11),
                (2, ParagraphKind::Paragraph, "Linea dos modificada", 11, 33),
            ],
        );
        expect_get_snapshot(&mut rig.client, s);
        rig.revisions
            .put("s1", &doc_id(), &RevisionId("r1".into()))
            .await
            .unwrap();
        rig.client.expect_list_revisions_since().returning(|_, _| {
            Ok(vec![RevisionMeta {
                revision_id: RevisionId("r2".into()),
                modified_time: chrono::Utc::now(),
                modifying_user_email: Some("user@example.com".into()),
            }])
        });
        let texts = Arc::new(Mutex::new(vec![
            "Linea uno\nLinea dos original\n".to_string(),
            "Linea uno\nLinea dos modificada\n".to_string(),
        ]));
        rig.client.expect_get_at_revision().returning(move |_, _| {
            let mut t = texts.lock().unwrap();
            Ok(t.remove(0))
        });
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: Some("sa@proj.iam.gserviceaccount.com"),
        };
        let ok = run_guard(&ctx, &doc_id(), &Scope::Paragraph { n: 1 })
            .await
            .unwrap();
        // Paragraph-1 scope → paragraph-2 change is outside → soft warning.
        assert!(!ok.soft_warnings.is_empty());
    }

    #[tokio::test]
    async fn guard_unknown_user_treated_as_human() {
        let mut rig = TestRig::new();
        let s = snap(
            "r2",
            vec![(1, ParagraphKind::Paragraph, "Hola modificado", 1, 18)],
        );
        expect_get_snapshot(&mut rig.client, s);
        rig.revisions
            .put("s1", &doc_id(), &RevisionId("r1".into()))
            .await
            .unwrap();
        rig.client.expect_list_revisions_since().returning(|_, _| {
            Ok(vec![RevisionMeta {
                revision_id: RevisionId("r2".into()),
                modified_time: chrono::Utc::now(),
                modifying_user_email: None, // redacted
            }])
        });
        let texts = Arc::new(Mutex::new(vec![
            "Hola\n".to_string(),
            "Hola modificado\n".to_string(),
        ]));
        rig.client.expect_get_at_revision().returning(move |_, _| {
            let mut t = texts.lock().unwrap();
            Ok(t.remove(0))
        });
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: Some("sa@proj.iam.gserviceaccount.com"),
        };
        // Redacted modifying_user_email → conservative; the SA filter
        // can't exclude it → at least one human change attributed.
        let err = run_guard(&ctx, &doc_id(), &Scope::All).await.unwrap_err();
        assert!(matches!(err, DocsError::HumanChangesPending { .. }));
    }

    #[tokio::test]
    async fn guard_uses_cache_hit_skipping_get() {
        let rig = TestRig::new();
        let s = snap("r1", vec![(1, ParagraphKind::Paragraph, "Hola", 1, 6)]);
        // Pre-seed cache.
        rig.cache.put("s1", &doc_id(), s.clone());
        // expect_get is NOT set → would panic if hit.
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let ok = run_guard(&ctx, &doc_id(), &Scope::All).await.unwrap();
        assert_eq!(ok.snapshot.revision_id.0, "r1");
    }
}
