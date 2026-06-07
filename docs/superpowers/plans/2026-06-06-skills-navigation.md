# Skills Navigation Implementation Plan (Alpha)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the user-facing built-in skills index at `docs/developer_guide/42_builtin_skills_index.md`, upgrade `src/libs/colmena/skills/README.md` with a navigation table, and add a CI test that fails the build if a registered skill is missing from the index. Mirrors the pattern shipped for tools in E-T18.

**Architecture:** Pure documentation + one CI test. Zero code surface change. The CI test reads `src/libs/colmena/skills/` at test time via `std::fs::read_dir` and asserts every folder containing a `SKILL.md` (excluding `_placeholder`) appears as a backtick-wrapped token in the index doc.

**Tech Stack:** Markdown, standard Rust file I/O (`std::fs::read_dir`, `env!("CARGO_MANIFEST_DIR")`), `include_str!` for embedding the index doc into the test binary.

**Spec:** [docs/superpowers/specs/2026-06-06-skills-navigation-design.md](../specs/2026-06-06-skills-navigation-design.md)

---

## File Structure

**New files:**
- `docs/developer_guide/42_builtin_skills_index.md` — user-facing reference

**Modified files:**
- `src/libs/colmena/skills/README.md` — contributor-side upgrade (quick-nav table + add-a-skill recipe)
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` — append `index_doc_covers_all_registered_skills` test inside the existing `text_coverage_tests` module
- `docs/developer_guide/DEVELOPER_GUIDE.md` — index entry for `42_*.md`
- `docs/CHANGELOG_2026-06.md` — E-T19 ship entry
- `docs/BACKLOG.md` (optional) — auto-gen follow-up

---

## Task Dependency Graph

```
Task 1 (write index doc) ─┐
                          ├→ Task 3 (CI test — needs the doc to exist)
Task 2 (README upgrade) ──┘
                                     ↓
                              Task 4 (docs sweep — last)
```

Tasks 1 and 2 are independent; either order works.

---

## Task 1 (E-T19a): Write `42_builtin_skills_index.md`

**Goal:** A single user-facing doc listing every built-in skill with one-line description + link to its `SKILL.md`.

**Files:**
- Create: `docs/developer_guide/42_builtin_skills_index.md`

- [ ] **Step 1: Create the file**

Path: `docs/developer_guide/42_builtin_skills_index.md`

```markdown
# Built-in skills index

Every Rust-native skill bundled with the `colmena_dag_engine` crate. Each
row links to the skill's `SKILL.md` — that file is what the LLM actually
loads via the [`load_skill`](24_skills.md) tool.

The corresponding tool index lives at
[41_builtin_tools_index.md](41_builtin_tools_index.md). For the runtime
mechanism (`include_dir!`, frontmatter contract, references discovery),
see [24_skills.md](24_skills.md).

## The skills

| Skill | What it teaches | SKILL.md |
|---|---|---|
| `crdt-doc-cross-sheet-analysis` | Patterns for comparing two CRDT sheets, joining/enriching, transforming rows based on conditions from another sheet. | [link](../../src/libs/colmena/skills/crdt-doc-cross-sheet-analysis/SKILL.md) |
| `crdt-doc-formulas` | Excel-style formulas in a CRDT spreadsheet — the `{v,f,fs}` cell schema, `include_formulas=true`, `needs_browser` warning handling. | [link](../../src/libs/colmena/skills/crdt-doc-formulas/SKILL.md) |
| `crdt-doc-run-python` | Calling the `crdt_doc_run_python` tool — DataFrame shape rules, output protocol, type quirks, debugging. | [link](../../src/libs/colmena/skills/crdt-doc-run-python/SKILL.md) |
| `expense-analysis` | Analyzing expense data — categories, vendor rollups, period comparisons. | [link](../../src/libs/colmena/skills/expense-analysis/SKILL.md) |
| `gsheets-cross-sheet-analysis` | Same cross-sheet patterns as `crdt-doc-cross-sheet-analysis`, but via Google Sheets `gsheets_*` tools. | [link](../../src/libs/colmena/skills/gsheets-cross-sheet-analysis/SKILL.md) |
| `python-expert` | Modern Python (3.11+) — typing, async, dataclasses, stdlib internals. Not for general programming questions. | [link](../../src/libs/colmena/skills/python-expert/SKILL.md) |
| `sales-analysis` | Analyzing sales data — common tables, KPIs, pitfalls. | [link](../../src/libs/colmena/skills/sales-analysis/SKILL.md) |
| `sql-optimizer` | Writing, reviewing, or optimizing SQL — performance, indexes, joins, query plans. Not for ORM-specific questions. | [link](../../src/libs/colmena/skills/sql-optimizer/SKILL.md) |

## How an LLM loads a skill

Call the `load_skill` synthetic tool with the skill name. The runtime
returns the `SKILL.md` content (and any references the LLM later requests
via the `reference` parameter). See
[29_lazy_tool_loading.md](29_lazy_tool_loading.md) for how skills appear
in the catalog before being loaded.

```json
{ "name": "load_skill", "arguments": { "name": "gsheets-cross-sheet-analysis" } }
```

## How to add a new skill

1. Create a directory under `src/libs/colmena/skills/<kebab-name>/`.
2. Add a `SKILL.md` file with YAML frontmatter. `name` must match the
   directory name. `description` should be a one-liner suitable for this
   index.
3. Add reference files under `references/<name>.md` if needed (linked
   from the `references:` list in the frontmatter).
4. Append a row to the table above with a one-line summary.
5. Run `cargo test --lib index_doc_covers_all_registered_skills` — the
   build refuses to ship if your skill is in the registry but not in
   this doc.

See [24_skills.md](24_skills.md) for the full contract and runtime
mechanics.
```

- [ ] **Step 2: Commit**

```bash
cd /Users/danielgarcia/startti/colmena
git add docs/developer_guide/42_builtin_skills_index.md
git commit -m "docs(E-T19a): built-in skills index

User-facing reference listing every built-in skill with a one-liner and a
link to its SKILL.md. Mirrors 41_builtin_tools_index.md. The CI test in
the next task (E-T19c) refuses to ship if a registered skill is missing.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2 (E-T19b): Upgrade `skills/README.md`

**Goal:** Replace the existing brief README with a contributor-facing navigation guide. Quick-nav table + how-to-add-a-skill recipe.

**Files:**
- Modify: `src/libs/colmena/skills/README.md`

- [ ] **Step 1: Read the current README**

```bash
cd /Users/danielgarcia/startti/colmena
cat src/libs/colmena/skills/README.md
```

The current file is short (~10 lines). We rewrite it.

- [ ] **Step 2: Write the new content**

Path: `src/libs/colmena/skills/README.md`

```markdown
# Built-in skills

Each subdirectory here compiles into the `colmena_dag_engine` crate at
build time via the `include_dir!` macro and becomes available to LLM
nodes as a built-in skill. Skills load on demand via the `load_skill`
synthetic tool — see [`docs/developer_guide/24_skills.md`](../../../docs/developer_guide/24_skills.md)
for the runtime contract.

## Quick navigation

| Skill folder | What it teaches |
|---|---|
| `crdt-doc-cross-sheet-analysis` | Cross-sheet CRDT analysis (joins, enrichment) |
| `crdt-doc-formulas` | Excel-style formulas in CRDT sheets |
| `crdt-doc-run-python` | Using `crdt_doc_run_python` correctly |
| `expense-analysis` | Expense data patterns |
| `gsheets-cross-sheet-analysis` | Cross-sheet Google Sheets analysis |
| `python-expert` | Modern Python typing/async/stdlib |
| `sales-analysis` | Sales data analysis |
| `sql-optimizer` | SQL performance + optimization |

The user-facing version of this list, with descriptions taken from each
skill's frontmatter, lives at
[`docs/developer_guide/42_builtin_skills_index.md`](../../../docs/developer_guide/42_builtin_skills_index.md).

## How to add a new skill

1. Create a directory named after the skill (kebab-case, e.g.
   `python-expert/`). The directory name is the skill's canonical name.
2. Add a `SKILL.md` file with YAML frontmatter:

   ```yaml
   ---
   name: python-expert          # MUST match the directory name
   description: One-line description suitable for the user-facing index.
   when_to_load: When to invoke this skill (helps the LLM auto-discover).
   references:
     - typing
     - async
   ---
   ```
3. For each entry in `references`, add `references/<name>.md` next to
   `SKILL.md`.
4. Add a row to the user-facing index doc at
   `docs/developer_guide/42_builtin_skills_index.md` and to the quick-nav
   table above.
5. Run `cargo test --lib index_doc_covers_all_registered_skills` — the
   build refuses to ship if your skill is in the registry but not in the
   user-facing index.
6. Keep each file under 64 KB (the `include_dir!` size cap).

## Naming convention

Skill folder names are kebab-case. Uniqueness is enforced at build time
by the `include_dir!` macro — two folders with the same name produce a
compile error.

## The `_placeholder` folder

The `_placeholder/` directory exists only to give `include_dir!` a
non-empty contract when no real skills are present in CI. It is exempt
from the user-facing index and the completeness test.

## Full contract

See [`docs/developer_guide/24_skills.md`](../../../docs/developer_guide/24_skills.md).
```

- [ ] **Step 3: Commit**

```bash
cd /Users/danielgarcia/startti/colmena
git add src/libs/colmena/skills/README.md
git commit -m "docs(E-T19b): upgrade skills/README.md with navigation table

Contributor-side counterpart to docs/developer_guide/42_builtin_skills_index.md.
Adds quick-nav table, full add-a-skill recipe with frontmatter shape, and
explicit notes on naming + the _placeholder convention.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3 (E-T19c): CI test `index_doc_covers_all_registered_skills`

**Goal:** Append a new test inside the existing `text_coverage_tests` module in `mod.rs`. The test reads `skills/` via `std::fs::read_dir`, asserts every skill folder (with a `SKILL.md`, excluding `_placeholder`) appears as a backtick-wrapped token in the index doc.

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`

- [ ] **Step 1: Locate the existing `text_coverage_tests` module**

```bash
cd /Users/danielgarcia/startti/colmena
grep -n "mod text_coverage_tests\|fn index_doc_covers_all_registered_tools" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
```

Expected: shows the `text_coverage_tests` module and the existing
`index_doc_covers_all_registered_tools` test (shipped in E-T18b). You
will append the new test inside the same module.

- [ ] **Step 2: Add the new test**

Inside `mod text_coverage_tests { ... }`, after the existing
`index_doc_covers_all_registered_tools` test, append:

```rust
    #[test]
    fn index_doc_covers_all_registered_skills() {
        // Embed the skills-index doc via include_str! so the test is portable.
        // Path: from mod.rs (src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs)
        // go up 7 levels to repo root, then into docs/developer_guide/.
        const INDEX_DOC: &str = include_str!(
            "../../../../../../../docs/developer_guide/42_builtin_skills_index.md"
        );

        let skills_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("skills");

        let mut missing: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&skills_dir).expect("can read skills dir") {
            let entry = entry.expect("can read dir entry");
            if !entry.file_type().expect("file_type").is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            // _placeholder exists only for include_dir!'s empty-folder contract.
            if name == "_placeholder" {
                continue;
            }
            // Only folders that actually contain a SKILL.md count as registered skills.
            if !entry.path().join("SKILL.md").exists() {
                continue;
            }
            let needle = format!("`{}`", name);
            if !INDEX_DOC.contains(&needle) {
                missing.push(name);
            }
        }

        assert!(
            missing.is_empty(),
            "These registered skills are missing from \
             docs/developer_guide/42_builtin_skills_index.md: {:?}",
            missing,
        );
    }
```

The `..`-segment count (7) matches the existing `index_doc_covers_all_registered_tools` test in the same module — copy that exact path prefix.

- [ ] **Step 3: Run the new test**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib index_doc_covers_all_registered_skills --quiet 2>&1 | tail -10
```

Expected: PASS. If FAIL, the message lists which skill folders are missing from the index doc — go back to Task 1 and add them.

- [ ] **Step 4: Verify by simulating a missing entry**

This is a manual confidence check. Comment out one row in the index doc temporarily and re-run the test:

```bash
cd /Users/danielgarcia/startti/colmena
# Temporarily disable one row
sed -i.bak 's/`crdt-doc-formulas`/x-crdt-doc-formulas-x/' \
  docs/developer_guide/42_builtin_skills_index.md
cargo test --lib index_doc_covers_all_registered_skills --quiet 2>&1 | tail -5
```

Expected output mentions `crdt-doc-formulas` in the missing list. Then restore:

```bash
mv docs/developer_guide/42_builtin_skills_index.md.bak \
   docs/developer_guide/42_builtin_skills_index.md
cargo test --lib index_doc_covers_all_registered_skills --quiet 2>&1 | tail -3
```

Expected: PASS again.

- [ ] **Step 5: Full suite + clippy**

```bash
cargo test --lib --quiet 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
```

Expected: all green, no warnings.

- [ ] **Step 6: Commit**

```bash
cd /Users/danielgarcia/startti/colmena
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
git commit -m "test(E-T19c): index_doc_covers_all_registered_skills

CI gate: every skill folder containing a SKILL.md (excluding _placeholder)
must appear as a backtick-wrapped token in the user-facing index doc.

The test reads CARGO_MANIFEST_DIR/skills/ via std::fs::read_dir at test
time so a developer adding a new skill locally sees the failure
immediately, not just on CI.

Mirrors index_doc_covers_all_registered_tools shipped in E-T18b.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4 (E-T19d): Docs sweep

**Goal:** Update the developer-guide index + changelog + optional backlog entry. Pure markdown editing.

**Files:**
- Modify: `docs/developer_guide/DEVELOPER_GUIDE.md`
- Modify: `docs/CHANGELOG_2026-06.md`
- Modify: `docs/BACKLOG.md`

- [ ] **Step 1: Update `docs/developer_guide/DEVELOPER_GUIDE.md`**

Find the list of dev-guide sections (the file probably already has entries
for `40_toolkit_packages.md` and `41_builtin_tools_index.md` from prior
work). Add a line after `41`:

```markdown
- `42_builtin_skills_index.md` — every built-in skill with one-line description + link to its SKILL.md
```

To find the right place, grep:

```bash
cd /Users/danielgarcia/startti/colmena
grep -n "41_builtin_tools_index\|42_builtin_skills_index" docs/developer_guide/DEVELOPER_GUIDE.md
```

After your edit, both `41` and `42` entries appear adjacent.

- [ ] **Step 2: Append to `docs/CHANGELOG_2026-06.md`**

Append at the end (preserve the rolling-changelog style):

```markdown
- **E-T19 shipped 2026-06-06** — built-in skills index. New
  [`docs/developer_guide/42_builtin_skills_index.md`](developer_guide/42_builtin_skills_index.md)
  lists every Rust-native skill (8 today) with a one-line description and
  a link to its `SKILL.md`.
  [`src/libs/colmena/skills/README.md`](../src/libs/colmena/skills/README.md)
  upgraded with contributor-side navigation + add-a-skill recipe. New CI
  test (`index_doc_covers_all_registered_skills`) refuses to ship if a
  skill folder containing a `SKILL.md` is missing from the index.
```

- [ ] **Step 3: Append to `docs/BACKLOG.md`**

Append:

```markdown
- **Auto-generate `42_builtin_skills_index.md`** — read each `SKILL.md`
  frontmatter (name + description) and emit the markdown table. The CI
  completeness test would become redundant; the build step would be the
  single source of truth. Same shape as the deferred auto-gen for
  `41_builtin_tools_index.md`.
```

- [ ] **Step 4: Verify + commit**

```bash
cd /Users/danielgarcia/startti/colmena
git diff --stat docs/developer_guide/DEVELOPER_GUIDE.md \
                docs/CHANGELOG_2026-06.md \
                docs/BACKLOG.md
cargo test --lib --quiet 2>&1 | tail -3
git add docs/developer_guide/DEVELOPER_GUIDE.md \
        docs/CHANGELOG_2026-06.md \
        docs/BACKLOG.md
git commit -m "docs(E-T19d): final docs sweep

- DEVELOPER_GUIDE.md indexed the new 42_builtin_skills_index.md
- CHANGELOG_2026-06.md records the E-T19 ship entry
- BACKLOG.md captures the auto-gen follow-up (parallel to the deferred
  41_builtin_tools_index.md auto-gen)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Final sweep

- [ ] **Step 1: Full lib test suite**

```bash
cd /Users/danielgarcia/startti/colmena
cargo test --lib --quiet 2>&1 | tail -5
```

Expected: all tests pass, including the new `index_doc_covers_all_registered_skills`.

- [ ] **Step 2: Clippy + fmt**

```bash
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
cargo fmt --check 2>&1 | tail -3
```

Expected: clippy clean, no fmt diff. If fmt diff: `cargo fmt && git add -u && git commit -m "style: fmt"`.

---

## Self-review checklist

| Spec section | Plan task(s) | OK? |
|---|---|---|
| §1 Goals — user-facing index doc | Task 1 | ✅ |
| §1 Goals — skills/README.md upgrade | Task 2 | ✅ |
| §1 Goals — CI test | Task 3 | ✅ |
| §3 Open-source rule | Honoured: only colmena-shipped skills in the doc | ✅ |
| §4.2 Doc sections (intro + table + how-to-use + how-to-add) | Task 1 step 1 covers all four | ✅ |
| §4.3 README quick-nav + how-to-add + naming convention + `_placeholder` note | Task 2 step 2 covers all four | ✅ |
| §4.4 CI test algorithm (read_dir, skip `_placeholder`, require `SKILL.md`, assert backtick token) | Task 3 step 2 implements all five conditions | ✅ |
| §5 Edge cases (`_placeholder` exemption, missing SKILL.md, missing description) | Task 3 step 2 handles `_placeholder` + missing SKILL.md. Missing description is a doc-author concern, not enforced. | ✅ |
| §6 Test — `index_doc_covers_all_registered_skills` | Task 3 | ✅ |
| §7 Task breakdown 1:1 mapping | Tasks 1–4 map exactly to E-T19a–d | ✅ |
| §8 Back-compat — zero break | No code surface change; tests prove | ✅ |
| §9 Future BACKLOG — auto-gen | Task 4 step 3 records it | ✅ |
