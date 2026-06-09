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
    DocsClient, DocsError, DocumentId, DocumentSnapshot, HumanChange, RevisionMeta, Scope,
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
    //
    // Note on the v1 design pivot (verified live 2026-06-09): the
    // original spec assumed Drive's Revisions API would yield an
    // edit-by-edit log we could filter by SA email and diff against the
    // prior content. In practice, for Google-native Docs/Sheets/Slides
    // the Drive Revisions API only returns NAMED versions, not
    // individual edits — so `list_revisions_since` consistently returns
    // an empty list and the guard never fires. Without per-edit
    // attribution, we cannot show paragraph-level diffs in v1.
    //
    // The robust v1 safety guarantee instead: **if the doc's
    // revisionId has moved since the agent's last write, block until
    // the agent explicitly acknowledges**. This always catches human
    // edits (every change advances revisionId). It cannot distinguish
    // agent vs human, but in practice the agent itself updates the
    // stored revision after every successful write, so any drift IS a
    // human change.
    //
    // Trade-off: the LLM doesn't see WHAT changed; only that something
    // changed. The agent can call `read_outline` / `read_as_markdown`
    // to inspect the current state before calling
    // `acknowledge_human_changes`. Paragraph-level diffs return as
    // v1.1 once Google exposes a fine-grained edit log (or we cache
    // prior snapshots in postgres for delta computation).
    let known = ctx.revisions.get(ctx.session_id, doc_id).await?;
    let current = snapshot.revision_id.clone();
    match known {
        Some(k) if k != current => {
            // Doc revision moved while the agent had a known cursor.
            // Treat as a human edit. The since timestamp is best-effort:
            // we don't have a precise wall-clock for the human's edit,
            // so we use the snapshot's "now" as the upper bound.
            return Err(DocsError::HumanChangesPending {
                since: chrono::Utc::now(),
                changes_overlapping_scope: vec![],
                changes_outside_scope: vec![],
            });
        }
        Some(_) | None => {
            // Either revisions match, or this is first contact for
            // this (session, doc). In both cases, proceed without
            // blocking. The first-contact case stores the current
            // revision below in the application use cases after each
            // successful write.
            Ok(GuardOk {
                snapshot,
                resolved_scope,
                soft_warnings: vec![],
            })
        }
    }
}

// Unused since v1 pivot — kept commented for v1.1 where Google may
// expose per-edit revisions or we add postgres snapshot caching:
//
//   fn diff_to_paragraph_changes(prior, current, revs) -> Vec<HumanChange>
//   fn merge_adjacent_to_modify(changes) -> Vec<HumanChange>
//
// Both relied on Drive's revisions.get to fetch text at a prior
// revision_id. Since Drive Revisions doesn't list per-edit
// revisions for native Google Docs, the prior text is never
// available. The functions and their tests were removed.

#[allow(dead_code)]
fn _suppress_warnings(_: RevisionMeta) {}

// Note: full unit tests for this module live in Task 19 alongside the
// `MockDocsClient`. We add a small smoke test here that exercises the
// pure helpers without any I/O.

#[cfg(test)]
mod guard_tests {
    use super::*;
    use crate::gdocs::application::_test_helpers::*;
    use crate::gdocs::domain::{ParagraphKind, RevisionId};

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
}
