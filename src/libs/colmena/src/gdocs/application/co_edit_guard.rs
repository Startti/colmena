//! Co-edit safety pipeline. Runs before every edit.
//!
//! Pipeline (v1.1, 2026-06-09):
//! 1. Fetch the doc snapshot (5s cache or fresh `documents.get`).
//! 2. Resolve the intended scope against that snapshot.
//! 3. Read `(prior_revision, prior_snapshot)` from the `RevisionStore`.
//!    - None → first contact: proceed without diffing.
//!    - Equal revisions → no drift: proceed.
//!    - Different revisions → drift; proceed to step 4.
//! 4. If a `prior_snapshot` is available, run `paragraph_diff(prior,
//!    current)` and partition the resulting [`HumanChange`]s by
//!    `ResolvedScope::contains_paragraph`.
//!    - Any overlap with scope → block with `HumanChangesPending`
//!      populated.
//!    - No overlap → proceed with `soft_warnings` (outside-scope
//!      changes for the agent's awareness).
//! 5. If the prior snapshot is missing (older instance, oversized doc,
//!    or migration not applied), fall back to v1 behavior:
//!    conservative block with empty change lists.
//!
//! See spec `docs/superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md`.

use crate::gdocs::application::diff::paragraph_diff;
use crate::gdocs::application::scope_resolver::{self, ResolvedScope};
use crate::gdocs::domain::{
    DocsClient, DocsError, DocumentId, DocumentSnapshot, HumanChange, Scope,
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

    // 3. Read prior revision + snapshot.
    //
    // v1 (2026-06-08) shipped with revisionId equality only — the guard
    // knew SOMETHING changed but could not say WHAT, since Drive's
    // Revisions API doesn't expose per-edit logs for Google-native docs.
    //
    // v1.1 (2026-06-09) augments the store with the post-write
    // `DocumentSnapshot`. On drift we run a Myers diff (`paragraph_diff`)
    // between the stored prior snapshot and the freshly-fetched current
    // snapshot, then partition the resulting `HumanChange`s by overlap
    // with the agent's intended scope:
    //   - any overlap with scope → block with the populated lists
    //   - no overlap, drift purely outside scope → proceed with
    //     `soft_warnings` for the agent's awareness
    //
    // When the prior snapshot is unavailable (older instance, oversized
    // doc, or migration not applied), we fall back to v1 behavior —
    // conservative block with empty lists.
    let (known, prior_snap) = ctx
        .revisions
        .get_with_snapshot(ctx.session_id, doc_id)
        .await?;
    let current_rev = snapshot.revision_id.clone();
    match known {
        None => {
            // First contact for this (session, doc) — use case writes
            // will persist the post-write snapshot.
            Ok(GuardOk {
                snapshot,
                resolved_scope,
                soft_warnings: vec![],
            })
        }
        Some(k) if k == current_rev => {
            // No drift — proceed.
            Ok(GuardOk {
                snapshot,
                resolved_scope,
                soft_warnings: vec![],
            })
        }
        Some(_) => match prior_snap {
            // Drift + prior snapshot available → diff + partition.
            Some(prior) => {
                let changes = paragraph_diff(&prior, &snapshot);
                let (overlap, outside) = partition_by_scope(changes, &resolved_scope);
                if !overlap.is_empty() {
                    Err(DocsError::HumanChangesPending {
                        since: chrono::Utc::now(),
                        changes_overlapping_scope: overlap,
                        changes_outside_scope: outside,
                    })
                } else {
                    Ok(GuardOk {
                        snapshot,
                        resolved_scope,
                        soft_warnings: outside,
                    })
                }
            }
            // Drift but no prior snapshot — degraded v1 behavior:
            // conservative block with empty lists. We can't prove the
            // change is outside scope, so don't relax safety.
            None => Err(DocsError::HumanChangesPending {
                since: chrono::Utc::now(),
                changes_overlapping_scope: vec![],
                changes_outside_scope: vec![],
            }),
        },
    }
}

/// Split a list of human changes into `(overlapping_scope, outside_scope)`
/// using the resolved scope's tab + paragraph range.
fn partition_by_scope(
    changes: Vec<HumanChange>,
    scope: &ResolvedScope,
) -> (Vec<HumanChange>, Vec<HumanChange>) {
    let (mut overlap, mut outside) = (Vec::new(), Vec::new());
    for c in changes {
        if scope.contains_paragraph(c.tab_id.as_ref(), c.paragraph) {
            overlap.push(c);
        } else {
            outside.push(c);
        }
    }
    (overlap, outside)
}

// Unit tests for this module live alongside the `MockDocsClient`
// (`gdocs::application::_test_helpers`). The v1.1 paragraph diff is
// covered by tests below + the `paragraph_diff` test suite in
// `gdocs::application::diff`.

#[cfg(test)]
mod guard_tests {
    use super::*;
    use crate::gdocs::application::_test_helpers::*;
    use crate::gdocs::domain::{HumanChangeKind, ParagraphKind, RevisionId};

    fn expect_get_snapshot(
        client: &mut crate::gdocs::domain::traits::MockDocsClient,
        snap: DocumentSnapshot,
    ) {
        client.expect_get().returning(move |_| Ok(snap.clone()));
    }

    /// First-contact path: no stored revision → proceed without blocking,
    /// regardless of what the snapshot says.
    #[tokio::test]
    async fn guard_first_contact_proceeds() {
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

    /// Stored revision equals current → no drift → proceed.
    #[tokio::test]
    async fn guard_known_equals_current_proceeds() {
        let mut rig = TestRig::new();
        let s = snap("rX", vec![(1, ParagraphKind::Paragraph, "Hola", 1, 6)]);
        expect_get_snapshot(&mut rig.client, s);
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
        let ok = run_guard(&ctx, &doc_id(), &Scope::All).await.unwrap();
        assert_eq!(ok.snapshot.revision_id.0, "rX");
    }

    /// Stored revision differs from current → block with HumanChangesPending.
    /// v1 returns empty change lists because Drive doesn't expose per-edit
    /// revisions for native Google Docs; agent must inspect the doc.
    #[tokio::test]
    async fn guard_revision_drift_blocks() {
        let mut rig = TestRig::new();
        let s = snap("r_new", vec![(1, ParagraphKind::Paragraph, "Hola", 1, 6)]);
        expect_get_snapshot(&mut rig.client, s);
        rig.revisions
            .put("s1", &doc_id(), &RevisionId("r_old".into()))
            .await
            .unwrap();
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let err = run_guard(&ctx, &doc_id(), &Scope::All).await.unwrap_err();
        match err {
            DocsError::HumanChangesPending {
                changes_overlapping_scope,
                changes_outside_scope,
                ..
            } => {
                // v1: no per-edit log available — both lists are empty.
                assert!(changes_overlapping_scope.is_empty());
                assert!(changes_outside_scope.is_empty());
            }
            other => panic!("expected HumanChangesPending, got {:?}", other),
        }
    }

    /// Cache hit short-circuits client.get() — verified by NOT setting
    /// expect_get (mockall would panic if hit).
    #[tokio::test]
    async fn guard_uses_cache_hit_skipping_get() {
        let rig = TestRig::new();
        let s = snap("r1", vec![(1, ParagraphKind::Paragraph, "Hola", 1, 6)]);
        rig.cache.put("s1", &doc_id(), s.clone());
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

    /// v1.1: drift + prior snapshot present + change overlaps scope →
    /// block with populated `changes_overlapping_scope`.
    #[tokio::test]
    async fn guard_drift_with_snapshot_overlap_blocks_with_details() {
        let mut rig = TestRig::new();
        let prior = snap("r_old", vec![(1, ParagraphKind::Paragraph, "Hola", 1, 6)]);
        let current = snap(
            "r_new",
            vec![(1, ParagraphKind::Paragraph, "Hola modificado", 1, 16)],
        );
        expect_get_snapshot(&mut rig.client, current);
        rig.revisions
            .put_with_snapshot("s1", &doc_id(), &RevisionId("r_old".into()), Some(&prior))
            .await
            .unwrap();
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let err = run_guard(&ctx, &doc_id(), &Scope::All).await.unwrap_err();
        match err {
            DocsError::HumanChangesPending {
                changes_overlapping_scope,
                changes_outside_scope,
                ..
            } => {
                assert_eq!(changes_overlapping_scope.len(), 1);
                assert_eq!(changes_overlapping_scope[0].kind, HumanChangeKind::Modify);
                assert_eq!(
                    changes_overlapping_scope[0].before_text.as_deref(),
                    Some("Hola")
                );
                assert_eq!(
                    changes_overlapping_scope[0].after_text.as_deref(),
                    Some("Hola modificado")
                );
                assert!(changes_outside_scope.is_empty());
            }
            other => panic!("expected HumanChangesPending, got {:?}", other),
        }
    }

    /// v1.1: drift + change is outside the requested scope → proceed
    /// with the change reported as `soft_warnings`.
    #[tokio::test]
    async fn guard_drift_outside_scope_proceeds_with_soft_warnings() {
        let mut rig = TestRig::new();
        let prior = snap(
            "r_old",
            vec![
                (1, ParagraphKind::Paragraph, "Hola", 1, 6),
                (2, ParagraphKind::Paragraph, "Adios", 7, 13),
            ],
        );
        let current = snap(
            "r_new",
            vec![
                (1, ParagraphKind::Paragraph, "Hola", 1, 6),
                (2, ParagraphKind::Paragraph, "Adios cambiado", 7, 23),
            ],
        );
        expect_get_snapshot(&mut rig.client, current);
        rig.revisions
            .put_with_snapshot("s1", &doc_id(), &RevisionId("r_old".into()), Some(&prior))
            .await
            .unwrap();
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let ok = run_guard(&ctx, &doc_id(), &Scope::Paragraph { n: 1 })
            .await
            .unwrap();
        assert_eq!(ok.soft_warnings.len(), 1);
        assert_eq!(ok.soft_warnings[0].paragraph, 2);
        assert_eq!(ok.soft_warnings[0].kind, HumanChangeKind::Modify);
    }

    /// v1.1 degraded path: drift but no prior snapshot stored → fall
    /// back to v1 conservative block with empty lists.
    #[tokio::test]
    async fn guard_drift_without_snapshot_falls_back_to_v1() {
        let mut rig = TestRig::new();
        let current = snap("r_new", vec![(1, ParagraphKind::Paragraph, "Hola", 1, 6)]);
        expect_get_snapshot(&mut rig.client, current);
        // Persist revision only (no snapshot) — simulates an older v1
        // session-state row.
        rig.revisions
            .put_with_snapshot("s1", &doc_id(), &RevisionId("r_old".into()), None)
            .await
            .unwrap();
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let err = run_guard(&ctx, &doc_id(), &Scope::All).await.unwrap_err();
        match err {
            DocsError::HumanChangesPending {
                changes_overlapping_scope,
                changes_outside_scope,
                ..
            } => {
                assert!(changes_overlapping_scope.is_empty());
                assert!(changes_outside_scope.is_empty());
            }
            other => panic!("expected HumanChangesPending, got {:?}", other),
        }
    }
}
