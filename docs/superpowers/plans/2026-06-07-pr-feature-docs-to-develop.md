# PR `feature/docs` → `develop` — execution plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans
> for sequential execution with checkpoints.

**Goal:** Ship the entire June 2026 work (spike + subsystems B/C/D/E/F + P1+P2 + docs) as a single PR to `develop`, after syncing the 19 commits develop has accumulated independently (suspend resume fix, gemini scalar fix, HITL demo).

**Strategy:** Merge `develop` → `feature/docs` first (resolve conflicts on our branch), then PR `feature/docs` → `develop`. Preserves history; merge commit lands cleanly in develop.

---

## Status snapshot

**Branch divergence (as of 2026-06-07):**

| Branch | Commits | Status |
|---|---|---|
| `feature/docs` | 256 commits ahead of `origin/develop` | All June 2026 work (spike + 6 subsystems + docs) |
| `origin/develop` | 19 commits ahead of `feature/docs` | Suspend resume fix, Gemini scalar fix, HITL demo, style cleanups |
| Merge base | `9b0c9f0` (2026-06-01) | Just after spike branched off |

**Files touched on both sides (potential conflicts):**

| File | Auto-merge? | Notes |
|---|---|---|
| `CLAUDE.md` | ✅ | Status entries in different sections |
| `docs/CHANGELOG_2026-06.md` | ❌ **CONFLICT** | Both append entries; need manual merge |
| `docs/DEVELOPER_GUIDE.md` | ✅ | TOC entries in different positions (but see number collision below) |
| `docs/node_configurations.json` | ✅ | Different node types |
| `src/.../dag_engine/application/run_use_case.rs` | ✅ | Suspend fix touched different region |
| `src/.../dag_engine/infrastructure/nodes/llm.rs` | ✅ | Suspend fix touched different region |
| `src/.../llm/infrastructure/gemini_adapter.rs` | ✅ | Gemini scalar fix in different method |

**Filename collision (not a merge conflict, but a TOC issue):**
- We have `docs/developer_guide/38_crdt_documents.md`
- develop added `docs/developer_guide/38_suspend_node.md`

Both files coexist, but the TOC numbering needs reconciliation — the suspend node guide should be renumbered to the next free slot (`44_suspend_node.md`) since CRDT (38) was created first and is referenced by other docs.

**19 develop-only commits to absorb:**

```
c5644aa Merge pull request #85 from Startti/claude/affectionate-agnesi-488291
0b3ea3a docs(suspend): list new test graphs + fix incorrect Q/A example
772c9f3 test(graphs): add HITL email approval demo (suspend + router)
2e9216b style(dag_engine): unify HashSet/HashMap imports + update spec status
189c3f7 style: cargo fmt the changes from this bugfix branch
7a7aa76 docs(changelog): strip confabulated CRDT content from CHANGELOG_2026-06
8c4ae4a docs(suspend): cross-link the resume_answer routing fix spec + CHANGELOG
f674204 fix(llm_call): defensive fallthrough when resume_answer has no pending tool
14d466e test(suspend): integration test for suspend→suspend cascade resume
af87e4f fix(dag_engine): gate __colmena_resume_answer injection by SUSPENDED set
8eab740 feat(dag_engine): add compute_resuming_node_ids helper
f9f7242 test(suspend): failing repro of suspend→llm_call resume bug
595efb9 docs(plan): implementation plan for suspend resume_answer routing fix
8f8c411 docs(spec): suspend resume_answer routing fix (Approach B)
06fbd93 Merge pull request #84 from Startti/claude/quirky-curie-23401d
1e6e3d8 docs(suspend): add dedicated developer guide + canonical examples
8e00448 Merge branch 'fix/gemini-non-object-tool-response' into develop
d99e975 fix(gemini): wrap non-object tool responses in {result: ...}
b6412a6 test(gemini): add regression tests for scalar function_response
```

---

## Task 1: Pre-merge — snapshot current state

**Files:** none

- [ ] **Step 1: Confirm clean working tree**

```bash
git status --short
```
Expected: empty.

- [ ] **Step 2: Confirm we're on `feature/docs`**

```bash
git branch --show-current
```
Expected: `feature/docs`.

- [ ] **Step 3: Snapshot current HEAD**

```bash
git log -1 --oneline > /tmp/colmena_pr/pre_merge_head.txt
mkdir -p /tmp/colmena_pr
git log -1 --oneline | tee /tmp/colmena_pr/pre_merge_head.txt
```
Save this — useful for rollback (`git reset --hard <sha>`).

- [ ] **Step 4: Verify origin/develop is current**

```bash
git fetch origin develop
git log -1 --oneline origin/develop
```

- [ ] **Step 5: Run final pre-merge sanity check**

```bash
cargo test --lib -p colmena_dag_engine 2>&1 | tail -3
cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```
Expected: 1388 passed, clippy clean, fmt clean. If any fails, STOP — do not start the merge with a red branch.

---

## Task 2: Merge `develop` into `feature/docs`

**Files:**
- Resolve: `docs/CHANGELOG_2026-06.md`
- Possibly rename: `docs/developer_guide/38_suspend_node.md` → `docs/developer_guide/44_suspend_node.md`
- Update: `docs/DEVELOPER_GUIDE.md` (TOC for renumbered file)

- [ ] **Step 1: Start the merge**

```bash
git merge origin/develop --no-ff -m "Merge develop into feature/docs

Brings in 19 commits from develop:
- Suspend resume_answer routing fix (Approach B)
- Gemini non-object tool response wrapping fix
- HITL email approval demo graph
- Misc style cleanups + changelog strip

Conflict resolution: docs/CHANGELOG_2026-06.md merged manually
keeping both sets of entries. 38_suspend_node.md renumbered to
44_suspend_node.md to avoid TOC collision with 38_crdt_documents.md."
```

This will halt on the CHANGELOG conflict.

- [ ] **Step 2: Resolve the CHANGELOG conflict**

Open `docs/CHANGELOG_2026-06.md`. The conflict will look like:

```
<<<<<<< HEAD (feature/docs)
## 11. Sheets write safety — collision policy + update_in_place (P1+P2)

[long entry]
=======
[Develop's entry — likely the strip of confabulated content + suspend
resume fix entry + Gemini scalar fix entry + HITL demo entry]
>>>>>>> origin/develop
```

**Resolution principle:** keep BOTH sides — append develop's entries
chronologically before §11 (since §11 is the most recent). Use this
order:

1. Original §1-§10 (E-T15 through E-T21) — already in feature/docs
2. develop's strip + suspend-routing + gemini-scalar + HITL entries —
   inserted as §11, §12, §13, §14
3. Renumber our existing P1+P2 entry from §11 to §15

The strip commit `7a7aa76` deletes CRDT content develop claims was
confabulated; in feature/docs that content is real (spike → B/C/D/F are
implemented). So when applying the strip, **only apply it to develop's
own content** — do NOT delete our CRDT sections.

If unsure: open both sides side-by-side via
`git show origin/develop:docs/CHANGELOG_2026-06.md > /tmp/dev_changelog.md`
and compare manually.

- [ ] **Step 3: Verify the renumbering decision**

```bash
ls docs/developer_guide/ | grep "^38_"
```
Expected: both `38_crdt_documents.md` and `38_suspend_node.md` present.

Choose the higher number for the new file: `44_suspend_node.md`.

```bash
git mv docs/developer_guide/38_suspend_node.md docs/developer_guide/44_suspend_node.md
```

- [ ] **Step 4: Update DEVELOPER_GUIDE.md to reflect the renumber**

Open `docs/DEVELOPER_GUIDE.md`. Find any line referring to
`38_suspend_node.md` — change to `44_suspend_node.md`. Also find the
TOC entry that develop's merge added and renumber it from 38 → 44.

Verify cross-references:

```bash
grep -rn "38_suspend_node\|suspend_node" docs/ src/ 2>/dev/null | grep -v "/target/" | grep -v "44_suspend_node"
```
Expected: empty.

- [ ] **Step 5: Stage all conflict resolutions**

```bash
git add docs/CHANGELOG_2026-06.md docs/DEVELOPER_GUIDE.md docs/developer_guide/44_suspend_node.md
```

- [ ] **Step 6: Verify the merge state**

```bash
git status --short
```
Expected: no unmerged paths. Only modifications + the rename of suspend_node.

- [ ] **Step 7: Run full test sweep BEFORE committing the merge**

```bash
cargo build -p colmena_dag_engine 2>&1 | tail -5
cargo test --lib -p colmena_dag_engine 2>&1 | tail -3
cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```

Expected: all green. The suspend fix tests + Gemini scalar tests
should now run alongside our 1388 — total should be ~1395+ passed.

If any test fails, STOP. The most likely culprits:
- Suspend resume tests interacting with our changes to llm.rs (check
  if our `on_existing_sheet` field deserialization conflicts with
  any path the suspend fix added)
- Gemini scalar response wrapping conflicting with our tool result
  shapes (unlikely — they touch different layers)

Diagnose by running the failing test verbosely:
```bash
cargo test --lib -p colmena_dag_engine <test_name> -- --nocapture
```

- [ ] **Step 8: Commit the merge**

```bash
git commit --no-edit
```
(The `-m` message from step 1 is preserved.)

Expected: clean commit. Note the merge SHA.

- [ ] **Step 9: Verify post-merge state**

```bash
git log --oneline -5
git log --oneline HEAD..origin/develop | wc -l    # should be 0
git log --oneline origin/develop..HEAD | wc -l    # should be ~270+ (256 ours + 19 from develop = ~275)
```

---

## Task 3: Pre-PR verification sweep

**Files:** none

- [ ] **Step 1: Full verbose test suite**

```bash
cargo test --verbose -p colmena_dag_engine 2>&1 | tee /tmp/colmena_pr/full_test.log | tail -10
```

Expected: all unit + integration + doctests pass. Look for `test result: ok` summary line at the very end.

- [ ] **Step 2: Ignored tests sweep (env-gated)**

```bash
set -a; source .env; set +a
export GOOGLE_APPLICATION_CREDENTIALS=/Users/danielgarcia/colmena-sa.json
cargo test --lib -p colmena_dag_engine -- --ignored 2>&1 | tee /tmp/colmena_pr/ignored_tests.log | tail -10
```

These need DATABASE_URL + TAVILY_API_KEY + GOOGLE_APPLICATION_CREDENTIALS. Some may still skip — note which.

- [ ] **Step 3: Workspace-wide check**

```bash
cargo check --workspace --all-targets 2>&1 | tail -5
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: clean across the entire workspace, not just the dag_engine crate.

- [ ] **Step 4: ADP worker compatibility sweep**

Per CLAUDE.md breaking-change discipline: any public-API change must
be swept against the ADP worker at
`/Users/danielgarcia/startti/adp/apps/service/ia/platform/{worker,api}/src/`.

The breaking changes in this PR:
- `RunPythonArgs::write_to_sheet` removed from `crdt_doc_run_python`
- `output_sheet` (singular) Python global removed from CRDT postlude
- Default `on_existing_sheet` policy changed from `auto_suffix` → `fail`
- Several `output_sheets` entry shape additions (additive — bare DataFrame still works)

```bash
cd /Users/danielgarcia/startti/adp
grep -rn "write_to_sheet\|output_sheet[^s]" apps/service/ia/platform/ 2>/dev/null | grep -v node_modules | head -20
```

Expected: empty (or only matches in `node_modules` / build artifacts).

If matches found: STOP. Either fix ADP first (file ADP issues with
the path to migrate) or revert the legacy removal commit in
feature/docs (NOT recommended — pollutes the PR).

```bash
cd /Users/danielgarcia/startti/colmena
```

- [ ] **Step 5: Documentation completeness check**

```bash
# Verify all the docs we created are tracked
git log origin/develop..HEAD --name-only --diff-filter=A 2>/dev/null | grep -E "^docs/superpowers/(specs|plans)/" | sort -u
git log origin/develop..HEAD --name-only --diff-filter=A 2>/dev/null | grep -E "^docs/developer_guide/" | sort -u
```

Expected list (specs + plans for the 6 subsystems + P1+P2 + this PR plan).
Expected dev guides: 38_crdt_documents, 39_gsheets, 40_toolkit_packages,
41_builtin_tools_index, 42_builtin_skills_index, 43_sheets_local_vs_gsheets,
44_suspend_node.

- [ ] **Step 6: Push the branch**

```bash
git push origin feature/docs 2>&1 | tail -5
```

If push is rejected (it shouldn't be — we're the only writer to this
branch), do `git pull --rebase origin feature/docs` first then re-push.

---

## Task 4: Open the PR

**Files:** none (PR body lives on GitHub)

- [ ] **Step 1: Generate the PR body locally for review**

Save as `/tmp/colmena_pr/pr_body.md`:

```markdown
# June 2026 monthly drop — spike + subsystems B/C/D/E/F + P1+P2

This PR ships everything from `feature/docs` since `develop` diverged
on 2026-06-01. Six independent subsystems landed across the month,
each with its own spec/plan/implementation/docs cycle.

## Subsystems shipped

| Subsystem | Description | Spec | Status |
|---|---|---|---|
| **Spike** | CRDT documents foundation — `yrs::Doc` + WS sync + Univer browser | [spike-results](docs/superpowers/specs/2026-05-31-documents-crdt-spike-results.md) | ✅ |
| **CRDT v1 (B)** | Multi-peer auto-context recent changes | [v1-design](docs/superpowers/specs/2026-06-01-documents-crdt-v1-design.md) + [B-design](docs/superpowers/specs/2026-06-03-crdt-recent-changes-design.md) | ✅ |
| **CRDT v1 (C)** | Pandas integration via `crdt_doc_run_python` sandbox | [C-design](docs/superpowers/specs/2026-06-03-crdt-pandas-integration-design.md) | ✅ |
| **CRDT v1 (D)** | Server-side formula evaluation (formualizer) | [D-design](docs/superpowers/specs/2026-06-04-crdt-formulas-design.md) | ✅ |
| **CRDT v1 (F)** | Cross-artifact analysis (`list_sheets_of` + `import_sheet`) | [F-design](docs/superpowers/specs/2026-06-04-crdt-cross-sheet-analysis-design.md) | ✅ |
| **Google Sheets (E)** | New `gsheets/` module + 10 synthetic tools + `gsheets_run_python` | [E-design](docs/superpowers/specs/2026-06-05-google-sheets-design.md) | ✅ |
| **Toolkit packages** | `enabled_tools: ["gsheets"]` flag-only activation | [packages-design](docs/superpowers/specs/2026-06-06-toolkit-packages-design.md) | ✅ |
| **Text centralization** | All LLM-facing strings moved to `text/` folder | [centralization-design](docs/superpowers/specs/2026-06-06-text-centralization-design.md) | ✅ |
| **Skills navigation** | Built-in skills index + per-tool skill auto-loading | [skills-nav-design](docs/superpowers/specs/2026-06-06-skills-navigation-design.md) | ✅ |
| **Pandas multi-sheet** | `output_sheets = {name: df}` write-back | [multisheet-design](docs/superpowers/specs/2026-06-06-pandas-multisheet-and-exploration-design.md) | ✅ |
| **Sheets write safety (P1+P2)** | Collision policy + `update_in_place` mode | [safety-design](docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md) | ✅ |

## Breaking changes

⚠️ **Three breaking changes**, all in CRDT/sheets tool surface; ADP worker
swept clean for all:

1. **`crdt_doc_run_python.write_to_sheet` arg removed.** Was the legacy
   single-tab write-back. Replaced by `output_sheets = {name: df}`.
   3 in-repo test graphs migrated.

2. **`output_sheet` (singular) Python global removed from CRDT postlude.**
   Same legacy path as above. Replaced by `output_sheets` (plural).

3. **Default `on_existing_sheet` policy changed from `auto_suffix` →
   `fail`.** When `output_sheets` writes to an existing tab, the
   dispatcher now returns a structured `SheetExists` error instead of
   silently writing to `"Name (2)"`. Operators who want the old
   behavior must set `fixed_config.on_existing_sheet: "auto_suffix"`.

## Verification

- ✅ **Unit + integration tests:** 1388 passed (with 28 ignored env-gated)
- ✅ **Clippy:** clean under `-D warnings`
- ✅ **Fmt:** clean
- ✅ **ADP worker compat:** swept, no legacy refs
- ✅ **E2E live:** P1 collision `fail` + P2 `update_in_place` dispatch
  verified against real Google Sheets (see CHANGELOG §15)

## Sub-PR structure for review

Reviewers may find it easier to navigate by sub-system. Each one has
its own commit range:

| Sub-system | Commit range | LoC |
|---|---|---|
| Spike | `01a6857..35cb5cb` | ~3500 |
| B (recent changes) | (TBD — fill from `git log`) | ~2200 |
| C (pandas) | (TBD) | ~1500 |
| D (formulas) | (TBD) | ~2800 |
| F (cross-artifact) | (TBD) | ~1100 |
| E (gsheets) | (TBD) | ~3400 |
| Toolkits / text / skills | (TBD) | ~1200 |
| P1+P2 (write safety) | `bdc3fc0..0a18d58` | ~2500 |
| Docs (orientation guide) | `0a18d58` | ~324 |

(Fill in TBDs by running `git log --oneline --grep="<subsystem>"` per
section before opening the PR.)

## Documentation

- [CHANGELOG_2026-06.md](docs/CHANGELOG_2026-06.md) — full chronological
  log of every ship + the merge-in from develop.
- [DEVELOPER_GUIDE.md](docs/DEVELOPER_GUIDE.md) — index of all docs,
  including the 7 new dev guides shipped this month (38_crdt_documents,
  39_gsheets, 40_toolkit_packages, 41_builtin_tools_index,
  42_builtin_skills_index, 43_sheets_local_vs_gsheets, 44_suspend_node).
- [BACKLOG.md](docs/BACKLOG.md) — every parked item from this work,
  with explicit triggers for "when to retake."

## Post-merge actions

After this PR merges to develop:
1. Verify Cloud Build picks up the new colmena commit in the ADP
   worker pipeline (it pulls colmena develop directly).
2. Smoke test the deployed ADP worker against `api.dev.startti.ai`.
3. The 4 pending BACKLOG items from sheets-write-safety v1.1
   (overwrite E2E, append/upsert/delete_where modes, last_modified
   in envelope, drive.file scope, 26-column header limit) wait for
   real-world triggers.
4. The crdt_documents v1.1 BACKLOG items (visual formatting, WS
   auto-reconnect, TTL eviction, etc.) wait for v1 to be deployed
   to production for ≥2 weeks before triage.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
```

- [ ] **Step 2: Create the PR via gh CLI**

```bash
gh pr create --base develop --head feature/docs \
  --title "June 2026: spike + subsystems B/C/D/E/F + sheets write safety" \
  --body-file /tmp/colmena_pr/pr_body.md 2>&1 | tee /tmp/colmena_pr/pr_create.log
```

The output should include the PR URL — paste it back to the user.

- [ ] **Step 3: Verify the PR opened cleanly**

```bash
gh pr view --json url,title,state,mergeable,additions,deletions,changedFiles | jq .
```

Expected:
- `state: "OPEN"`
- `mergeable: "MERGEABLE"` (or `"UNKNOWN"` initially while GitHub computes it)
- `changedFiles`: large number (~250-300)
- `additions`/`deletions`: very large (thousands)

If `mergeable: "CONFLICTING"`, something happened between merging
develop and pushing — pull develop again, re-resolve, push, re-check.

- [ ] **Step 4: Optionally trigger draft → ready transition**

If the PR was opened as draft (gh pr create defaults to draft for
large diffs sometimes):

```bash
gh pr ready
```

---

## Task 5: PR review iteration (placeholder)

**Files:** depends on review feedback

This task is a placeholder — actual review feedback will dictate the
changes. Track each round as a separate sub-task with its own commit
range:

- [ ] **Round 1: Address review comments**

```bash
# Each fix as its own commit; do NOT amend the merge commit
git commit -m "fix(<area>): address review comment on <topic>"
git push
```

- [ ] **Round N: Final approval**

Wait for at least 1 LGTM from a reviewer. If automated CI fails on
something subtle (e.g. a doctest only run in `cargo test --verbose`
that didn't show locally), fix and re-push.

---

## Task 6: Merge to develop

**Files:** none (GitHub action)

- [ ] **Step 1: Confirm green CI + LGTM**

```bash
gh pr checks
gh pr view --json reviewDecision | jq -r .reviewDecision
```

Expected: all checks passing, `APPROVED` or `REVIEW_REQUIRED` (if
your repo doesn't require formal approval).

- [ ] **Step 2: Merge — prefer "Create a merge commit"**

```bash
gh pr merge --merge --auto
```

`--merge` (not `--squash`, not `--rebase`) preserves the 256+
commits' authorship/timestamps in develop's history. This is
intentional for a monthly drop of this size — each subsystem's
commit history is the audit trail.

If your repo policy requires squash: use `gh pr merge --squash` and
write a comprehensive squash message that summarizes the 6 subsystems.

- [ ] **Step 3: Verify develop now has everything**

```bash
git checkout develop
git pull origin develop
git log --oneline | head -10   # should show the merge commit + our 256 + the 19 develop commits
```

- [ ] **Step 4: Optionally delete feature/docs locally + remote**

After the merge is confirmed:

```bash
gh pr view --json state | jq -r .state    # should be "MERGED"
git branch -d feature/docs
git push origin --delete feature/docs
```

Note: the user must explicitly approve branch deletion. If unsure,
leave it — branches are cheap.

---

## Rollback plan (if everything goes sideways)

**Scenario 1: Conflict resolution went wrong, want to redo the merge**

```bash
git reset --hard $(cat /tmp/colmena_pr/pre_merge_head.txt | awk '{print $1}')
# This restores feature/docs to before the merge. Re-run Task 2.
```

**Scenario 2: PR merged to develop and broke ADP worker**

```bash
# On develop, revert the merge commit
git checkout develop && git pull
git revert -m 1 <merge_commit_sha>
git push origin develop
# Then: fix the ADP-impacting commit on a fresh branch, re-PR.
```

**Scenario 3: Some subset of subsystems must be reverted**

```bash
# Each subsystem's commits are isolated; revert by SHA range
git revert <oldest_sha>..<newest_sha>
```

---

## Estimated time

- Task 1 (snapshot): 5 min
- Task 2 (merge + resolve): 30-60 min (mostly CHANGELOG conflict + renumber)
- Task 3 (verification sweep): 15 min + cargo test wall-clock (~3 min)
- Task 4 (PR creation): 10 min
- Task 5 (review iteration): unknown (depends on reviewer turnaround)
- Task 6 (final merge): 5 min

**Total active time: ~1.5-2 hours**. Wall-clock to merged depends on
review cycle.
