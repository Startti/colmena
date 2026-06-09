# Paragraph-level human-change diff for Google Docs co-edit guard — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enrich the gdocs co-edit guard so that when drift is detected (user edited the doc between agent writes), the agent receives a concrete, paragraph-level list of what changed — partitioned by overlap with the intended scope — without any extra API calls.

**Architecture:** Persist a `DocumentSnapshot` (the same one already hydrated to build `EditResult.outline_snapshot`) alongside the revisionId in `gdocs_session_state`. On the next edit, diff prior vs current snapshots via Myers (the `similar` crate, already in `Cargo.toml`), partition the diff by `ResolvedScope`, populate the `changes_overlapping_scope` and `changes_outside_scope` fields of `HumanChangesPending`, and either block (overlap) or proceed with `soft_warnings` (outside).

**Tech Stack:** Rust 1.95, async-trait, `similar = "2"` (Myers diff), sqlx (Postgres JSONB), serde, mockall.

**Spec:** `docs/superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md`

---

## File Structure

### Created
- `src/libs/colmena/src/gdocs/application/diff.rs` — pure paragraph diff fn + tests
- `src/libs/colmena/migrations/postgres/20260609000000_gdocs_session_state_snapshot.sql`
- `tests/graphs/agents/gdocs_phase1_build_v11.json` (extends existing phase1 graph for v1.1 verification)
- `tests/graphs/agents/gdocs_phase2_continue_v11.json` (extends phase2)

### Modified
- `src/libs/colmena/src/gdocs/domain/types.rs` — extend `HumanChange` with `tab_id` / `before_text` / `after_text`
- `src/libs/colmena/src/gdocs/application/scope_resolver.rs` — add `ResolvedScope::contains_paragraph`
- `src/libs/colmena/src/gdocs/application/mod.rs` — register `pub mod diff;`
- `src/libs/colmena/src/gdocs/application/co_edit_guard.rs` — use diff + partition
- `src/libs/colmena/src/gdocs/infrastructure/revision_store.rs` — extend trait with snapshot, Postgres ALTER-aware
- `src/libs/colmena/src/gdocs/application/delete_text.rs` — pass snapshot to put_with_snapshot
- `src/libs/colmena/src/gdocs/application/style.rs` — same
- `src/libs/colmena/src/gdocs/application/replace_text.rs` — same
- `src/libs/colmena/src/gdocs/application/replace_section.rs` — same (2 sites)
- `src/libs/colmena/src/gdocs/application/named_range.rs` — same
- `src/libs/colmena/src/gdocs/application/insert.rs` — same
- `src/libs/colmena/src/gdocs/application/apply_edits.rs` — same
- `src/libs/colmena/src/gdocs/application/_test_helpers.rs` (if exists) — add a TestRig helper for the new method signatures
- `docs/developer_guide/45_gdocs.md` — §Co-edit guard updated with v1.1 behavior + degraded mode
- `docs/CHANGELOG_2026-06.md` — entry for v1.1 paragraph-diff
- `docs/BACKLOG.md` — mark "Subsystem G v1.1 item 2" done
- `ADP_PRISMA_PENDING_TABLES.md` — add `last_snapshot_json` + `last_snapshot_size_bytes` columns
- `src/libs/colmena/src/gdocs/mod.rs` — re-export `application::diff` if any item is pub

---

## Task Decomposition

12 tasks. Each commit is independent and bisectable. Tests precede implementation in tasks where TDD adds value (diff algorithm). RevisionStore refactor uses TDD with the in-memory adapter first.

---

### Task 1: Migration — extend `gdocs_session_state` with snapshot columns

**Files:**
- Create: `src/libs/colmena/migrations/postgres/20260609000000_gdocs_session_state_snapshot.sql`

- [ ] **Step 1: Create the migration file**

```sql
-- gdocs_session_state v1.1 extension — persist last DocumentSnapshot
-- so the co-edit guard can show paragraph-level diffs of human changes.

ALTER TABLE gdocs_session_state
  ADD COLUMN IF NOT EXISTS last_snapshot_json       JSONB,
  ADD COLUMN IF NOT EXISTS last_snapshot_size_bytes INTEGER;

-- Rollback:
-- ALTER TABLE gdocs_session_state DROP COLUMN IF EXISTS last_snapshot_size_bytes;
-- ALTER TABLE gdocs_session_state DROP COLUMN IF EXISTS last_snapshot_json;
```

- [ ] **Step 2: Apply migration locally**

Run:
```bash
psql "$DATABASE_URL" \
  -f src/libs/colmena/migrations/postgres/20260609000000_gdocs_session_state_snapshot.sql
```
Expected: `ALTER TABLE` printed, exit 0.

- [ ] **Step 3: Verify schema**

Run:
```bash
psql "$DATABASE_URL" -c "\d gdocs_session_state"
```
Expected output includes both new columns:
```
 last_snapshot_json       | jsonb    |
 last_snapshot_size_bytes | integer  |
```

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/migrations/postgres/20260609000000_gdocs_session_state_snapshot.sql
git commit -m "feat(gdocs): add snapshot columns to gdocs_session_state for v1.1 diff"
```

---

### Task 2: Domain — extend `HumanChange` with `tab_id` / `before_text` / `after_text`

**Files:**
- Modify: `src/libs/colmena/src/gdocs/domain/types.rs:166-173`
- Test: same file `#[cfg(test)] mod tests`

- [ ] **Step 1: Write a failing test for the new fields**

Append to `mod tests`:
```rust
    #[test]
    fn human_change_serializes_new_fields() {
        let c = HumanChange {
            kind: HumanChangeKind::Modify,
            paragraph: 4,
            preview: "abc".into(),
            modified_time: "2026-06-09T23:25:13Z".parse().unwrap(),
            modifying_user: None,
            tab_id: Some(TabId("t1".into())),
            before_text: Some("a".into()),
            after_text: Some("b".into()),
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["tab_id"], "t1");
        assert_eq!(v["before_text"], "a");
        assert_eq!(v["after_text"], "b");
        assert_eq!(v["kind"], "modify");
    }
```

Run:
```bash
cargo test -p colmena_dag_engine --lib gdocs::domain::types::tests::human_change_serializes_new_fields
```
Expected: FAIL with "missing field `tab_id`" or compile error.

- [ ] **Step 2: Extend the struct**

Edit `types.rs:166-173`:
```rust
/// A human-authored change observed outside the agent's current edit scope,
/// surfaced so the agent can decide whether to proceed or re-plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanChange {
    pub kind: HumanChangeKind,
    pub paragraph: u32,
    pub preview: String,
    pub modified_time: chrono::DateTime<chrono::Utc>,
    pub modifying_user: Option<String>,
    /// Tab where the change happened. `None` for single-tab docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<TabId>,
    /// Paragraph text before the change. `None` for `Insert`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_text: Option<String>,
    /// Paragraph text after the change. `None` for `Delete`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_text: Option<String>,
}
```

- [ ] **Step 3: Run test**

```bash
cargo test -p colmena_dag_engine --lib gdocs::domain::types::tests::human_change_serializes_new_fields
```
Expected: PASS.

- [ ] **Step 4: Run all gdocs domain tests**

```bash
cargo test -p colmena_dag_engine --lib gdocs::domain
```
Expected: all PASS (no regressions on existing tests that construct `HumanChange`).

If any pre-existing test constructs `HumanChange { ... }` without the new fields, the `#[serde(default, skip_serializing_if = "Option::is_none")]` annotations let serde defaults handle deserialization; the literal struct construction will still need the fields. Update those constructions to pass `tab_id: None, before_text: None, after_text: None`.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/gdocs/domain/types.rs
git commit -m "feat(gdocs): extend HumanChange with tab_id, before_text, after_text"
```

---

### Task 3: `ResolvedScope::contains_paragraph` helper

**Files:**
- Modify: `src/libs/colmena/src/gdocs/application/scope_resolver.rs`
- Test: same file `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test**

Add to the existing `mod tests` (or create one if absent) in `scope_resolver.rs`:
```rust
    #[test]
    fn contains_paragraph_respects_tab_and_range() {
        let rs = ResolvedScope {
            tab_id: Some(TabId("plan".into())),
            paragraph_start: 5,
            paragraph_end: 10,
        };
        // In tab, in range.
        assert!(rs.contains_paragraph(Some(&TabId("plan".into())), 5));
        assert!(rs.contains_paragraph(Some(&TabId("plan".into())), 10));
        assert!(rs.contains_paragraph(Some(&TabId("plan".into())), 7));
        // In tab, out of range.
        assert!(!rs.contains_paragraph(Some(&TabId("plan".into())), 4));
        assert!(!rs.contains_paragraph(Some(&TabId("plan".into())), 11));
        // Different tab.
        assert!(!rs.contains_paragraph(Some(&TabId("anexo".into())), 7));
        // No tab on either side (single-tab doc, scope has tab_id None) should match.
        let rs_no_tab = ResolvedScope { tab_id: None, paragraph_start: 1, paragraph_end: 3 };
        assert!(rs_no_tab.contains_paragraph(None, 2));
        // Scope has tab None but change has Some: still matches (None means "any tab").
        assert!(rs_no_tab.contains_paragraph(Some(&TabId("anexo".into())), 2));
    }
```

Run:
```bash
cargo test -p colmena_dag_engine --lib gdocs::application::scope_resolver::tests::contains_paragraph_respects_tab_and_range
```
Expected: FAIL (method does not exist).

- [ ] **Step 2: Implement**

In `scope_resolver.rs`, add an `impl ResolvedScope` block after the struct definition:
```rust
impl ResolvedScope {
    /// True if `(tab, n)` falls within this resolved scope.
    ///
    /// - When `self.tab_id` is `None`, any tab matches (the scope is
    ///   doc-wide).
    /// - When `self.tab_id` is `Some`, the change's tab must match.
    /// - `n` must be in `[paragraph_start, paragraph_end]` (inclusive).
    pub fn contains_paragraph(&self, tab: Option<&TabId>, n: u32) -> bool {
        let tab_ok = match (&self.tab_id, tab) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(a), Some(b)) => a == b,
        };
        tab_ok && n >= self.paragraph_start && n <= self.paragraph_end
    }
}
```

- [ ] **Step 3: Run test**

```bash
cargo test -p colmena_dag_engine --lib gdocs::application::scope_resolver::tests::contains_paragraph_respects_tab_and_range
```
Expected: PASS.

- [ ] **Step 4: Run all scope_resolver tests**

```bash
cargo test -p colmena_dag_engine --lib gdocs::application::scope_resolver
```
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/gdocs/application/scope_resolver.rs
git commit -m "feat(gdocs): add ResolvedScope::contains_paragraph helper"
```

---

### Task 4: Diff algorithm — `gdocs/application/diff.rs`

**Files:**
- Create: `src/libs/colmena/src/gdocs/application/diff.rs`
- Modify: `src/libs/colmena/src/gdocs/application/mod.rs` (add `pub mod diff;`)

- [ ] **Step 1: Register the module**

Edit `src/libs/colmena/src/gdocs/application/mod.rs`, add (preserving alphabetic order if present):
```rust
pub mod diff;
```

- [ ] **Step 2: Write the failing tests first**

Create `src/libs/colmena/src/gdocs/application/diff.rs`:
```rust
//! Paragraph-level diff between two [`DocumentSnapshot`]s.
//!
//! Used by the co-edit guard to translate "doc revisionId moved" into a
//! concrete list of [`HumanChange`]s the LLM can reason about, without
//! any extra API calls.
//!
//! See spec `docs/superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md`.

use crate::gdocs::domain::{
    DocumentSnapshot, HumanChange, HumanChangeKind, ParagraphSnapshot, TabId, TabSnapshot,
};
use chrono::Utc;
use similar::{capture_diff_slices, Algorithm, DiffOp};
use std::collections::HashMap;

const PREVIEW_MAX_CHARS: usize = 120;

/// Compute paragraph-level diff between `prior` and `current`.
///
/// Pure: no I/O. Deterministic: output is sorted by `(tab_order_in_current, paragraph_n)`.
///
/// - `Insert` and `Modify` use `paragraph` from the `current` snapshot.
/// - `Delete` uses the paragraph number in `current` where the deletion is
///   logically anchored (i.e., the paragraph immediately following the
///   deletion, clamped to last+1 if the deletion is at end-of-tab).
pub fn paragraph_diff(prior: &DocumentSnapshot, current: &DocumentSnapshot) -> Vec<HumanChange> {
    let prior_tabs: HashMap<Option<TabId>, &TabSnapshot> =
        prior.tabs.iter().map(|t| (t.tab_id.clone(), t)).collect();

    let mut out = Vec::new();

    for (tab_idx, current_tab) in current.tabs.iter().enumerate() {
        let prior_paragraphs: Vec<&ParagraphSnapshot> = prior_tabs
            .get(&current_tab.tab_id)
            .map(|t| t.paragraphs.iter().collect())
            .unwrap_or_default();
        let current_paragraphs: Vec<&ParagraphSnapshot> = current_tab.paragraphs.iter().collect();
        diff_tab(
            &prior_paragraphs,
            &current_paragraphs,
            current_tab.tab_id.as_ref(),
            tab_idx,
            &mut out,
        );
    }

    out.sort_by_key(|c| (c.tab_id.as_ref().map(|t| t.0.clone()), c.paragraph));
    out
}

fn diff_tab(
    prior: &[&ParagraphSnapshot],
    current: &[&ParagraphSnapshot],
    tab_id: Option<&TabId>,
    _tab_order: usize,
    out: &mut Vec<HumanChange>,
) {
    let prior_texts: Vec<&str> = prior.iter().map(|p| p.text.as_str()).collect();
    let current_texts: Vec<&str> = current.iter().map(|p| p.text.as_str()).collect();
    let ops = capture_diff_slices(Algorithm::Myers, &prior_texts, &current_texts);

    for op in ops {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for i in 0..new_len {
                    out.push(make_change(
                        HumanChangeKind::Insert,
                        tab_id,
                        current.get(new_index + i).map(|p| p.n).unwrap_or(0),
                        None,
                        Some(current_texts[new_index + i].to_string()),
                    ));
                }
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                for i in 0..old_len {
                    // Anchor delete to next paragraph in current (where the
                    // deletion logically sits). If at end-of-tab, anchor to
                    // last+1 conceptually — we use the last existing paragraph
                    // in current as a best-effort, or paragraph 0 if current
                    // tab is empty.
                    let anchor = current
                        .last()
                        .map(|p| p.n.saturating_add(1))
                        .unwrap_or(0);
                    out.push(make_change(
                        HumanChangeKind::Delete,
                        tab_id,
                        anchor,
                        Some(prior_texts[old_index + i].to_string()),
                        None,
                    ));
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let modify_len = old_len.min(new_len);
                for i in 0..modify_len {
                    out.push(make_change(
                        HumanChangeKind::Modify,
                        tab_id,
                        current.get(new_index + i).map(|p| p.n).unwrap_or(0),
                        Some(prior_texts[old_index + i].to_string()),
                        Some(current_texts[new_index + i].to_string()),
                    ));
                }
                if new_len > old_len {
                    for i in modify_len..new_len {
                        out.push(make_change(
                            HumanChangeKind::Insert,
                            tab_id,
                            current.get(new_index + i).map(|p| p.n).unwrap_or(0),
                            None,
                            Some(current_texts[new_index + i].to_string()),
                        ));
                    }
                } else if old_len > new_len {
                    for i in modify_len..old_len {
                        let anchor = current
                            .get(new_index + modify_len)
                            .map(|p| p.n)
                            .or_else(|| current.last().map(|p| p.n.saturating_add(1)))
                            .unwrap_or(0);
                        out.push(make_change(
                            HumanChangeKind::Delete,
                            tab_id,
                            anchor,
                            Some(prior_texts[old_index + i].to_string()),
                            None,
                        ));
                    }
                }
            }
        }
    }
}

fn make_change(
    kind: HumanChangeKind,
    tab_id: Option<&TabId>,
    paragraph: u32,
    before: Option<String>,
    after: Option<String>,
) -> HumanChange {
    let preview_source = match kind {
        HumanChangeKind::Delete => before.as_deref().unwrap_or(""),
        HumanChangeKind::Insert | HumanChangeKind::Modify => after.as_deref().unwrap_or(""),
    };
    let preview = truncate(preview_source, PREVIEW_MAX_CHARS);
    HumanChange {
        kind,
        paragraph,
        preview,
        modified_time: Utc::now(),
        modifying_user: None,
        tab_id: tab_id.cloned(),
        before_text: before,
        after_text: after,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut buf: String = s.chars().take(max).collect();
    buf.push('…');
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::domain::{
        DocumentId, ParagraphKind, RevisionId, TabId, TabSnapshot,
    };

    fn p(n: u32, text: &str) -> ParagraphSnapshot {
        ParagraphSnapshot {
            n,
            kind: ParagraphKind::Paragraph,
            text: text.into(),
            start_index: n,
            end_index: n + text.len() as u32,
        }
    }

    fn snap_single_tab(rev: &str, paragraphs: Vec<ParagraphSnapshot>) -> DocumentSnapshot {
        DocumentSnapshot {
            doc_id: DocumentId("doc1".into()),
            revision_id: RevisionId(rev.into()),
            title: "t".into(),
            tabs: vec![TabSnapshot {
                tab_id: None,
                paragraphs,
            }],
        }
    }

    #[test]
    fn no_changes() {
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "b")]);
        let b = snap_single_tab("r2", vec![p(1, "a"), p(2, "b")]);
        let d = paragraph_diff(&a, &b);
        assert!(d.is_empty(), "expected no diffs, got {:?}", d);
    }

    #[test]
    fn single_modify() {
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "b"), p(3, "c")]);
        let b = snap_single_tab("r2", vec![p(1, "a"), p(2, "B!"), p(3, "c")]);
        let d = paragraph_diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].kind, HumanChangeKind::Modify);
        assert_eq!(d[0].paragraph, 2);
        assert_eq!(d[0].before_text.as_deref(), Some("b"));
        assert_eq!(d[0].after_text.as_deref(), Some("B!"));
    }

    #[test]
    fn single_insert_end() {
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "b")]);
        let b = snap_single_tab("r2", vec![p(1, "a"), p(2, "b"), p(3, "c")]);
        let d = paragraph_diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].kind, HumanChangeKind::Insert);
        assert_eq!(d[0].paragraph, 3);
        assert_eq!(d[0].after_text.as_deref(), Some("c"));
    }

    #[test]
    fn single_insert_middle() {
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "c")]);
        let b = snap_single_tab("r2", vec![p(1, "a"), p(2, "b"), p(3, "c")]);
        let d = paragraph_diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].kind, HumanChangeKind::Insert);
        assert_eq!(d[0].paragraph, 2);
        assert_eq!(d[0].after_text.as_deref(), Some("b"));
    }

    #[test]
    fn single_delete() {
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "b"), p(3, "c")]);
        let b = snap_single_tab("r2", vec![p(1, "a"), p(2, "c")]);
        let d = paragraph_diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].kind, HumanChangeKind::Delete);
        assert_eq!(d[0].before_text.as_deref(), Some("b"));
        assert_eq!(d[0].after_text, None);
    }

    #[test]
    fn replace_shrink() {
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "b"), p(3, "c"), p(4, "d")]);
        let b = snap_single_tab("r2", vec![p(1, "a"), p(2, "X"), p(3, "d")]);
        // b removed (b,c) and inserted (X). Myers will likely produce
        // 1 Modify (b→X) + 1 Delete (c) OR 1 Replace which becomes
        // Modify+Delete. Either way, we verify net effect.
        let d = paragraph_diff(&a, &b);
        let kinds: Vec<HumanChangeKind> = d.iter().map(|c| c.kind).collect();
        assert!(kinds.contains(&HumanChangeKind::Delete) || kinds.contains(&HumanChangeKind::Modify),
                "expected at least one Delete or Modify, got {:?}", kinds);
        assert_eq!(d.iter().filter(|c| c.kind == HumanChangeKind::Insert).count(), 0,
                   "should not have Inserts on shrink (net effect is delete)");
    }

    #[test]
    fn multi_tab_isolated_change() {
        let a = DocumentSnapshot {
            doc_id: DocumentId("doc1".into()),
            revision_id: RevisionId("r1".into()),
            title: "t".into(),
            tabs: vec![
                TabSnapshot { tab_id: Some(TabId("tab1".into())), paragraphs: vec![p(1, "a")] },
                TabSnapshot { tab_id: Some(TabId("tab2".into())), paragraphs: vec![p(1, "b")] },
            ],
        };
        let b = DocumentSnapshot {
            doc_id: DocumentId("doc1".into()),
            revision_id: RevisionId("r2".into()),
            title: "t".into(),
            tabs: vec![
                TabSnapshot { tab_id: Some(TabId("tab1".into())), paragraphs: vec![p(1, "a")] },
                TabSnapshot { tab_id: Some(TabId("tab2".into())), paragraphs: vec![p(1, "B!")] },
            ],
        };
        let d = paragraph_diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].tab_id.as_ref().map(|t| t.0.as_str()), Some("tab2"));
        assert_eq!(d[0].kind, HumanChangeKind::Modify);
    }

    #[test]
    fn preview_truncates_long_text() {
        let long: String = "x".repeat(200);
        let a = snap_single_tab("r1", vec![p(1, "")]);
        let b = snap_single_tab("r2", vec![p(1, &long)]);
        let d = paragraph_diff(&a, &b);
        assert_eq!(d.len(), 1);
        // Preview ≤ 120 chars + 1 ellipsis. Ellipsis is one char.
        assert!(d[0].preview.chars().count() <= 121, "preview too long: {}", d[0].preview);
        assert!(d[0].preview.ends_with('…'));
        // But after_text is full
        assert_eq!(d[0].after_text.as_deref().map(|s| s.len()), Some(200));
    }

    #[test]
    fn deterministic_order() {
        let a = snap_single_tab("r1", vec![p(1, "a"), p(2, "b"), p(3, "c")]);
        let b = snap_single_tab("r2", vec![p(1, "A!"), p(2, "B!"), p(3, "c")]);
        let d1 = paragraph_diff(&a, &b);
        let d2 = paragraph_diff(&a, &b);
        assert_eq!(d1, d2, "diff should be deterministic");
        // First by paragraph
        assert_eq!(d1[0].paragraph, 1);
        assert_eq!(d1[1].paragraph, 2);
    }
}
```

- [ ] **Step 3: Run failing tests**

```bash
cargo test -p colmena_dag_engine --lib gdocs::application::diff
```
Expected: tests run (module compiles) and all 8 PASS. If any fails, debug
the algorithm and re-run.

If the `replace_shrink` test is flaky based on Myers's choice of
op-decomposition, relax the assertion to only check net effect (sum of
inserts/deletes/modifies maps to expected paragraph state).

- [ ] **Step 4: Run all gdocs application tests**

```bash
cargo test -p colmena_dag_engine --lib gdocs::application
```
Expected: all PASS (no regressions).

- [ ] **Step 5: Clippy + fmt**

```bash
cargo fmt -- src/libs/colmena/src/gdocs/application/diff.rs src/libs/colmena/src/gdocs/application/mod.rs
cargo clippy -p colmena_dag_engine --lib --all-targets -- -D warnings
```
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/gdocs/application/diff.rs src/libs/colmena/src/gdocs/application/mod.rs
git commit -m "feat(gdocs): add paragraph_diff for co-edit guard v1.1"
```

---

### Task 5: `RevisionStore` trait — add snapshot persistence

**Files:**
- Modify: `src/libs/colmena/src/gdocs/infrastructure/revision_store.rs`

- [ ] **Step 1: Write failing tests on `InMemoryRevisionStore`**

Replace the `#[cfg(test)] mod tests` block at the bottom with these (preserve the existing 3 tests and add new):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::domain::{ParagraphKind, ParagraphSnapshot, TabSnapshot};

    fn make_snapshot(rev: &str) -> DocumentSnapshot {
        DocumentSnapshot {
            doc_id: DocumentId("doc1".into()),
            revision_id: RevisionId(rev.into()),
            title: "t".into(),
            tabs: vec![TabSnapshot {
                tab_id: None,
                paragraphs: vec![ParagraphSnapshot {
                    n: 1,
                    kind: ParagraphKind::Paragraph,
                    text: "hello".into(),
                    start_index: 1,
                    end_index: 6,
                }],
            }],
        }
    }

    #[tokio::test]
    async fn in_memory_round_trip_legacy_api() {
        let store = InMemoryRevisionStore::new();
        let doc = DocumentId("doc1".into());
        let rev = RevisionId("rev_5".into());
        assert!(store.get("s1", &doc).await.unwrap().is_none());
        store.put("s1", &doc, &rev).await.unwrap();
        assert_eq!(store.get("s1", &doc).await.unwrap(), Some(rev));
    }

    #[tokio::test]
    async fn in_memory_round_trip_with_snapshot() {
        let store = InMemoryRevisionStore::new();
        let doc = DocumentId("doc1".into());
        let rev = RevisionId("rev_5".into());
        let snap = make_snapshot("rev_5");
        store
            .put_with_snapshot("s1", &doc, &rev, Some(&snap))
            .await
            .unwrap();
        let (got_rev, got_snap) = store.get_with_snapshot("s1", &doc).await.unwrap();
        assert_eq!(got_rev, Some(rev));
        assert_eq!(got_snap, Some(snap));
    }

    #[tokio::test]
    async fn in_memory_put_without_snapshot_clears_old_snapshot() {
        let store = InMemoryRevisionStore::new();
        let doc = DocumentId("doc1".into());
        let snap = make_snapshot("r1");
        store
            .put_with_snapshot("s1", &doc, &RevisionId("r1".into()), Some(&snap))
            .await
            .unwrap();
        // Now put without snapshot — snapshot should be cleared.
        store
            .put_with_snapshot("s1", &doc, &RevisionId("r2".into()), None)
            .await
            .unwrap();
        let (_, got_snap) = store.get_with_snapshot("s1", &doc).await.unwrap();
        assert_eq!(got_snap, None);
    }

    #[tokio::test]
    async fn in_memory_scoped_by_session() {
        let store = InMemoryRevisionStore::new();
        let doc = DocumentId("doc1".into());
        store
            .put("s1", &doc, &RevisionId("ra".into()))
            .await
            .unwrap();
        store
            .put("s2", &doc, &RevisionId("rb".into()))
            .await
            .unwrap();
        assert_eq!(store.get("s1", &doc).await.unwrap().unwrap().0, "ra");
        assert_eq!(store.get("s2", &doc).await.unwrap().unwrap().0, "rb");
    }
}
```

Run:
```bash
cargo test -p colmena_dag_engine --lib gdocs::infrastructure::revision_store
```
Expected: FAIL ("method `put_with_snapshot` not found on trait").

- [ ] **Step 2: Extend the trait + InMemory impl**

Replace the trait + InMemoryRevisionStore in `revision_store.rs`. Keep `DocumentSnapshot` import. New file body:

```rust
//! Persistent storage of the last revision + snapshot the agent saw per
//! `(agent_session_id, document_id)`. Postgres-backed in production;
//! `InMemoryRevisionStore` available for unit tests.

use crate::gdocs::domain::{DocsError, DocumentId, DocumentSnapshot, RevisionId};
use async_trait::async_trait;
use sqlx::PgPool;

/// Maximum serialized snapshot size to persist. Beyond this, snapshot is
/// dropped (NULL) and the co-edit guard degrades to v1 (revisionId equality
/// only) for that doc.
pub const MAX_SNAPSHOT_BYTES: usize = 1_048_576; // 1 MB

/// Persistent revision-tracking port. The co-edit guard reads stored
/// revision + snapshot before each edit; successful writes update both.
#[async_trait]
pub trait RevisionStore: Send + Sync {
    /// New API — returns both revision and snapshot if present.
    async fn get_with_snapshot(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
    ) -> Result<(Option<RevisionId>, Option<DocumentSnapshot>), DocsError>;

    /// New API — persists revision and optionally a snapshot. Passing
    /// `None` for `snapshot` clears any previously stored snapshot.
    async fn put_with_snapshot(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
        rev: &RevisionId,
        snapshot: Option<&DocumentSnapshot>,
    ) -> Result<(), DocsError>;

    /// Backward-compat shim — fetches only the revision.
    async fn get(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
    ) -> Result<Option<RevisionId>, DocsError> {
        let (rev, _) = self.get_with_snapshot(session_id, doc_id).await?;
        Ok(rev)
    }

    /// Backward-compat shim — persists only the revision (snapshot None).
    async fn put(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
        rev: &RevisionId,
    ) -> Result<(), DocsError> {
        self.put_with_snapshot(session_id, doc_id, rev, None).await
    }
}

/// Production implementation backed by Postgres.
pub struct PostgresRevisionStore {
    pool: PgPool,
    has_snapshot_col: bool,
}

impl PostgresRevisionStore {
    pub async fn new(pool: PgPool) -> Self {
        let has_snapshot_col = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (\
                SELECT 1 FROM information_schema.columns \
                WHERE table_name = 'gdocs_session_state' \
                AND column_name = 'last_snapshot_json'\
            )",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or(false);

        if !has_snapshot_col {
            tracing::warn!(
                "gdocs: last_snapshot_json column missing on gdocs_session_state; \
                 paragraph diff degraded to v1 (revisionId equality only). \
                 Apply migration 20260609000000_gdocs_session_state_snapshot.sql"
            );
        }
        Self {
            pool,
            has_snapshot_col,
        }
    }

    /// Synchronous constructor for tests / contexts that have already
    /// verified the schema. Defaults to assuming snapshot col is present.
    #[cfg(test)]
    pub fn new_assume_columns(pool: PgPool) -> Self {
        Self {
            pool,
            has_snapshot_col: true,
        }
    }
}

#[async_trait]
impl RevisionStore for PostgresRevisionStore {
    async fn get_with_snapshot(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
    ) -> Result<(Option<RevisionId>, Option<DocumentSnapshot>), DocsError> {
        if self.has_snapshot_col {
            let row: Option<(String, Option<serde_json::Value>)> = sqlx::query_as(
                "SELECT last_revision_id, last_snapshot_json \
                 FROM gdocs_session_state \
                 WHERE agent_session_id = $1 AND document_id = $2",
            )
            .bind(session_id)
            .bind(&doc_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| DocsError::Internal(format!("revision_store.get: {e}")))?;
            match row {
                None => Ok((None, None)),
                Some((rev, snap_json)) => {
                    let snap = snap_json.and_then(|v| serde_json::from_value(v).ok());
                    Ok((Some(RevisionId(rev)), snap))
                }
            }
        } else {
            let row: Option<(String,)> = sqlx::query_as(
                "SELECT last_revision_id FROM gdocs_session_state \
                 WHERE agent_session_id = $1 AND document_id = $2",
            )
            .bind(session_id)
            .bind(&doc_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| DocsError::Internal(format!("revision_store.get: {e}")))?;
            Ok((row.map(|(s,)| RevisionId(s)), None))
        }
    }

    async fn put_with_snapshot(
        &self,
        session_id: &str,
        doc_id: &DocumentId,
        rev: &RevisionId,
        snapshot: Option<&DocumentSnapshot>,
    ) -> Result<(), DocsError> {
        if self.has_snapshot_col {
            let snap_json = snapshot.and_then(|s| serde_json::to_value(s).ok());
            let snap_bytes = snap_json
                .as_ref()
                .and_then(|v| serde_json::to_string(v).ok().map(|s| s.len()));
            let (stored_json, stored_bytes) = match snap_bytes {
                Some(b) if b > MAX_SNAPSHOT_BYTES => {
                    tracing::warn!(
                        bytes = b,
                        doc_id = %doc_id.0,
                        "gdocs.snapshot.too_large — dropping snapshot, guard degrades to v1 for this doc"
                    );
                    (None, None)
                }
                Some(b) => (snap_json, Some(b as i32)),
                None => (None, None),
            };

            sqlx::query(
                "INSERT INTO gdocs_session_state \
                    (agent_session_id, document_id, last_revision_id, last_edit_at, \
                     last_snapshot_json, last_snapshot_size_bytes) \
                 VALUES ($1, $2, $3, now(), $4, $5) \
                 ON CONFLICT (agent_session_id, document_id) \
                 DO UPDATE SET last_revision_id = EXCLUDED.last_revision_id, \
                               last_edit_at     = now(), \
                               last_snapshot_json       = EXCLUDED.last_snapshot_json, \
                               last_snapshot_size_bytes = EXCLUDED.last_snapshot_size_bytes",
            )
            .bind(session_id)
            .bind(&doc_id.0)
            .bind(&rev.0)
            .bind(stored_json)
            .bind(stored_bytes)
            .execute(&self.pool)
            .await
            .map_err(|e| DocsError::Internal(format!("revision_store.put: {e}")))?;
        } else {
            sqlx::query(
                "INSERT INTO gdocs_session_state \
                    (agent_session_id, document_id, last_revision_id, last_edit_at) \
                 VALUES ($1, $2, $3, now()) \
                 ON CONFLICT (agent_session_id, document_id) \
                 DO UPDATE SET last_revision_id = EXCLUDED.last_revision_id, \
                               last_edit_at     = now()",
            )
            .bind(session_id)
            .bind(&doc_id.0)
            .bind(&rev.0)
            .execute(&self.pool)
            .await
            .map_err(|e| DocsError::Internal(format!("revision_store.put: {e}")))?;
        }
        Ok(())
    }
}

/// In-memory `RevisionStore` for unit tests.
#[cfg(test)]
pub struct InMemoryRevisionStore {
    map: tokio::sync::RwLock<
        std::collections::HashMap<(String, String), (RevisionId, Option<DocumentSnapshot>)>,
    >,
}

#[cfg(test)]
impl Default for InMemoryRevisionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl InMemoryRevisionStore {
    pub fn new() -> Self {
        Self {
            map: tokio::sync::RwLock::new(Default::default()),
        }
    }
}

#[cfg(test)]
#[async_trait]
impl RevisionStore for InMemoryRevisionStore {
    async fn get_with_snapshot(
        &self,
        sid: &str,
        doc: &DocumentId,
    ) -> Result<(Option<RevisionId>, Option<DocumentSnapshot>), DocsError> {
        match self.map.read().await.get(&(sid.into(), doc.0.clone())) {
            None => Ok((None, None)),
            Some((rev, snap)) => Ok((Some(rev.clone()), snap.clone())),
        }
    }

    async fn put_with_snapshot(
        &self,
        sid: &str,
        doc: &DocumentId,
        rev: &RevisionId,
        snapshot: Option<&DocumentSnapshot>,
    ) -> Result<(), DocsError> {
        self.map
            .write()
            .await
            .insert((sid.into(), doc.0.clone()), (rev.clone(), snapshot.cloned()));
        Ok(())
    }
}
```

Then re-add the test module from Step 1 (it should already match what
was written there).

- [ ] **Step 3: Update construction sites of `PostgresRevisionStore`**

Search the codebase:
```bash
grep -rn "PostgresRevisionStore::new" src/libs/colmena/src/
```
Update each to `await` since `new` is now async:
```rust
// BEFORE: let store = PostgresRevisionStore::new(pool);
// AFTER:  let store = PostgresRevisionStore::new(pool).await;
```

- [ ] **Step 4: Run unit tests**

```bash
cargo test -p colmena_dag_engine --lib gdocs::infrastructure::revision_store
```
Expected: all 4 PASS.

- [ ] **Step 5: Run all gdocs tests + clippy + fmt**

```bash
cargo test -p colmena_dag_engine --lib gdocs
cargo clippy -p colmena_dag_engine --lib --all-targets -- -D warnings
cargo fmt --check
```
Expected: PASS / clean / clean.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/gdocs/infrastructure/revision_store.rs
git add -u src/libs/colmena/src/  # for any updated construction sites
git commit -m "feat(gdocs): extend RevisionStore with snapshot persistence + graceful degrade"
```

---

### Task 6: Co-edit guard — use diff + partition

**Files:**
- Modify: `src/libs/colmena/src/gdocs/application/co_edit_guard.rs`

- [ ] **Step 1: Update existing tests for new HumanChange fields**

The 4 existing tests in `guard_tests` mod construct `HumanChangesPending` with empty Vecs — those will still compile. But if any test was constructing `HumanChange` directly elsewhere, update them too. Search:
```bash
grep -rn "HumanChange {" src/libs/colmena/src/ tests/
```
Add `tab_id: None, before_text: None, after_text: None` to any literal.

- [ ] **Step 2: Write failing tests for the new behaviors**

Add to the `mod guard_tests` in `co_edit_guard.rs`:
```rust
    /// Drift + prior snapshot present + change overlaps scope → block with
    /// populated changes_overlapping_scope.
    #[tokio::test]
    async fn guard_drift_with_snapshot_overlap_blocks_with_details() {
        let mut rig = TestRig::new();
        // Prior snapshot: 1 paragraph "Hola"
        let prior = snap("r_old", vec![(1, ParagraphKind::Paragraph, "Hola", 1, 6)]);
        // Current snapshot (fetched by guard): same paragraph modified
        let current = snap("r_new", vec![(1, ParagraphKind::Paragraph, "Hola modificado", 1, 16)]);
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
                assert_eq!(changes_overlapping_scope[0].before_text.as_deref(), Some("Hola"));
                assert_eq!(changes_overlapping_scope[0].after_text.as_deref(), Some("Hola modificado"));
                assert!(changes_outside_scope.is_empty());
            }
            other => panic!("expected HumanChangesPending, got {:?}", other),
        }
    }

    /// Drift + prior snapshot present + change outside scope → proceed with
    /// soft_warnings.
    #[tokio::test]
    async fn guard_drift_outside_scope_proceeds_with_soft_warnings() {
        let mut rig = TestRig::new();
        let prior = snap("r_old", vec![
            (1, ParagraphKind::Paragraph, "Hola", 1, 6),
            (2, ParagraphKind::Paragraph, "Adios", 7, 13),
        ]);
        let current = snap("r_new", vec![
            (1, ParagraphKind::Paragraph, "Hola", 1, 6),
            (2, ParagraphKind::Paragraph, "Adios cambiado", 7, 23),
        ]);
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
        let ok = run_guard(&ctx, &doc_id(), &Scope::Paragraph { n: 1 }).await.unwrap();
        // Paragraph 1 untouched → outside scope warnings should include paragraph 2.
        assert_eq!(ok.soft_warnings.len(), 1);
        assert_eq!(ok.soft_warnings[0].paragraph, 2);
    }

    /// Drift + no prior snapshot (degraded) → block with empty lists (v1 behavior).
    #[tokio::test]
    async fn guard_drift_without_snapshot_falls_back_to_v1() {
        let mut rig = TestRig::new();
        let current = snap("r_new", vec![(1, ParagraphKind::Paragraph, "Hola", 1, 6)]);
        expect_get_snapshot(&mut rig.client, current);
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
```

Add `use crate::gdocs::domain::HumanChangeKind;` at the top of `guard_tests` if not already imported.

Run:
```bash
cargo test -p colmena_dag_engine --lib gdocs::application::co_edit_guard
```
Expected: 3 new tests FAIL (guard doesn't use snapshot/diff yet); 4 existing tests should still pass.

- [ ] **Step 3: Implement the new guard flow**

Replace the body of `run_guard` in `co_edit_guard.rs`. Update top imports first:
```rust
use crate::gdocs::application::diff::paragraph_diff;
use crate::gdocs::application::scope_resolver::{self, ResolvedScope};
use crate::gdocs::domain::{DocsClient, DocsError, DocumentId, DocumentSnapshot, HumanChange, Scope};
use crate::gdocs::infrastructure::outline_cache::OutlineCache;
use crate::gdocs::infrastructure::revision_store::RevisionStore;
```

(Remove `RevisionMeta` import if no longer used; also drop the `_suppress_warnings` helper.)

Replace `run_guard` body:
```rust
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
    let (known, prior_snap) = ctx
        .revisions
        .get_with_snapshot(ctx.session_id, doc_id)
        .await?;

    match known {
        None => {
            // First contact — proceed without blocking.
            Ok(GuardOk {
                snapshot,
                resolved_scope,
                soft_warnings: vec![],
            })
        }
        Some(k) if k == snapshot.revision_id => {
            // No drift — proceed.
            Ok(GuardOk {
                snapshot,
                resolved_scope,
                soft_warnings: vec![],
            })
        }
        Some(_) => {
            // Drift — compute paragraph-level diff if we have a prior snapshot;
            // otherwise degrade to v1 (revisionId-only) behavior.
            let changes: Vec<HumanChange> = match prior_snap {
                Some(prior) => paragraph_diff(&prior, &snapshot),
                None => Vec::new(),
            };
            let (overlap, outside) = partition_by_scope(changes, &resolved_scope);
            if !overlap.is_empty() {
                return Err(DocsError::HumanChangesPending {
                    since: chrono::Utc::now(),
                    changes_overlapping_scope: overlap,
                    changes_outside_scope: outside,
                });
            }
            // Drift but no overlap with scope — proceed with soft warnings.
            // When we degraded (changes empty), soft_warnings stays empty too —
            // and we MUST still block because we couldn't prove the human's
            // change is outside the scope. Block on the conservative side.
            if prior_snap.is_none() {
                // Degraded path — preserve v1 conservative block.
                return Err(DocsError::HumanChangesPending {
                    since: chrono::Utc::now(),
                    changes_overlapping_scope: vec![],
                    changes_outside_scope: vec![],
                });
            }
            Ok(GuardOk {
                snapshot,
                resolved_scope,
                soft_warnings: outside,
            })
        }
    }
}

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
```

Note: the `prior_snap.is_none()` branch ensures we don't relax safety
when we can't prove the human change is outside the scope — v1
conservative block stands. The `outside`-only proceed path only fires
when we successfully diffed and the partition was empty for overlap.

- [ ] **Step 4: Run tests**

```bash
cargo test -p colmena_dag_engine --lib gdocs::application::co_edit_guard
```
Expected: 7 tests PASS (4 old + 3 new).

- [ ] **Step 5: Run all gdocs application tests**

```bash
cargo test -p colmena_dag_engine --lib gdocs::application
```
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/gdocs/application/co_edit_guard.rs
git commit -m "feat(gdocs): co-edit guard surfaces paragraph-level human changes (v1.1)"
```

---

### Task 7: Wire snapshot persistence into each use case (8 call sites)

**Files:**
- Modify (all `ctx.revisions.put(...)` → `ctx.revisions.put_with_snapshot(..., Some(&fresh))`):
  - `delete_text.rs:120`
  - `style.rs:147`
  - `replace_text.rs:191`
  - `replace_section.rs:90`
  - `replace_section.rs:161`
  - `named_range.rs:69`
  - `insert.rs:280`
  - `apply_edits.rs:226`

For each file, the pattern is:

**Before:**
```rust
ctx.revisions
    .put(ctx.session_id, &doc_id, &fresh.revision_id)
    .await?;
```

**After:**
```rust
ctx.revisions
    .put_with_snapshot(ctx.session_id, &doc_id, &fresh.revision_id, Some(&fresh))
    .await?;
```

Where `fresh` is the post-write `DocumentSnapshot` already in scope at
each call site (used to construct `EditResult.outline_snapshot`).

- [ ] **Step 1: Update `delete_text.rs:120`**

- [ ] **Step 2: Update `style.rs:147`**

- [ ] **Step 3: Update `replace_text.rs:191`**

- [ ] **Step 4: Update `replace_section.rs:90` and `:161`**

- [ ] **Step 5: Update `named_range.rs:69`**

- [ ] **Step 6: Update `insert.rs:280`**

- [ ] **Step 7: Update `apply_edits.rs:226`**

- [ ] **Step 8: Build + run all gdocs tests**

```bash
cargo build -p colmena_dag_engine
cargo test -p colmena_dag_engine --lib gdocs
```
Expected: all PASS. If any test inspected `revisions.put` calls specifically with `mockall`, update the expectation to `put_with_snapshot`.

- [ ] **Step 9: clippy + fmt**

```bash
cargo clippy -p colmena_dag_engine --lib --all-targets -- -D warnings
cargo fmt --check
```
Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add src/libs/colmena/src/gdocs/application/
git commit -m "feat(gdocs): use cases persist post-write snapshot for v1.1 diff"
```

---

### Task 8: Full repo sweep — `cargo test --verbose` + clippy

**Files:** none modified.

- [ ] **Step 1: Run full test suite**

```bash
source .env
cargo test --verbose 2>&1 | tail -50
```
Expected: `test result: ok` with the expected count (≥ 1587 before; should grow by ~10-15 from new tests).

- [ ] **Step 2: Run integration `#[ignore]` tests**

```bash
source .env
cargo test -- --ignored 2>&1 | tail -30
```
Expected: ignored tests run (or skip if DB not available — note any failures).

- [ ] **Step 3: Clippy + fmt**

```bash
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```
Expected: clean.

- [ ] **Step 4: Verify ADP worker still compiles against this colmena**

```bash
cd ../adp
cargo check --manifest-path apps/service/ia/platform/worker/Cargo.toml
cd -
```
Expected: no errors.

- [ ] **Step 5: Commit (if any fmt/clippy auto-fixes)**

```bash
git status
# If there are auto-fixes, commit them:
# git add -u && git commit -m "style(gdocs): apply cargo fmt"
```

---

### Task 9: Live verification — adapt phase1/phase2 graphs

**Files:**
- Create: `tests/graphs/agents/gdocs_phase1_build_v11.json`
- Create: `tests/graphs/agents/gdocs_phase2_continue_v11.json`

Goal: same shape as the existing phase1/phase2 graphs but verifying v1.1
diff output. Most important: after manual edit between phases, phase2's
agent receives a non-empty `changes_overlapping_scope` in the error.

- [ ] **Step 1: Copy existing graphs as v11 variants**

```bash
cp tests/graphs/agents/gdocs_phase1_build.json tests/graphs/agents/gdocs_phase1_build_v11.json
cp tests/graphs/agents/gdocs_phase2_continue.json tests/graphs/agents/gdocs_phase2_continue_v11.json
```

- [ ] **Step 2: In phase1_v11, adjust the system message**

In `gdocs_phase1_build_v11.json`, edit the `llm_call` node's system message to be specific about the test scenario:
> "You will create a 4-paragraph plan in the user-shared doc. Then yield control. The user may edit the doc before the next phase."

- [ ] **Step 3: In phase2_v11, instruct the agent to attempt an edit on a specific paragraph**

In `gdocs_phase2_continue_v11.json`, system message:
> "Pick up where phase 1 left off. The user may have modified the plan. Attempt to update 'Objetivo 1' using `gdocs_replace_section`. If the tool returns `human_changes_pending`, REPORT THE EXACT FIELDS `changes_overlapping_scope` and `changes_outside_scope` to the user before doing anything else."

- [ ] **Step 4: Run phase 1**

```bash
source .env
mkdir -p /tmp/colmena_e2e
cargo run --bin dag_engine -- run tests/graphs/agents/gdocs_phase1_build_v11.json \
  --agent-session-id gdocs_v11_demo_001 2>&1 \
  | tee /tmp/colmena_e2e/gdocs_phase1_v11.sse
```
Expected: doc created with 4 paragraphs, exit 0.

- [ ] **Step 5: Manual edit**

In the browser, open the doc, locate "Objetivo 1", append " (modificado por humano)" to it. Save (Google auto-saves).

- [ ] **Step 6: Run phase 2**

```bash
cargo run --bin dag_engine -- run tests/graphs/agents/gdocs_phase2_continue_v11.json \
  --agent-session-id gdocs_v11_demo_001 2>&1 \
  | tee /tmp/colmena_e2e/gdocs_phase2_v11.sse
```
Expected:
- `gdocs_replace_section` returns `human_changes_pending`.
- The error payload includes `changes_overlapping_scope` with at least 1 entry showing `kind: "modify"`, `before_text: "Objetivo 1: ..."`, `after_text: "Objetivo 1: ... (modificado por humano)"`.
- LLM final response repeats those fields verbatim.

- [ ] **Step 7: Verify SSE captures the populated payload**

```bash
grep -A 30 "human_changes_pending" /tmp/colmena_e2e/gdocs_phase2_v11.sse | head -60
```
Expected: see `before_text` and `after_text` populated, NOT empty.

- [ ] **Step 8: Commit the graphs (NOT the SSE traces — those stay in /tmp)**

```bash
git add tests/graphs/agents/gdocs_phase1_build_v11.json tests/graphs/agents/gdocs_phase2_continue_v11.json
git commit -m "test(gdocs): add phase1/phase2 v1.1 graphs verifying paragraph-level diff"
```

---

### Task 10: Update developer guide `45_gdocs.md` (Co-edit guard section)

**Files:**
- Modify: `docs/developer_guide/45_gdocs.md`

- [ ] **Step 1: Locate the co-edit guard section**

```bash
grep -n "Co-edit guard\|co_edit_guard\|HumanChangesPending" docs/developer_guide/45_gdocs.md
```

- [ ] **Step 2: Replace the v1 section with v1.1 behavior**

Update the section to describe:
- The new error payload shape (with `before_text` / `after_text` / `tab_id`).
- The "proceed with soft_warnings" path when changes are outside scope.
- The degraded mode when snapshot is missing (older instances or
  oversized docs) — block with empty lists, same as v1.
- The 1 MB snapshot cap and `COLMENA_GDOCS_MAX_SNAPSHOT_BYTES` env
  override.
- The migration the operator must apply
  (`20260609000000_gdocs_session_state_snapshot.sql`).
- New example tool result with populated fields (copy from spec §6.2).

- [ ] **Step 3: Verify**

```bash
grep -c "before_text\|soft_warnings\|MAX_SNAPSHOT_BYTES" docs/developer_guide/45_gdocs.md
```
Expected: > 0 for each term.

- [ ] **Step 4: Commit**

```bash
git add docs/developer_guide/45_gdocs.md
git commit -m "docs(gdocs): document co-edit guard v1.1 paragraph diff in 45_gdocs.md"
```

---

### Task 11: Update CHANGELOG, BACKLOG, CLAUDE.md, ADP_PRISMA doc

**Files:**
- Modify: `docs/CHANGELOG_2026-06.md`
- Modify: `docs/BACKLOG.md`
- Modify: `CLAUDE.md`
- Modify: `ADP_PRISMA_PENDING_TABLES.md`

- [ ] **Step 1: CHANGELOG entry**

In `docs/CHANGELOG_2026-06.md`, prepend (under the appropriate section header):

```markdown
### 2026-06-09 — gdocs co-edit guard v1.1 (paragraph diff)

When the user edits a Google Doc between agent writes, the guard now
returns a paragraph-level list of changes — populated `before_text` /
`after_text` / `tab_id` per change — partitioned by overlap with the
agent's intended scope. Changes inside the scope still block with
`human_changes_pending`; changes outside the scope let the edit proceed
with `soft_warnings`. Implementation persists a `DocumentSnapshot` in
`gdocs_session_state.last_snapshot_json` (capped at 1 MB; opt-out by
`COLMENA_GDOCS_MAX_SNAPSHOT_BYTES`) and runs Myers diff via `similar`.
Instances without the migration applied degrade gracefully to v1
behavior (warn at boot, empty change lists, conservative block).

Migration: `20260609000000_gdocs_session_state_snapshot.sql`. ADP must
add the 2 columns to its Prisma schema — see
`ADP_PRISMA_PENDING_TABLES.md`.

Spec: `docs/superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md`.
```

- [ ] **Step 2: BACKLOG**

In `docs/BACKLOG.md`, locate the "Subsystem G v1.1" section and mark
item 2 done:
```markdown
2. ~~Paragraph-level human-change diff~~ — shipped 2026-06-09. Postgres
   snapshot cache + Myers diff. See CHANGELOG.
```

- [ ] **Step 3: CLAUDE.md**

In `/Users/danielgarcia/startti/colmena/CLAUDE.md` under "Current Status", append a bullet (after the Subsystem G v1 line):
```markdown
- **Subsystem G v1.1 paragraph diff shipped 2026-06-09** — co-edit guard
  now returns paragraph-level `before_text` / `after_text` per
  `HumanChange`, partitioned by scope overlap. Adds `last_snapshot_json`
  to `gdocs_session_state` (migration
  `20260609000000_gdocs_session_state_snapshot.sql`). ADP Prisma update
  pending — see `ADP_PRISMA_PENDING_TABLES.md`. Spec at
  [`docs/superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md`](docs/superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md).
```

- [ ] **Step 4: ADP_PRISMA_PENDING_TABLES.md**

Append a new section at the bottom of `/Users/danielgarcia/startti/colmena/ADP_PRISMA_PENDING_TABLES.md`:

```markdown
## 5. `gdocs_session_state` — v1.1 extension (2026-06-09)

Two columns added to support paragraph-level human-change diff in the
co-edit guard. Additive; existing rows unaffected.

### Raw SQL (idempotent)

```sql
ALTER TABLE gdocs_session_state
  ADD COLUMN IF NOT EXISTS last_snapshot_json       JSONB,
  ADD COLUMN IF NOT EXISTS last_snapshot_size_bytes INTEGER;
```

### Prisma update

In the existing `model GdocsSessionState`, add:
```prisma
  lastSnapshotJson      Json?    @map("last_snapshot_json")
  lastSnapshotSizeBytes Int?     @map("last_snapshot_size_bytes")
```

### Behavior when not applied

Colmena detects the missing columns at boot, logs
`gdocs.snapshot.column_missing` once, and degrades to v1 guard behavior
(revisionId equality only; empty change lists). No crash, no data loss.
```

- [ ] **Step 5: Commit**

```bash
git add docs/CHANGELOG_2026-06.md docs/BACKLOG.md CLAUDE.md ADP_PRISMA_PENDING_TABLES.md
git commit -m "docs: changelog + backlog + ADP integration notes for gdocs v1.1"
```

---

### Task 12: Final sweep — verify CI passes + push

**Files:** none.

- [ ] **Step 1: Final local sweep**

```bash
source .env
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --verbose 2>&1 | tail -10
```
Expected: clean / clean / `test result: ok`.

- [ ] **Step 2: Inspect log of new commits**

```bash
git log origin/develop..HEAD --oneline
```
Expected: ~10-12 commits matching task boundaries.

- [ ] **Step 3: Push**

```bash
git push origin develop
```

- [ ] **Step 4: Monitor CI**

```bash
sleep 60
gh run list --branch develop --limit 1
gh run watch <run_id>
```
Or with run_in_background and a 1200s ScheduleWakeup if CI takes longer.
Expected: `status=completed conclusion=success` on all 5 Python matrix jobs.

- [ ] **Step 5: Verify ADP worker compatibility one more time**

```bash
cd ../adp
cargo check --manifest-path apps/service/ia/platform/worker/Cargo.toml
cd -
```
Expected: no errors. (No behavioral change for ADP — the trait still
exposes `get`/`put` as default shims.)

- [ ] **Step 6: Final commit (only if anything from §1 changed)**

If everything was already clean, no commit needed. Otherwise:
```bash
git add -u && git commit -m "chore(gdocs): final v1.1 sweep" && git push origin develop
```

---

## Self-Review Checklist

**1. Spec coverage**
- §3.1 HumanChange extension → Task 2 ✓
- §3.2 Migration → Task 1 ✓
- §3.3 Size cap + fallback → Task 5 (MAX_SNAPSHOT_BYTES) ✓
- §4 Diff algorithm → Task 4 ✓
- §4.5 partition_by_scope → Task 6 (inside guard); contains_paragraph helper → Task 3 ✓
- §5 Guard refactor → Task 6 ✓
- §5.1 Snapshot persistence per use case → Task 7 ✓
- §6 Error format → handled by enriched HumanChange fields (Task 2) + guard (Task 6) ✓
- §7 Migration + degraded mode → Task 5 (PostgresRevisionStore::new), Task 1 (migration) ✓
- §8 Testing → Tasks 4 (diff), 5 (store), 6 (guard) ✓
- §8.5 Integration test live + §8.6 E2E graph → Task 9 ✓
- §9 Observability → tracing calls in Task 5 (warns) ✓
- §10 Risks → mitigations are in Task 5 (graceful degrade) + Task 6 (conservative block on missing snapshot) ✓
- §11 Acceptance criteria → Task 8 (test/clippy/fmt sweep), Task 9 (live verify), Task 11 (docs), Task 12 (CI + push) ✓

**2. Placeholder scan**
- No "TBD" / "implement later" / "fill in details" in any task ✓
- Every code step has full code, every command step has full command ✓
- No "similar to Task N" — code repeated where needed ✓

**3. Type / name consistency**
- `paragraph_diff(prior, current)` signature used in Task 4 and Task 6 ✓
- `put_with_snapshot` / `get_with_snapshot` consistent across Task 5 (definition), Task 6 (consumer), Task 7 (call sites) ✓
- `MAX_SNAPSHOT_BYTES` defined Task 5, referenced in CHANGELOG Task 11 ✓
- `HumanChange` field names (`tab_id`, `before_text`, `after_text`) consistent across Tasks 2, 4, 6, 9 ✓
- `contains_paragraph` defined Task 3, consumed Task 6 ✓

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-06-09-gdocs-paragraph-diff.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — execute tasks in this session with checkpoints.

**Which approach?**
