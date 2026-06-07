# Merge `develop` → `feature/docs` — fix-preservation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans
> for sequential execution with verification gates after each fix.

**Goal:** Bring the 19 commits develop has accumulated into `feature/docs`
WITHOUT regressing any of the 3 production fixes that landed in develop while
feature/docs was the integration branch. Each fix has its own preservation
checklist and a test that must pass after the merge.

**Why now:** The PR `feature/docs → develop` cannot open cleanly until both
branches share a common tip. Doing the merge IN feature/docs (instead of
during the PR) means conflict resolution happens in our isolated workspace
and we can iterate without polluting develop's history.

**Critical insight:** The 19 commits include a CHANGELOG strip commit
(`7a7aa76`) that **deletes "confabulated" CRDT content from develop's
worldview**. In feature/docs that content is REAL (we actually implemented
subsystems B/C/D/F). The strip MUST NOT be applied naively or we'd delete
the entries for work that genuinely shipped on our branch.

---

## What's in develop that needs to land here

Three real fixes + one merge that affects our CHANGELOG:

### Fix 1 — Suspend `resume_answer` routing (Approach B)

**Reported:** ADP 2026-06-04. **Spec:** [`docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md`](../specs/2026-06-05-suspend-resume-answer-routing-fix-design.md) (lands as part of the merge).

**Bug it fixes:** when a graph has a `suspend` node followed by a downstream `llm_call`, the resume run injects `__colmena_resume_answer` into EVERY node — including the downstream `llm_call` that had no prior suspend. The `llm_call` then aborts with `"llm_call resume: no pending tool call found in conversation history"`. ADP reported this on 2026-06-04 during HITL testing.

**Code changes (4 commits):**

| SHA | What it adds |
|---|---|
| `8eab740` | New helper `DagRunUseCase::compute_resuming_node_ids(all_outputs, resume_answer) -> HashSet<String>` in `run_use_case.rs`. Walks the persisted snapshot looking for `__colmena_status: "SUSPENDED"` (recursive for orchestrator/subgraph wrap). 4 unit tests in `resuming_node_ids_tests` mod. |
| `af87e4f` | In the main loop of `DagRunUseCase`, compute the set ONCE at start (`let resuming_node_ids = ...`), then gate the `inputs.insert("__colmena_resume_answer", ...)` line with `if resuming_node_ids.contains(&node_id)`. |
| `f9f7242` | Failing-repro test that asserts the bug existed pre-fix. |
| `f674204` | Defensive fallthrough in `llm.rs::ExecutableNode for LlmNode`: when `resume_answer` is set but `find_pending_tool_call` returns `None`, fall through to the fresh-run path with a `warn!` log instead of `.ok_or(...)?`. Belt-and-suspenders complement to the engine fix. |
| `14d466e` | Integration test for the cascade variant (suspend → suspend → resume). |

**Files touched by develop:**
- `src/libs/colmena/src/dag_engine/application/run_use_case.rs` (+28 / -1 in `af87e4f`, +75 helper + tests in `8eab740`)
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (+30 / -10 in `f674204`)
- `src/libs/colmena/tests/suspend_resume_routing.rs` (NEW)

**Files we (feature/docs) ALSO touched:**
- `llm.rs` — multiple times, most recently T5 added a `pub on_existing_sheet: Option<String>` to `GsheetsRunPythonArgs` (DIFFERENT struct from `LlmNode`; both live in same file). Auto-merge LIKELY clean because the regions are far apart.
- `run_use_case.rs` — not touched in P1+P2 specifically, but earlier subsystems (B-T8, B-T13 added `session_id` threading) may have edited the surrounding area.

**Tests that MUST pass post-merge (preservation gates):**

```bash
cargo test --lib -p colmena_dag_engine resuming_node_ids_tests 2>&1 | tail -8
# Expected: 4 passed
```

```bash
cargo test --test suspend_resume_routing -p colmena_dag_engine 2>&1 | tail -8
# Expected: N passed (look at the file to see how many; ~3-4 tests)
```

```bash
cargo run --release --bin dag_engine -- run tests/graphs/basic/suspend_then_llm_resume.json \
  --agent-session-id merge_check_$(date +%s) > /tmp/colmena_merge/suspend_then_llm_resume.sse 2>&1
# Expected: graph reaches suspend, persists state, exits cleanly
# (Full resume test is in the unit test suite — the graph just verifies the engine doesn't panic.)
```

If any of these fail, the merge DID regress the fix. STOP and inspect the merge of `run_use_case.rs` or `llm.rs` by hand.

### Fix 2 — Gemini non-object tool response wrapping

**Spec:** [`docs/superpowers/plans/2026-06-01-gemini-scalar-tool-response-fix.md`](2026-06-01-gemini-scalar-tool-response-fix.md) (lands as part of the merge).

**Bug it fixes:** Gemini's `functionResponse.response` field is typed as `google.protobuf.Struct` and only accepts JSON objects. The previous adapter only wrapped tool content in `{result: ...}` when JSON parse failed. If a tool returned a valid JSON scalar (`output = 5040`, `output = [1,2,3]`, `output = true`, `output = null`), Gemini's API silently rejected it with `400 INVALID_ARGUMENT` — the agent died after one turn with empty result + 0 completion tokens + NO error visible in SSE.

**Code changes (2 commits):**

| SHA | What it adds |
|---|---|
| `b6412a6` | Regression tests in `gemini_adapter.rs` covering scalar number, scalar string, array, bool, null. |
| `d99e975` | In `gemini_adapter.rs::GeminiAdapter::adapt_messages`, change `unwrap_or_else(|_| json!({result: ...}))` to `match { Ok(v) if v.is_object() => v, Ok(v) => json!({result: v}), Err(_) => json!({result: <string>}) }`. Objects pass through unchanged. |

**Files touched by develop:**
- `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs` (+30 / -6)
- `CLAUDE.md` (1 line added to status section)

**Files we (feature/docs) ALSO touched:**
- `gemini_adapter.rs` — **probably none of our subsystem work touched this**. Verify with grep before assuming auto-merge.
- `CLAUDE.md` — both branches added status entries; will auto-merge (different sections of the file).

**Tests that MUST pass post-merge:**

```bash
cargo test --lib -p colmena_dag_engine gemini_adapter::tests 2>&1 | tail -10
# Expected: all gemini_adapter tests pass (includes the 4-5 new regression tests for scalar/array/bool/null)
```

Spot-check the fix didn't get reverted:

```bash
grep -A5 "Ok(v) if v.is_object()" src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs
# Expected: the match arm visible (not just a single .unwrap_or_else)
```

### Fix 3 — HITL email approval demo graph

**Code change (1 commit):**

| SHA | What it adds |
|---|---|
| `772c9f3` | New graph `tests/graphs/basic/suspend_email_approval_demo.json` demonstrating `suspend` + `router` for a HITL approval workflow. |

**Risk:** zero — additive only, no conflicts possible.

**Verification:**

```bash
ls tests/graphs/basic/suspend_email_approval_demo.json
# Expected: file exists after merge
python3 -c "import json; json.load(open('tests/graphs/basic/suspend_email_approval_demo.json')); print('OK')"
# Expected: OK
```

### CHANGELOG strip (`7a7aa76`) — needs SPECIAL handling

**Develop's view:** strips "confabulated CRDT content" from `CHANGELOG_2026-06.md` because in develop's worldview the CRDT subsystems never landed.

**Our view:** CRDT subsystems DID land (we have working code, tests, docs). The "confabulated" entries develop deleted are **REAL** in feature/docs.

**Resolution rule:** during the merge, when `git merge` reports the CHANGELOG conflict, **manually accept OUR version of all CRDT-related sections** and **only merge in develop's NEW entries** (the suspend-routing-fix entry + the gemini-scalar entry + the HITL demo entry, if any). Do NOT apply the strip to our content.

---

## Pre-merge state snapshot

| Property | Value |
|---|---|
| `feature/docs` HEAD | `645a066` (the PR plan I just committed) |
| `origin/develop` HEAD | `c5644aa` (last verified `git fetch`) |
| Merge base | `9b0c9f0` (2026-06-01) |
| Commits ours-only | 257 |
| Commits develop-only | 19 |
| Files with both-side modifications | 5 (CLAUDE.md, CHANGELOG, DEVELOPER_GUIDE, node_configurations, llm.rs, run_use_case.rs, gemini_adapter.rs) |
| Files dev-side only (new) | 8 (3 specs/plans + suspend_node guide + 3 test graphs + 1 integration test) |
| Real conflicts (from `git merge --no-commit` dry-run) | 1 (CHANGELOG_2026-06.md) |
| TOC number collision | 1 (`38_suspend_node.md` vs our `38_crdt_documents.md`) |

---

## Task 1: Snapshot + scratch space

**Files:** none (just metadata + safety net)

- [ ] **Step 1: Make sure working tree is clean**

```bash
git status --short
```

Expected: empty.

- [ ] **Step 2: Snapshot current HEAD for rollback**

```bash
mkdir -p /tmp/colmena_merge
git rev-parse HEAD > /tmp/colmena_merge/pre_merge_head.txt
git log -1 --oneline | tee /tmp/colmena_merge/pre_merge_head.txt
```

If anything goes south, recovery is:
```bash
git reset --hard $(cat /tmp/colmena_merge/pre_merge_head.txt | awk '{print $1}')
```

- [ ] **Step 3: Fetch and confirm develop is current**

```bash
git fetch origin develop
git log -1 --oneline origin/develop | tee /tmp/colmena_merge/develop_head.txt
```

Expected: `c5644aa` (or newer if someone pushed in the meantime).

- [ ] **Step 4: Save develop's CHANGELOG for reference during conflict resolution**

```bash
git show origin/develop:docs/CHANGELOG_2026-06.md > /tmp/colmena_merge/develop_changelog.md
git show HEAD:docs/CHANGELOG_2026-06.md > /tmp/colmena_merge/feature_docs_changelog.md
diff /tmp/colmena_merge/feature_docs_changelog.md /tmp/colmena_merge/develop_changelog.md > /tmp/colmena_merge/changelog_diff.txt || true
wc -l /tmp/colmena_merge/changelog_diff.txt
```

Glance at the diff — confirm develop only ADDED entries for suspend/gemini/HITL, plus the strip of stuff that doesn't exist in our tree.

- [ ] **Step 5: Establish baseline test count**

```bash
cargo test --lib -p colmena_dag_engine 2>&1 | grep "test result" | tail -1 | tee /tmp/colmena_merge/baseline_tests.txt
```

Expected: `test result: ok. 1388 passed; 0 failed; 28 ignored; 0 measured; 0 filtered out`. After the merge, the count should be HIGHER (develop adds ~5-10 tests for suspend + gemini).

---

## Task 2: Pre-merge audit of high-risk files

**Files:** none (read-only inspection)

Before letting git auto-merge, manually inspect the regions in `llm.rs` and `run_use_case.rs` to predict what could go wrong.

- [ ] **Step 1: Locate develop's suspend-fix region in `run_use_case.rs`**

```bash
git show origin/develop -- src/libs/colmena/src/dag_engine/application/run_use_case.rs | head -80
```

Note the line range where `let resuming_node_ids: ...` and the gated injection live (around line 253 and line 387 in develop's version).

- [ ] **Step 2: Check our (feature/docs) version of the same file in that region**

```bash
grep -n "resume_answer\|__colmena_resume_answer\|SUSPENDED" src/libs/colmena/src/dag_engine/application/run_use_case.rs | head -20
```

If our copy has any reference to `__colmena_resume_answer` injection that DIFFERS from develop's pre-fix shape, the merge may need manual resolution. If we don't reference it at all → auto-merge will land develop's fix cleanly.

- [ ] **Step 3: Locate develop's fallthrough fix in `llm.rs`**

```bash
git show origin/develop -- src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs | grep -B2 -A10 "find_pending_tool_call" | head -30
```

Note the `match maybe_pending { Some(pending) => { ... } None => { warn! + fall through } }` block.

- [ ] **Step 4: Check our version**

```bash
grep -n "find_pending_tool_call\|no pending tool" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs | head -10
```

If our version still has the old `.ok_or("...no pending tool call...")?` pattern, the auto-merge should apply develop's fix as a clean replacement. If we modified the surrounding code, may need manual review.

- [ ] **Step 5: Verify the gemini_adapter region is OURS-clean**

```bash
git diff origin/develop -- src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs | head -20
git log feature/docs --oneline -- src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs 2>/dev/null | head -5
```

Expected: no recent feature/docs commits touched this file. If true, the gemini fix lands as a pure addition.

- [ ] **Step 6: Decide path forward**

Based on steps 1-5, classify each high-risk file:
- ✅ **Auto-merge safe** — develop's change is isolated; we don't touch the region.
- ⚠️ **Auto-merge with verification** — we touch the file but in a different region.
- 🔴 **Manual merge required** — we modified the same lines develop changed.

Record findings in `/tmp/colmena_merge/audit.md`. If anything is 🔴, plan the manual resolution before starting the merge.

---

## Task 3: Execute the merge

**Files:**
- Modify: `docs/CHANGELOG_2026-06.md` (conflict)
- Possibly modify: `docs/DEVELOPER_GUIDE.md` (TOC), `src/libs/colmena/src/dag_engine/{application/run_use_case.rs, infrastructure/nodes/llm.rs}` (if audit said 🔴)
- Rename: `docs/developer_guide/38_suspend_node.md` → `docs/developer_guide/44_suspend_node.md`

- [ ] **Step 1: Start the merge (will halt on CHANGELOG)**

```bash
git merge origin/develop --no-ff --no-commit
```

Expected output:
```
Auto-merging CLAUDE.md
Auto-merging docs/CHANGELOG_2026-06.md
CONFLICT (add/add): Merge conflict in docs/CHANGELOG_2026-06.md
Auto-merging docs/DEVELOPER_GUIDE.md
Auto-merging docs/node_configurations.json
Auto-merging src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
Automatic merge failed; fix conflicts and then commit the result.
```

If the conflict list is BIGGER than this, STOP — re-run the audit (Task 2) on the new conflicts before resolving.

- [ ] **Step 2: Resolve the CHANGELOG conflict — preserving OUR CRDT content**

Open `docs/CHANGELOG_2026-06.md`. Find the `<<<<<<<` markers. The resolution is:

1. **Keep ALL of our §1-§11** (E-T15 through P1+P2 + the in-doc commit version of §11 which is "sheets-write-safety"). Our content is REAL and must survive.
2. **Insert develop's NEW entries** that are NOT in our file. Specifically the entries about:
   - Gemini scalar tool response fix (shipped 2026-06-01 per develop)
   - Suspend resume_answer routing fix (Approach B, shipped 2026-06-06 per develop)
   - HITL email approval demo graph
3. **Renumber as needed** so the final file is monotonically increasing — likely our sheets-write-safety is the most recent, so it stays as §15 (or whatever the new last position is) after develop's 3 entries are inserted as §12-§14.
4. **DO NOT apply develop's strip** to our CRDT content. The strip commit (`7a7aa76`) deletes CRDT/B/C/D/F entries because in develop's worldview they never landed. In ours they DID.

Cross-reference with the saved copies:
```bash
diff /tmp/colmena_merge/feature_docs_changelog.md /tmp/colmena_merge/develop_changelog.md | less
```

Use the diff to identify exactly what develop ADDED (those are the ones to merge in) vs what develop REMOVED (those are the strip — ignore).

Save when satisfied, then:

```bash
git add docs/CHANGELOG_2026-06.md
```

- [ ] **Step 3: Rename `38_suspend_node.md` → `44_suspend_node.md` to avoid TOC collision**

```bash
git mv docs/developer_guide/38_suspend_node.md docs/developer_guide/44_suspend_node.md
```

- [ ] **Step 4: Update DEVELOPER_GUIDE.md TOC for the renumber**

The auto-merge already inserted develop's TOC entry for "38 — Suspend Node". Find it and:
- Renumber the entry from 38 to 44.
- Update the link from `./developer_guide/38_suspend_node.md` to `./developer_guide/44_suspend_node.md`.

Verify no other doc cross-references the old path:

```bash
grep -rn "38_suspend_node" docs/ src/ 2>/dev/null | grep -v "/target/"
```

Expected: empty after the renumber. Fix any remaining occurrences (likely in cross-links inside the guide itself, or in the CHANGELOG entry for the suspend fix).

```bash
git add docs/DEVELOPER_GUIDE.md docs/developer_guide/44_suspend_node.md
```

- [ ] **Step 5: Verify NO unmerged paths remain**

```bash
git status --short | grep "^UU\|^AA\|^DD"
```

Expected: empty.

- [ ] **Step 6: Inspect the auto-merged `run_use_case.rs`**

This is CRITICAL — the auto-merge may have landed without conflict markers but with broken logic if our edits collided in a way git resolved by picking sides.

```bash
grep -n "compute_resuming_node_ids\|resuming_node_ids\|__colmena_resume_answer" \
  src/libs/colmena/src/dag_engine/application/run_use_case.rs
```

Expected to see (all of these):
- `let resuming_node_ids: std::collections::HashSet<String> = Self::compute_resuming_node_ids(...)` — the snapshot at run start
- `if resuming_node_ids.contains(&node_id) { inputs.insert("__colmena_resume_answer", ...) }` — the gated injection
- `fn compute_resuming_node_ids(...)` definition
- `mod resuming_node_ids_tests` — the 4 unit tests

If any is missing, the merge dropped part of develop's fix. Inspect the file manually and re-apply by hand (use `git show origin/develop:<path>` to see develop's full version).

- [ ] **Step 7: Inspect the auto-merged `llm.rs`**

Confirm the fallthrough fix landed:

```bash
grep -n "find_pending_tool_call\|maybe_pending\|fall through" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs | head -10
```

Expected:
- `let maybe_pending = find_pending_tool_call(...)`
- `if let Some(pending) = maybe_pending { ... }` (the success branch — our existing logic)
- An `else` arm (or `None` arm in a `match`) with a `tracing::warn!` + fall through

If the file still has the old `.ok_or(...)?` pattern, the fallthrough fix was LOST. Re-apply by hand from `git show f674204 -- src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`.

- [ ] **Step 8: Inspect the auto-merged `gemini_adapter.rs`**

```bash
grep -B1 -A5 "Ok(v) if v.is_object()" src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs
```

Expected: the match arms `Ok(v) if v.is_object() => v` / `Ok(v) => json!({result: v})` / `Err(_) => json!({result: ...})` ALL visible. If we see only the old `unwrap_or_else(|_| json!({result: ...}))`, the gemini fix was LOST. Re-apply from `git show d99e975 -- src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs`.

- [ ] **Step 9: Inspect CLAUDE.md auto-merge**

Both branches added "shipped" status entries. They should coexist (different sections). Verify:

```bash
grep -n "Gemini scalar\|gemini.*scalar\|Suspend resume_answer\|Sheets write safety" CLAUDE.md
```

Expected: all four entries present.

- [ ] **Step 10: Inspect docs/node_configurations.json auto-merge**

```bash
python3 -c "import json; json.load(open('docs/node_configurations.json')); print('VALID JSON')"
```

Expected: VALID JSON (auto-merge corruption of JSON is unlikely but worth checking — a stray `<<<<<<<` would tank the parse).

If parse fails, open the file and look for conflict markers, then resolve.

---

## Task 4: Build + test gates (fix-preservation verification)

**Files:** none

Each step here is a HARD GATE — if it fails, the merge is broken and must be repaired before continuing.

- [ ] **Gate A: Cargo build**

```bash
cargo build -p colmena_dag_engine 2>&1 | tail -10
```

Expected: clean build, no warnings. If `denied warning` fires from a use-statement merge mishap, fix by hand.

- [ ] **Gate B: Cargo clippy**

```bash
cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings 2>&1 | tail -10
```

Expected: 0 warnings.

- [ ] **Gate C: Fmt**

```bash
cargo fmt --check 2>&1 | tail -5
```

If `--check` fails, run `cargo fmt` to fix in place, then re-check.

- [ ] **Gate D: Suspend fix preservation tests**

```bash
echo "=== resuming_node_ids unit tests ==="
cargo test --lib -p colmena_dag_engine resuming_node_ids_tests 2>&1 | tail -8
# Expected: 4 passed

echo "=== suspend_resume_routing integration test ==="
cargo test --test suspend_resume_routing -p colmena_dag_engine 2>&1 | tail -10
# Expected: all tests passing
```

If any test FAILS:
- Diagnose with `cargo test --test suspend_resume_routing -p colmena_dag_engine -- --nocapture`
- Inspect the failed assertion against develop's expected behavior.
- The fix WAS in develop and DID pass develop's CI. If it fails here, OUR merge resolution broke it.

- [ ] **Gate E: Gemini fix preservation tests**

```bash
cargo test --lib -p colmena_dag_engine gemini_adapter 2>&1 | tail -10
```

Expected: all gemini_adapter tests pass, including the new scalar/array/bool/null regression tests.

- [ ] **Gate F: Our existing 1388 tests still pass**

```bash
cargo test --lib -p colmena_dag_engine 2>&1 | grep "test result" | tail -1
```

Expected: a number STRICTLY HIGHER than 1388 (we should have gained ~5-10 tests from develop). Same `0 failed`. No regression in our existing tests.

- [ ] **Gate G: HITL demo graph parses**

```bash
python3 -c "import json; json.load(open('tests/graphs/basic/suspend_email_approval_demo.json')); print('OK')"
```

Expected: OK.

- [ ] **Gate H: Visual diff of high-risk files (sanity check)**

```bash
# Compare our merged version against develop's version
diff <(git show origin/develop:src/libs/colmena/src/dag_engine/application/run_use_case.rs) \
     src/libs/colmena/src/dag_engine/application/run_use_case.rs | head -100
```

Look at the diff — develop's content should be a SUBSET of our merged file (our file = develop's content + everything we added in feature/docs). If develop's content has lines that ARE in our file, that's expected. If develop's content has lines NOT in our file, that's a regression.

Same for `llm.rs` and `gemini_adapter.rs`.

---

## Task 5: Commit the merge

**Files:** none

- [ ] **Step 1: Final visual review of all changes about to commit**

```bash
git status --short
git diff --cached --stat | tail -20
```

Expected:
- A merge commit forming with ~50-150 file changes.
- Includes all 19 develop commits being recorded.
- Our own 257 commits stay intact (the merge is `--no-ff`).

- [ ] **Step 2: Commit the merge**

```bash
git commit -m "$(cat <<'EOF'
Merge develop into feature/docs

Brings in 19 commits from develop accumulated since the 2026-06-01 split:

Fixes preserved (verified by post-merge tests):
- Suspend resume_answer routing (Approach B, af87e4f + 8eab740 + f674204):
  __colmena_resume_answer injection now gated by SUSPENDED set
  computed once at run start. New compute_resuming_node_ids helper
  + 4 unit tests. Defensive fallthrough in llm.rs when no pending
  tool call exists. Verified: resuming_node_ids_tests (4/4),
  suspend_resume_routing integration test, suspend graphs parse.
- Gemini non-object tool response (d99e975 + b6412a6): wrap any
  non-object value in {result: ...}. Verified: gemini_adapter tests
  including new scalar/array/bool/null regression cases.
- HITL email approval demo (772c9f3): new graph in tests/graphs/basic/.

Conflict resolution:
- docs/CHANGELOG_2026-06.md: merged manually, preserving ALL of
  feature/docs's CRDT/B/C/D/E/F entries (real work) while inserting
  develop's new suspend/gemini/HITL entries. Did NOT apply develop's
  strip commit (7a7aa76) since the "confabulated" content is real
  in this branch.

Renames:
- docs/developer_guide/38_suspend_node.md → 44_suspend_node.md
  (avoids TOC collision with our 38_crdt_documents.md).

No public API regressions. ADP worker swept clean — both fixes are
ADP-relevant (suspend was reported BY ADP, gemini was wire-format).
All 1388 feature/docs tests still pass; new tests from develop bring
total to ~1395-1400.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3: Verify merge committed**

```bash
git log --oneline -5
# Should show: <new merge SHA> + 645a066 (PR plan) + previous commits
git log --pretty=%P HEAD -1
# Should show TWO parents (merge commit), confirming --no-ff worked
git log --oneline HEAD..origin/develop | wc -l
# Should be 0 — develop is fully merged
```

---

## Task 6: Final verification (everything in one place)

**Files:** none

- [ ] **Step 1: Full test suite (including doctests)**

```bash
cargo test --verbose -p colmena_dag_engine 2>&1 | tee /tmp/colmena_merge/full_test.log | tail -10
```

Expected: ~1395-1400 passed, 0 failed, ~28 ignored.

- [ ] **Step 2: Workspace-wide clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 3: Legacy refs sweep (regression check)**

```bash
grep -rn "write_to_sheet\|output_sheet[^s]" src/ tests/ 2>/dev/null | grep -v "/target/"
```

Expected: empty (P1+P2's legacy removal must still be intact after merge).

- [ ] **Step 4: All preservation gates one more time as a single command**

```bash
cargo test --lib -p colmena_dag_engine "resuming_node_ids_tests::|gemini_adapter::tests::" 2>&1 | tail -10
cargo test --test suspend_resume_routing -p colmena_dag_engine 2>&1 | tail -5
```

Expected: all pass.

- [ ] **Step 5: Confirm we're ready to push**

```bash
git log --oneline -5
git status
```

The merge commit + 645a066 (PR plan) + earlier commits should be visible. `git status` should report `Your branch is ahead of 'origin/feature/docs' by 1 commit`.

- [ ] **Step 6: Push**

```bash
git push origin feature/docs 2>&1 | tail -3
```

Expected: clean push. If rejected because someone else pushed to feature/docs in the meantime (unlikely but possible), pull-rebase first:

```bash
git pull --rebase origin feature/docs
# Re-run Task 4 gates after rebase
git push origin feature/docs
```

---

## Rollback procedures

### If the merge is irrecoverably broken mid-resolution

```bash
git merge --abort
# Working tree restored to pre-merge state
git status   # should be clean
```

### If the merge committed but post-merge tests fail

```bash
git reset --hard $(cat /tmp/colmena_merge/pre_merge_head.txt | awk '{print $1}')
# This destroys the merge commit and restores feature/docs to 645a066
```

After reset, re-attempt the merge with a fresh strategy — likely the conflict resolution needs to be done by reading both sides more carefully, OR the merge order should be reversed (rebase feature/docs onto develop instead of merging).

### If the push succeeds but CI fails on develop's expected behavior

This means a fix WAS regressed and our local test suite didn't catch it. Investigate via `git diff HEAD~ HEAD -- <file_that_failed>` and re-apply the missing piece, then push as a fix-up commit (DO NOT rebase the merge commit itself).

---

## Estimated time

| Task | Time |
|---|---|
| Task 1: Snapshot | 5 min |
| Task 2: Pre-merge audit | 10 min |
| Task 3: Execute merge + manual resolutions | 30-45 min |
| Task 4: Build + test gates | 5 min + cargo wall-clock |
| Task 5: Commit | 5 min |
| Task 6: Final verification + push | 10 min |

**Total: ~1-1.5 hours active.**

After this merge lands, the next step is the PR plan in
[`2026-06-07-pr-feature-docs-to-develop.md`](2026-06-07-pr-feature-docs-to-develop.md)
Task 3 onwards (skip Tasks 1-2 since the merge is done here).
