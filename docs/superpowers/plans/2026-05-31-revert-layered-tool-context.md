# Revert layered-tool-context Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Revert layered-tool-context feature (layers 1a + 1b + 2 + plumbing) — keep only `llm_call.skills` + `load_skill` mechanism. Add recursive references and `skills_path` config to `llm_call`.

**Architecture:** Strategy is 4 atomic revert commits (one per layer/concern), followed by cleanup of built-in skills/tests/docs, then 2 feature commits (recursive refs + skills_path), then doc updates. Single PR to develop.

**Tech Stack:** Rust (tokio async), serde, async-trait, existing skills infrastructure (`SkillRepository`, frontmatter parser, `load_skill` synthetic tool).

**Design spec:** [docs/superpowers/specs/2026-05-31-revert-layered-tool-context-design.md](../specs/2026-05-31-revert-layered-tool-context-design.md)

---

## Conventions for this plan

- All commits use trailer: `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>`
- Run Rust tests with `cargo test --lib -p colmena_dag_engine [filter]` (package name).
- Lib crate name is `colmena` (used in `use colmena::...`).
- TDD where feature code is added; revert + cleanup phases skip TDD (we're removing code, not adding).
- Every task ends with a commit. Don't batch commits across tasks.
- Verify branch before commit: `git branch --show-current` must return `feature/revert-layered-skills`.
- The 17 layered commits in develop are: `48d69be`, `918acfd`, `a918a57`, `b190042`, `d40e5bc`, `d6d9630`, `de97917`, `0f0befc`, `08e617a`, `7fc4dd2`, `34342a9`, `2abdce1`, `16392e9`, `b94f910`, `9d03325`, `7678255`, `1677519`, `31cda9c`. **Do not** revert: skill-system pre-existing commits, anything from HTML feature, `8bc19f8` (sql auto-create — separate work).

The plan is split into **6 phases**:

| Phase | Scope | Commits |
|---|---|---|
| 0 | Setup (worktree + branch) | 0 |
| A | Revert layered (4 atomic reverts, one per concern) | 4 |
| B | Cleanup (built-in skill, e2e graphs, migration error) | 3 |
| C | Recursive references (TDD) | 1 |
| D | `skills_path` / `skills_paths` (TDD) | 1 |
| E | Docs (24_skills.md, superseded markers, CHANGELOG) | 1 |
| F | Final verify + PR | 0 |

Total: ~10 commits, ~24 tasks.

---

## Phase 0 — Setup

### Task 0.1: Create worktree + branch

**Files:** none

- [ ] **Step 1: Create isolated worktree from develop tip**

```bash
cd /Users/danielgarcia/startti/colmena
git fetch origin develop
git worktree add .claude/worktrees/revert-layered -b feature/revert-layered-skills origin/develop
cd .claude/worktrees/revert-layered
```

- [ ] **Step 2: Verify state**

```bash
git branch --show-current
git log --oneline -3
ls src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tool_context.rs
ls src/libs/colmena/skills/sql_query-guide/ 2>&1 | head -3
```

Expected: branch is `feature/revert-layered-skills`; HEAD matches origin/develop; `tool_context.rs` and `sql_query-guide/` directory both exist (we will delete them).

- [ ] **Step 3: Baseline test pass**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
cargo test --lib -p colmena_dag_engine 2>&1 | tail -3
```

Expected: a number of tests pass (the count is your baseline; it will go DOWN as we delete tests for removed code, then UP as we add tests for the new features).

No commit. This task is environment setup only.

---

## Phase A — Revert layered (4 atomic commits)

### Task A.1: Revert plumbing layer (`tool_context.rs`, executor wiring, per-request catalog, validation, extra_info)

**Files affected** (will be auto-touched by revert):
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tool_context.rs` (deleted)
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs` (modified)
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs` (modified)
- `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` (modified)
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (modified)

- [ ] **Step 1: Apply 7 plumbing reverts with --no-commit (most recent first)**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
git revert --no-commit 7678255 9d03325 b94f910 16392e9 2abdce1 34342a9 7fc4dd2
```

If any revert errors with "could not apply", STOP and report. The 7 commits in order: `tool_context_blocks in extra_info`, `graph-load validation`, `per-request load_skill catalog`, `unify skill_repo`, `pipe SkillRepository into describe_tool`, `append tool context block to ToolDefinition`, `build_tool_context_block`.

- [ ] **Step 2: Verify the revert removed the expected files**

```bash
test ! -f src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tool_context.rs && echo "tool_context.rs deleted ✓" || echo "FAIL"
git status -s | head -20
```

- [ ] **Step 3: Verify build still compiles**

```bash
cargo build --lib -p colmena_dag_engine 2>&1 | tail -5
```

Expected: success. If errors, they should be about other layered pieces still present that we revert in later tasks — note them but continue (we'll fix in A.2-A.4).

If errors are about UNRELATED things (e.g., HTML or other features), STOP — something went wrong.

- [ ] **Step 4: Commit**

```bash
git commit -m "$(cat <<'EOF'
revert: layered-tool-context plumbing (build_tool_context_block, per-request catalog, validation, tool_context_blocks)

Reverts the plumbing that wired layered-tool-context into the LLM pipeline:
- build_tool_context_block + tool_context.rs (deleted entirely)
- append tool context block to ToolDefinition.description
- pipe SkillRepository + registry into describe_tool dispatch
- unify skill_repo with existing skill_repository
- per-request load_skill catalog with layer 1/2/3 rules
- graph-load validation for skill wiring
- tool_context_blocks in extra_info summary

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task A.2: Revert Layer 2 (`tool_configuration.skills` + auto-derive)

**Files affected:**
- `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs`
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

- [ ] **Step 1: Apply layer 2 reverts**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
git revert --no-commit 31cda9c 1677519 08e617a
```

The 3 commits: `auto-derive also includes layer-1 guides matching tool node_types`, `auto-derive skill load list from tool.skills`, `add skills: Vec<String> field`.

- [ ] **Step 2: Verify the `skills` field is gone from ToolConfiguration**

```bash
grep "pub skills:" src/libs/colmena/src/dag_engine/domain/tool_configuration.rs && echo "FAIL: field still present" || echo "field removed ✓"
```

- [ ] **Step 3: Verify build**

```bash
cargo build --lib -p colmena_dag_engine 2>&1 | tail -5
```

Expected: success.

- [ ] **Step 4: Commit**

```bash
git commit -m "$(cat <<'EOF'
revert: layered-tool-context layer 2 (tool_configuration.skills + auto-derive from tool.skills)

Reverts tool-scoped skills feature:
- tool_configuration.skills field
- auto-derive skill load list from tool.skills
- auto-derive of layer-1 guides matching tool node_types

Tools no longer carry their own skill references. All skills are referenced
from llm_call.skills (layer 3, the only mechanism that survives).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task A.3: Revert Layer 1b (runtime `tool_description_supplement` + SqlPermissions::describe_policy_for_llm)

**Files affected:**
- `src/libs/colmena/src/dag_engine/domain/node.rs`
- `src/libs/colmena/src/dag_engine/domain/sql_permissions.rs`
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`

- [ ] **Step 1: Apply layer 1b reverts**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
git revert --no-commit 0f0befc de97917 d6d9630
```

The 3 commits: `SqlNode implements tool_description_supplement`, `SqlPermissions::describe_policy_for_llm`, `add tool_description_supplement hook to ExecutableNode`.

- [ ] **Step 2: Verify trait method is gone**

```bash
grep "tool_description_supplement" src/libs/colmena/src/dag_engine/domain/node.rs && echo "FAIL" || echo "trait method removed ✓"
grep "describe_policy_for_llm" src/libs/colmena/src/dag_engine/domain/sql_permissions.rs && echo "FAIL" || echo "describe_policy_for_llm removed ✓"
```

- [ ] **Step 3: Verify build + SQL tests still pass**

```bash
cargo build --lib -p colmena_dag_engine 2>&1 | tail -3
cargo test --lib -p colmena_dag_engine sql 2>&1 | tail -3
```

Expected: build success; existing SQL tests still pass (the validator's runtime enforcement is independent of this `describe_policy_for_llm` text generation).

- [ ] **Step 4: Commit**

```bash
git commit -m "$(cat <<'EOF'
revert: layered-tool-context layer 1b (tool_description_supplement runtime hook)

Reverts the runtime-computed config-derived text injection:
- ExecutableNode::tool_description_supplement trait method
- SqlNode impl that returned permissions text
- SqlPermissions::describe_policy_for_llm helper

The runtime SQL validator continues to enforce permissions; we no longer
proactively tell the LLM what's allowed via auto-injected text. If the user
wants the agent to know the policy upfront, they author a skill describing
it and reference it from llm_call.skills.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task A.4: Revert Layer 1a (node_type frontmatter + auto-attach + sql_query-guide)

**Files affected:**
- `src/libs/colmena/src/skills/domain/skill.rs`
- `src/libs/colmena/src/skills/domain/skill_repository.rs`
- `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs`
- `src/libs/colmena/src/skills/infrastructure/composite_skill_repository.rs`
- `src/libs/colmena/skills/sql_query-guide/` (directory deleted)

- [ ] **Step 1: Apply layer 1a reverts**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
git revert --no-commit d40e5bc b190042 a918a57 918acfd 48d69be
```

The 5 commits: `author sql_query-guide as the first layer-1 guide`, `find_by_node_type + duplicate-guide validation`, `add node_type to test SkillCatalogEntry instances`, `surface node_type through SkillCatalogEntry`, `parse optional node_type frontmatter`.

- [ ] **Step 2: Verify node_type is gone from Skill/SkillCatalogEntry**

```bash
grep "node_type" src/libs/colmena/src/skills/domain/skill.rs && echo "FAIL: still present" || echo "Skill.node_type removed ✓"
grep "node_type" src/libs/colmena/src/skills/domain/skill_repository.rs && echo "FAIL: still present" || echo "SkillCatalogEntry.node_type + find_by_node_type removed ✓"
test ! -d src/libs/colmena/skills/sql_query-guide && echo "sql_query-guide deleted ✓" || echo "FAIL"
```

- [ ] **Step 3: Verify build**

```bash
cargo build --lib -p colmena_dag_engine 2>&1 | tail -5
```

Expected: success.

- [ ] **Step 4: Commit**

```bash
git commit -m "$(cat <<'EOF'
revert: layered-tool-context layer 1a (node_type frontmatter + auto-attach + sql_query-guide)

Reverts the auto-attach of skills to tools by node_type:
- Skill.node_type field + parsing from SKILL.md frontmatter
- SkillCatalogEntry.node_type field
- SkillRepository::find_by_node_type + duplicate-guide validation
- Built-in skill src/libs/colmena/skills/sql_query-guide (used node_type)

Skills no longer auto-attach to any tool. To make a guide visible to an
LLM, reference it explicitly from llm_call.skills.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase B — Cleanup

### Task B.1: Delete e2e graphs that depended on layered

**Files affected:**
- `tests/graphs/agents/sql_layered_tool_context.json` (delete)
- `tests/graphs/agents/inventory_roleplay_*.json` (delete the layered variants)

- [ ] **Step 1: Find and delete layered-specific e2e graphs**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
ls tests/graphs/agents/sql_layered_tool_context.json 2>/dev/null && rm tests/graphs/agents/sql_layered_tool_context.json && echo "deleted sql_layered_tool_context.json"
ls tests/graphs/agents/inventory_roleplay_*.json 2>/dev/null
```

If `inventory_roleplay_*.json` files exist, inspect each: if they reference `tool_configuration.skills` or `node_type`, delete them. Otherwise keep.

```bash
for f in tests/graphs/agents/inventory_roleplay_*.json; do
  if [ -f "$f" ] && grep -q "\"skills\":\|node_type" "$f"; then
    echo "Deleting layered: $f"
    rm "$f"
  fi
done
```

- [ ] **Step 2: Also remove any cargo test fixture skills that used node_type**

```bash
find tests/fixtures src/libs/colmena/tests -name "SKILL.md" 2>/dev/null | xargs grep -l "^node_type:" 2>/dev/null | while read f; do
  dir=$(dirname "$f")
  echo "Deleting fixture: $dir"
  rm -rf "$dir"
done
```

- [ ] **Step 3: Run lib tests, identify and remove orphan tests for removed APIs**

```bash
cargo test --lib -p colmena_dag_engine 2>&1 | grep -E "(FAIL|error\[)" | head -20
```

If a test fails because it references `node_type`, `tool_description_supplement`, `tool_configuration.skills`, or the removed `tool_context_blocks`, locate that test and delete it (or fix it if it was a regression test we want to keep — case-by-case).

Common patterns to grep:
```bash
grep -rn "node_type\|tool_description_supplement\|tool_configuration\.skills\|tool_context_blocks\|build_tool_context_block\|find_by_node_type" src/libs/colmena/src --include="*.rs" 2>&1 | head -20
```

Each match needs to either: (a) be deleted as orphan layered code, or (b) updated if it's pre-existing code that survived the revert with a stale reference.

- [ ] **Step 4: Verify all lib tests pass**

```bash
cargo build --lib -p colmena_dag_engine 2>&1 | tail -3
cargo test --lib -p colmena_dag_engine 2>&1 | tail -3
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A tests/ src/libs/colmena/tests/ src/libs/colmena/src/
git commit -m "$(cat <<'EOF'
chore(skills): remove orphan tests and e2e graphs for reverted layered-tool-context

Deletes:
- tests/graphs/agents/sql_layered_tool_context.json
- tests/graphs/agents/inventory_roleplay_*.json (variants that used tool_configuration.skills or node_type)
- Test fixtures with SKILL.md frontmatter declaring node_type
- Stale references to find_by_node_type / tool_description_supplement / tool_context_blocks

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task B.2: Add migration error for legacy `node_type` frontmatter

**Files:**
- Modify: `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs` (or wherever frontmatter is parsed — confirm with `grep -rn "fn parse_frontmatter\|node_type" src/libs/colmena/src/skills`)

- [ ] **Step 1: Write a failing test**

In the skills test module that has access to a frontmatter parser, add:

```rust
#[test]
fn legacy_node_type_frontmatter_is_rejected_with_migration_error() {
    let yaml = "---\nname: my-skill\ndescription: x\nnode_type: sql_query\n---\nbody";
    let err = parse_skill_from_content("my-skill", yaml).expect_err("must reject");
    let msg = err.to_string();
    assert!(
        msg.contains("deprecated") && msg.contains("node_type"),
        "expected migration error message, got: {msg}"
    );
}
```

Replace `parse_skill_from_content` with the actual function name in the codebase. Discover it by:
```bash
grep -n "fn parse.*frontmatter\|fn parse_skill\|YamlFrontmatter\|fn from_content" src/libs/colmena/src/skills/infrastructure/*.rs | head -10
```

- [ ] **Step 2: Run, verify fails**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
cargo test --lib -p colmena_dag_engine legacy_node_type_frontmatter_is_rejected 2>&1 | tail -5
```

Expected: test compiles and FAILS (the parser today silently ignores unknown YAML keys).

- [ ] **Step 3: Implement the rejection**

In the frontmatter parser (the file where `serde_yaml::from_str` is called on the YAML chunk), BEFORE the typed deserialize, scan for the `node_type:` key and reject:

```rust
// Migration safeguard: layered-tool-context is reverted; flag legacy frontmatter.
if yaml_str
    .lines()
    .any(|l| l.trim_start().starts_with("node_type:"))
{
    return Err(SkillError::FrontmatterError {
        path: path.into(),
        reason: "deprecated 'node_type' frontmatter — layered-tool-context was reverted. \
                 Remove the node_type field; skills are now referenced explicitly from llm_call.skills."
            .into(),
    });
}
```

Adjust `SkillError::FrontmatterError` variant name and `path` parameter to whatever the actual error type uses (discover via `grep -n "pub enum SkillError" src/libs/colmena/src/skills/domain/skill_error.rs`).

- [ ] **Step 4: Run, verify pass**

```bash
cargo test --lib -p colmena_dag_engine legacy_node_type_frontmatter_is_rejected 2>&1 | tail -3
cargo test --lib -p colmena_dag_engine 2>&1 | tail -3
```

Expected: new test PASS; existing tests still PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/skills/
git commit -m "$(cat <<'EOF'
feat(skills): reject deprecated 'node_type' frontmatter with migration error

Skills authored under the layered-tool-context model that included
'node_type: X' in frontmatter now fail to load with a clear migration
message pointing users to llm_call.skills instead.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

### Task B.3: Audit + cleanup any remaining layered references in docs (light pass — full rewrite in Phase E)

**Files:**
- Modify: `docs/CODEBASE_TOUR.md` (if it mentions tool_description_supplement / node_type guides)
- Modify: `docs/AGENT_FEATURES_INDEX.md` (if it lists layered features)

- [ ] **Step 1: Find docs that mention layered concepts**

```bash
grep -rln "tool_description_supplement\|node_type guide\|layer 1\|layer 2\|build_tool_context_block\|tool_context_blocks" docs/ 2>/dev/null | grep -v "superpowers/specs\|superpowers/plans" | head -10
```

Files under `docs/superpowers/specs/` and `docs/superpowers/plans/` are kept for history and will be marked superseded in Phase E — skip them here.

- [ ] **Step 2: Remove references in code-tour docs**

For each file found, open it, remove the paragraphs about layered. If a file has substantial layered content (whole section), delete that section. Keep this pass light — Phase E does the full rewrite.

- [ ] **Step 3: Commit (only if any docs were modified)**

```bash
git status -s docs/
# If files modified:
git add docs/
git commit -m "$(cat <<'EOF'
docs: remove layered-tool-context references from code-tour and feature index

Light pass — full rewrite of skills docs happens in a separate commit
(see Phase E of the revert plan).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

If no docs were modified, skip the commit.

---

## Phase C — Recursive references

### Task C.1: Extend `SkillReferenceMeta` with nested `references` field

**Files:**
- Modify: `src/libs/colmena/src/skills/domain/skill.rs`

- [ ] **Step 1: Write failing test**

In `src/libs/colmena/src/skills/domain/skill.rs`, inside `#[cfg(test)] mod tests`, append:

```rust
#[test]
fn skill_reference_meta_supports_nested_references() {
    let meta = SkillReferenceMeta {
        name: "frameworks".into(),
        description: "Web frameworks".into(),
        references: vec![SkillReferenceMeta {
            name: "django".into(),
            description: "Django specifics".into(),
            references: vec![],
        }],
    };
    let s = serde_json::to_string(&meta).unwrap();
    let back: SkillReferenceMeta = serde_json::from_str(&s).unwrap();
    assert_eq!(back.references.len(), 1);
    assert_eq!(back.references[0].name, "django");
}

#[test]
fn skill_reference_meta_defaults_empty_references_when_missing() {
    // Backward-compat: existing SKILL.md files have references without the nested field.
    let yaml = r#"{"name":"foo","description":"bar"}"#;
    let meta: SkillReferenceMeta = serde_json::from_str(yaml).unwrap();
    assert!(meta.references.is_empty());
}
```

- [ ] **Step 2: Run, verify fails**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
cargo test --lib -p colmena_dag_engine skill_reference_meta 2>&1 | tail -5
```

Expected: compile error — `references` field not in `SkillReferenceMeta`.

- [ ] **Step 3: Add the field**

In `src/libs/colmena/src/skills/domain/skill.rs`, REPLACE the existing `SkillReferenceMeta` struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillReferenceMeta {
    pub name: String,
    pub description: String,
    /// Nested sub-references. Empty when this is a leaf.
    #[serde(default)]
    pub references: Vec<SkillReferenceMeta>,
}
```

- [ ] **Step 4: Run, verify pass**

```bash
cargo build --lib -p colmena_dag_engine 2>&1 | tail -3
cargo test --lib -p colmena_dag_engine skill_reference_meta 2>&1 | tail -5
```

Expected: PASS.

(If existing code that constructs `SkillReferenceMeta` fails to compile because of the new field, use `..Default::default()` — but the `#[serde(default)]` + adding a default impl might be necessary. Check `derive(Default)` is also on the struct, OR initialize the field explicitly at every call site.)

- [ ] **Step 5: Commit** (we batch this with the rest of Phase C in C.6)

NO COMMIT YET — Phase C is one logical change, we commit at the end.

### Task C.2: Parse `references` frontmatter from `references/*.md` files

**Files:**
- Modify: `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs` (and/or `path_skill_repository.rs` — wherever reference files are loaded)
- Modify: `src/libs/colmena/src/skills/domain/skill_repository.rs` (if the trait gates this)

- [ ] **Step 1: Discover where references are loaded**

```bash
grep -n "load_reference\|references/\|.md\"" src/libs/colmena/src/skills/infrastructure/*.rs | head -20
```

Identify the function that reads a `references/<name>.md` file. Today it likely just returns the raw bytes / parses no frontmatter.

- [ ] **Step 2: Write failing test**

In the appropriate infrastructure test module (e.g., `builtin_skill_repository.rs` tests), add:

```rust
#[tokio::test]
async fn reference_file_can_declare_nested_references() {
    // Author a fixture: a SKILL.md + references/frameworks.md where frameworks.md
    // has its OWN frontmatter declaring sub-references.
    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tmp.path().join("python-expert");
    std::fs::create_dir_all(skill_dir.join("references")).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: python-expert
description: Python expertise
references:
  - name: frameworks
    description: Web frameworks
---
Body.
"#,
    ).unwrap();
    std::fs::write(
        skill_dir.join("references/frameworks.md"),
        r#"---
references:
  - name: django
    description: Django specifics
  - name: fastapi
    description: FastAPI specifics
---
Frameworks overview.
"#,
    ).unwrap();
    std::fs::write(skill_dir.join("references/django.md"), "Django body").unwrap();
    std::fs::write(skill_dir.join("references/fastapi.md"), "FastAPI body").unwrap();

    let repo = PathSkillRepository::load(tmp.path()).await.unwrap();
    let entries = repo.list_available();
    let py = entries.iter().find(|e| e.name == "python-expert").unwrap();
    // The reference 'frameworks' should have 2 nested references.
    // ... assert via load + inspect, or via a new public method on the repo.
}
```

Adjust the type name `PathSkillRepository` and `load` method to whatever exists in the repo (discover via `grep -n "pub struct.*SkillRepository\|impl.*SkillRepository" src/libs/colmena/src/skills/infrastructure/*.rs`).

- [ ] **Step 3: Run, verify fails**

```bash
cargo test --lib -p colmena_dag_engine reference_file_can_declare_nested 2>&1 | tail -5
```

Expected: fails (parser doesn't read reference file frontmatter today).

- [ ] **Step 4: Implement frontmatter parsing for references**

In the function that loads a reference file, change it to:

1. Read the file content
2. Split on `---` to separate frontmatter from body
3. If frontmatter is present, parse with `serde_yaml::from_str` into a `ReferenceFrontmatter` struct (new):

```rust
#[derive(Debug, Deserialize, Default)]
struct ReferenceFrontmatter {
    #[serde(default)]
    references: Vec<SkillReferenceMetaInput>,
}

#[derive(Debug, Deserialize)]
struct SkillReferenceMetaInput {
    name: String,
    description: String,
}
```

4. Attach the parsed sub-references to the `SkillReferenceMeta` for this reference in the parent skill (when building the skill graph).

The implementation will need to recurse: a parsed sub-reference might itself live in a file with its own frontmatter declaring further sub-references.

- [ ] **Step 5: Run, verify pass**

```bash
cargo test --lib -p colmena_dag_engine reference_file_can_declare_nested 2>&1 | tail -5
cargo test --lib -p colmena_dag_engine 2>&1 | tail -3
```

Expected: PASS.

NO COMMIT YET.

### Task C.3: Detect reference cycles + enforce max depth 5

**Files:**
- Modify: same files as C.2 (parsing/loading code)

- [ ] **Step 1: Write failing tests**

```rust
#[tokio::test]
async fn reference_cycle_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tmp.path().join("looping");
    std::fs::create_dir_all(skill_dir.join("references")).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: looping\ndescription: x\nreferences:\n  - name: a\n    description: \"\"\n---\n",
    ).unwrap();
    std::fs::write(
        skill_dir.join("references/a.md"),
        "---\nreferences:\n  - name: b\n    description: \"\"\n---\n",
    ).unwrap();
    std::fs::write(
        skill_dir.join("references/b.md"),
        "---\nreferences:\n  - name: a\n    description: \"\"\n---\n",
    ).unwrap();
    let err = PathSkillRepository::load(tmp.path()).await.unwrap_err();
    assert!(err.to_string().to_lowercase().contains("cycle"));
}

#[tokio::test]
async fn reference_depth_over_5_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tmp.path().join("deep");
    std::fs::create_dir_all(skill_dir.join("references")).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: deep\ndescription: x\nreferences:\n  - name: a\n    description: \"\"\n---\n",
    ).unwrap();
    // a → b → c → d → e → f  (depth 6 below the skill's root references)
    for (this, next) in [("a","b"),("b","c"),("c","d"),("d","e"),("e","f")] {
        std::fs::write(
            skill_dir.join(format!("references/{this}.md")),
            format!("---\nreferences:\n  - name: {next}\n    description: \"\"\n---\n"),
        ).unwrap();
    }
    std::fs::write(skill_dir.join("references/f.md"), "leaf").unwrap();
    let err = PathSkillRepository::load(tmp.path()).await.unwrap_err();
    assert!(err.to_string().to_lowercase().contains("depth"));
}
```

- [ ] **Step 2: Run, verify fails**

```bash
cargo test --lib -p colmena_dag_engine reference_cycle reference_depth 2>&1 | tail -5
```

Expected: fails.

- [ ] **Step 3: Implement cycle detection + depth limit**

In the same loading function as C.2, when recursing through references:

```rust
const MAX_REFERENCE_DEPTH: u8 = 5;

fn load_reference_recursive(
    skill_dir: &Path,
    name: &str,
    description: &str,
    visited: &mut Vec<String>,
    depth: u8,
) -> Result<SkillReferenceMeta, SkillError> {
    if depth > MAX_REFERENCE_DEPTH {
        return Err(SkillError::ReferenceDepthExceeded {
            skill: visited.first().cloned().unwrap_or_default(),
            max: MAX_REFERENCE_DEPTH,
        });
    }
    if visited.iter().any(|n| n == name) {
        return Err(SkillError::ReferenceCycle {
            skill: visited.first().cloned().unwrap_or_default(),
            cycle: format!("{} → {}", visited.join(" → "), name),
        });
    }
    visited.push(name.to_string());

    let ref_path = skill_dir.join(format!("references/{name}.md"));
    let content = std::fs::read_to_string(&ref_path)?;
    let (frontmatter, _body) = split_frontmatter(&content);
    let parsed: ReferenceFrontmatter = if frontmatter.is_empty() {
        ReferenceFrontmatter::default()
    } else {
        serde_yaml::from_str(&frontmatter)?
    };

    let mut sub_refs = Vec::with_capacity(parsed.references.len());
    for sub in parsed.references {
        let sub_meta = load_reference_recursive(skill_dir, &sub.name, &sub.description, visited, depth + 1)?;
        sub_refs.push(sub_meta);
    }
    visited.pop();

    Ok(SkillReferenceMeta {
        name: name.to_string(),
        description: description.to_string(),
        references: sub_refs,
    })
}
```

Add corresponding variants to `SkillError`:

```rust
#[error("reference depth exceeds max {max} in skill '{skill}'")]
ReferenceDepthExceeded { skill: String, max: u8 },

#[error("reference cycle in skill '{skill}': {cycle}")]
ReferenceCycle { skill: String, cycle: String },
```

- [ ] **Step 4: Run, verify pass**

```bash
cargo test --lib -p colmena_dag_engine reference_cycle reference_depth 2>&1 | tail -5
cargo test --lib -p colmena_dag_engine 2>&1 | tail -3
```

Expected: PASS.

NO COMMIT YET.

### Task C.4: `load_reference` accepts path with `/`

**Files:**
- Modify: `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs` (and any other `SkillRepository` impls)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs` (synthetic tool that exposes load_reference)

- [ ] **Step 1: Write failing test**

In the same skills test file:

```rust
#[tokio::test]
async fn load_reference_navigates_nested_path() {
    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tmp.path().join("py");
    std::fs::create_dir_all(skill_dir.join("references")).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "---\nname: py\ndescription: x\nreferences:\n  - name: fw\n    description: x\n---\n").unwrap();
    std::fs::write(skill_dir.join("references/fw.md"), "---\nreferences:\n  - name: django\n    description: x\n---\nFrameworks").unwrap();
    std::fs::write(skill_dir.join("references/django.md"), "Django body").unwrap();
    let repo = PathSkillRepository::load(tmp.path()).await.unwrap();
    let r = repo.load_reference("py", "fw/django").await.unwrap();
    assert!(r.body.contains("Django body"));
    assert_eq!(r.reference_name, "fw/django");
}
```

- [ ] **Step 2: Run, verify fails**

```bash
cargo test --lib -p colmena_dag_engine load_reference_navigates_nested_path 2>&1 | tail -5
```

Expected: fails (today `load_reference` treats name as flat).

- [ ] **Step 3: Implement path navigation**

In each `SkillRepository` impl of `load_reference`, change to:

```rust
async fn load_reference(
    &self,
    skill_name: &str,
    reference_name: &str,
) -> Result<SkillReference, SkillError> {
    let segments: Vec<&str> = reference_name.split('/').collect();
    // The final segment is the leaf file name.
    let leaf = segments.last().ok_or_else(|| SkillError::InvalidReferencePath {
        path: reference_name.to_string(),
    })?;
    let ref_path = self.skill_path(skill_name).join("references").join(format!("{leaf}.md"));
    let content = tokio::fs::read_to_string(&ref_path).await.map_err(|_| {
        SkillError::ReferenceNotFound {
            skill: skill_name.to_string(),
            reference: reference_name.to_string(),
        }
    })?;
    let (_frontmatter, body) = split_frontmatter(&content);
    Ok(SkillReference {
        skill_name: skill_name.to_string(),
        reference_name: reference_name.to_string(),
        body,
    })
}
```

Note: validation that each intermediate segment is declared in its parent's frontmatter is done at LOAD time (Tasks C.2/C.3). At runtime in `load_reference`, we trust the structure and just open the leaf file (the LLM only knows about references that the skill graph documents, so an invalid path is the LLM's fault and the file simply won't exist → `ReferenceNotFound`).

Add `InvalidReferencePath` variant to `SkillError`:

```rust
#[error("invalid reference path: '{path}'")]
InvalidReferencePath { path: String },
```

- [ ] **Step 4: Update synthetic tool description**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs`, find the `load_reference` tool definition. Update its `reference_name` parameter description to mention path navigation:

```rust
"description": "The name of the reference to load. To navigate nested references, separate names with '/'. Example: 'frameworks/django/orm'."
```

- [ ] **Step 5: Run, verify pass**

```bash
cargo test --lib -p colmena_dag_engine load_reference 2>&1 | tail -5
cargo test --lib -p colmena_dag_engine 2>&1 | tail -3
```

Expected: PASS.

### Task C.5: Commit Phase C

- [ ] **Step 1: Verify all skills tests pass + fmt/clippy clean**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
cargo test --lib -p colmena_dag_engine skills 2>&1 | tail -3
cargo fmt --manifest-path src/libs/colmena/Cargo.toml --all -- --check; echo "fmt: $?"
cargo clippy --manifest-path src/libs/colmena/Cargo.toml --lib -- -D warnings 2>&1 | tail -3
```

If fmt fails: run `cargo fmt --manifest-path src/libs/colmena/Cargo.toml --all` and stage the changes.
If clippy fails: fix the lints.

- [ ] **Step 2: Commit**

```bash
git add -A src/libs/colmena/src/skills src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs
git commit -m "$(cat <<'EOF'
feat(skills): recursive references (nested) + path navigation in load_reference

Skills can now declare references whose own files (references/<name>.md)
carry frontmatter declaring further sub-references. SkillReferenceMeta
gains a nested `references` field; the loader recurses with a max depth
of 5 and detects cycles, both as hard errors at graph-load time.

load_reference("skill", "path/to/sub") navigates the tree by splitting
the second argument on '/'. The synthetic tool description documents
this for the LLM.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase D — `skills_path` / `skills_paths` in `llm_call`

### Task D.1: Add `skills_path` and `skills_paths` to the LLM node config schema

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (config parsing)

- [ ] **Step 1: Discover where llm_call config is parsed**

```bash
grep -n "skills:\|skills.*Vec<String>\|fn build_skill" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs | head -10
```

Find the struct or function that reads `config.skills` from the JSON.

- [ ] **Step 2: Write failing test**

In the llm.rs test module, add:

```rust
#[tokio::test]
async fn llm_config_accepts_skills_path() {
    // Create a temp dir with one skill, then build an llm_call config that
    // references it via skills_path and verify the loaded skills include it.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("hello-skill")).unwrap();
    std::fs::write(
        tmp.path().join("hello-skill/SKILL.md"),
        "---\nname: hello-skill\ndescription: hi\n---\nbody",
    ).unwrap();

    let cfg = serde_json::json!({
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "test",
        "skills_path": tmp.path().to_str().unwrap(),
    });
    let resolved = LlmNode::resolve_skill_names(&cfg).await.unwrap();
    assert!(resolved.iter().any(|n| n == "hello-skill"));
}

#[tokio::test]
async fn llm_config_accepts_skills_paths_plural() {
    let tmp1 = tempfile::tempdir().unwrap();
    let tmp2 = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp1.path().join("a")).unwrap();
    std::fs::write(tmp1.path().join("a/SKILL.md"), "---\nname: a\ndescription: x\n---\n").unwrap();
    std::fs::create_dir_all(tmp2.path().join("b")).unwrap();
    std::fs::write(tmp2.path().join("b/SKILL.md"), "---\nname: b\ndescription: x\n---\n").unwrap();

    let cfg = serde_json::json!({
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "test",
        "skills_paths": [tmp1.path().to_str().unwrap(), tmp2.path().to_str().unwrap()],
    });
    let resolved = LlmNode::resolve_skill_names(&cfg).await.unwrap();
    assert!(resolved.iter().any(|n| n == "a"));
    assert!(resolved.iter().any(|n| n == "b"));
}

#[tokio::test]
async fn llm_config_unions_skills_array_with_skills_path() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("from-path")).unwrap();
    std::fs::write(tmp.path().join("from-path/SKILL.md"), "---\nname: from-path\ndescription: x\n---\n").unwrap();

    let cfg = serde_json::json!({
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "test",
        "skills": ["builtin-name"],
        "skills_path": tmp.path().to_str().unwrap(),
    });
    let resolved = LlmNode::resolve_skill_names(&cfg).await.unwrap();
    assert!(resolved.contains(&"builtin-name".to_string()));
    assert!(resolved.contains(&"from-path".to_string()));
}
```

Replace `LlmNode::resolve_skill_names` with the actual method name in the codebase. If the resolution is inline in `execute`, refactor it into a static method `LlmNode::resolve_skill_names(config) -> Result<Vec<String>>` first (this is the design-for-testability improvement).

- [ ] **Step 3: Run, verify fails**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
cargo test --lib -p colmena_dag_engine llm_config_accepts_skills_path 2>&1 | tail -5
```

Expected: fails (config field doesn't exist yet).

- [ ] **Step 4: Implement**

In the LLM node config parser:

```rust
#[derive(Debug, Deserialize, Default)]
struct LlmCallSkillsConfig {
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    skills_path: Option<String>,
    #[serde(default)]
    skills_paths: Vec<String>,
}

impl LlmNode {
    pub async fn resolve_skill_names(config: &serde_json::Value) -> Result<Vec<String>, NodeError> {
        let parsed: LlmCallSkillsConfig = serde_json::from_value(config.clone())
            .unwrap_or_default();
        let mut all = std::collections::BTreeSet::<String>::new();
        for name in parsed.skills {
            all.insert(name);
        }
        let mut paths: Vec<String> = parsed.skills_paths;
        if let Some(p) = parsed.skills_path {
            paths.push(p);
        }
        for path in paths {
            let entries = list_skills_in_path(&path).await?;
            for name in entries {
                all.insert(name);
            }
        }
        Ok(all.into_iter().collect())
    }
}

async fn list_skills_in_path(path: &str) -> Result<Vec<String>, NodeError> {
    let mut out = vec![];
    let mut rd = tokio::fs::read_dir(path).await.map_err(|e| NodeError::ConfigError(
        format!("skills_path '{path}' not readable: {e}")
    ))?;
    while let Some(entry) = rd.next_entry().await.map_err(|e| NodeError::ConfigError(e.to_string()))? {
        if entry.path().join("SKILL.md").exists() {
            if let Some(name) = entry.file_name().to_str() {
                out.push(name.to_string());
            }
        }
    }
    Ok(out)
}
```

Then update the existing skills-resolution code path in `LlmNode::execute` to call `resolve_skill_names` instead of reading `config.skills` directly.

- [ ] **Step 5: Run, verify pass**

```bash
cargo test --lib -p colmena_dag_engine llm_config_ 2>&1 | tail -5
cargo test --lib -p colmena_dag_engine 2>&1 | tail -3
```

Expected: PASS.

### Task D.2: Hard error for missing skills_path + warn for empty path

**Files:** same as D.1

- [ ] **Step 1: Write failing tests**

```rust
#[tokio::test]
async fn skills_path_missing_returns_error() {
    let cfg = serde_json::json!({
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "test",
        "skills_path": "/nonexistent/path/xyz",
    });
    let err = LlmNode::resolve_skill_names(&cfg).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("not readable") || msg.contains("nonexistent"), "got: {msg}");
}

#[tokio::test]
async fn skills_path_empty_returns_empty_list_without_error() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = serde_json::json!({
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "test",
        "skills_path": tmp.path().to_str().unwrap(),
    });
    let resolved = LlmNode::resolve_skill_names(&cfg).await.unwrap();
    assert!(resolved.is_empty());
}
```

- [ ] **Step 2: Run, verify**

```bash
cargo test --lib -p colmena_dag_engine skills_path_missing skills_path_empty 2>&1 | tail -5
```

Expected: the first probably already passes (read_dir fails). The second probably passes too (no SKILL.md files = empty list, no error). If both pass without changes, that's fine — the existing impl already meets the contract.

- [ ] **Step 3: Commit Phase D**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
cargo fmt --manifest-path src/libs/colmena/Cargo.toml --all
cargo clippy --manifest-path src/libs/colmena/Cargo.toml --lib -- -D warnings 2>&1 | tail -3
cargo test --lib -p colmena_dag_engine 2>&1 | tail -3
git add -A src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "$(cat <<'EOF'
feat(llm): skills_path and skills_paths config for llm_call

LLM node config now accepts:
- skills: Vec<String>  (existing, names from any loaded repo)
- skills_path: String  (NEW, loads all skills under that directory)
- skills_paths: Vec<String>  (NEW, plural form)

All three coexist; the resolver unions them by name (dedup). Missing
skills_path is a hard error at resolution time; an empty path returns
an empty list without error.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase E — Docs

### Task E.1: Rewrite `docs/developer_guide/24_skills.md`

**Files:**
- Modify: `docs/developer_guide/24_skills.md`

- [ ] **Step 1: Read current state**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
wc -l docs/developer_guide/24_skills.md
grep -n "layer\|node_type\|tool_description_supplement\|tool_configuration.skills" docs/developer_guide/24_skills.md | head -10
```

- [ ] **Step 2: Edit the doc**

Make these changes:

1. **Remove** all sections about layers 1/2 (node-type guides auto-fold, tool_configuration.skills).
2. **Remove** all mentions of `node_type:` frontmatter.
3. **Remove** the "wart removal" and layered context examples.
4. **Add** a new section "Nested references" describing how a reference file can have its own `references:` frontmatter, with the example:
   ```
   my-skill/
     SKILL.md          # references: [{name: "fw", description: ...}]
     references/
       fw.md           # also has frontmatter: references: [{name: "django"}]
       django.md       # leaf — no frontmatter or empty references
   ```
   Mention `load_reference("my-skill", "fw/django")` and the max depth of 5.
5. **Add** a new section "Configuring skills on an LLM node" describing the three options:
   ```json
   {
     "type": "llm_call",
     "config": {
       "skills": ["python-expert"],
       "skills_path": "./my-skills",
       "skills_paths": ["./more-skills", "./still-more"]
     }
   }
   ```
   Explain: union by name, dedup, missing path = hard error.

- [ ] **Step 3: Verify the doc no longer mentions layered concepts**

```bash
grep -n "layer 1\|layer 2\|node_type\|tool_description_supplement\|tool_configuration\.skills" docs/developer_guide/24_skills.md && echo "FAIL" || echo "clean ✓"
```

### Task E.2: Mark superseded specs/plans + add CHANGELOG entry

**Files:**
- Modify: `docs/superpowers/specs/2026-05-29-layered-tool-context-design.md`
- Modify: `docs/superpowers/plans/2026-05-29-layered-tool-context.md`
- Modify: `docs/CHANGELOG_2026-05.md`

- [ ] **Step 1: Add superseded marker at the top of the layered spec**

In `docs/superpowers/specs/2026-05-29-layered-tool-context-design.md`, after the `# Spec` header, INSERT:

```markdown
> ⚠️ **SUPERSEDED on 2026-05-31** by
> [2026-05-31-revert-layered-tool-context-design.md](2026-05-31-revert-layered-tool-context-design.md).
> The 3-layer mechanism was reverted; only `llm_call.skills` survives.
> Recursive references and `skills_path` were added as replacements.
```

- [ ] **Step 2: Same marker on the plan**

In `docs/superpowers/plans/2026-05-29-layered-tool-context.md`, after the header, INSERT:

```markdown
> ⚠️ **SUPERSEDED on 2026-05-31**. See
> [2026-05-31-revert-layered-tool-context.md](2026-05-31-revert-layered-tool-context.md)
> for the revert + replacement plan.
```

- [ ] **Step 3: Add CHANGELOG entry**

Append to `docs/CHANGELOG_2026-05.md` (or create today's CHANGELOG file if needed):

```markdown
## 2026-05-31 — Revert layered-tool-context

**Breaking changes:**

- Skill frontmatter no longer supports `node_type:` (rejected with migration error).
- `tool_configuration.skills` field removed from graph schema.
- `ExecutableNode::tool_description_supplement` trait method removed (auto-injected
  policy text no longer reaches the LLM; runtime validators still enforce).
- Built-in skill `sql_query-guide` removed.

**Migration:**

- Skills with `node_type:` → remove the frontmatter field; reference them
  explicitly from `llm_call.skills`.
- `tool_configuration.skills` → move skill names to `llm_call.skills`.
- SQL permissions visibility to the LLM → author a skill markdown
  describing the policy and reference it from `llm_call.skills`.

**New features:**

- Recursive references: `references/<name>.md` files can declare their own
  `references:` frontmatter. `load_reference("skill", "path/to/sub")` navigates.
- `llm_call.skills_path` / `skills_paths` config: load all skills under a
  directory without enumerating by name.
```

- [ ] **Step 4: Commit Phase E**

```bash
git add -A docs/
git commit -m "$(cat <<'EOF'
docs(skills): rewrite 24_skills.md for post-revert model + supersede layered spec/plan + CHANGELOG entry

- 24_skills.md: removed all references to layered (node_type, tool_description_supplement,
  tool_configuration.skills); added 'Nested references' and 'Configuring skills on an
  LLM node' sections.
- Marked 2026-05-29-layered-tool-context spec + plan as SUPERSEDED.
- Added CHANGELOG entry with breaking changes and migration guide.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Phase F — Final verify + PR

### Task F.1: Full verification pass

**Files:** none

- [ ] **Step 1: Run lib + integration tests**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
cargo test --lib -p colmena_dag_engine 2>&1 | tail -5
```

Expected: all tests PASS. Note the count.

- [ ] **Step 2: Run all integration test files**

```bash
ls src/libs/colmena/tests/*.rs | while read f; do
  name=$(basename "$f" .rs)
  echo "=== $name ==="
  cargo test --test "$name" -p colmena_dag_engine 2>&1 | tail -3
done
```

Expected: each test file's suite PASS. If any test file is HTML/documents-related, it should still pass (we didn't touch HTML).

- [ ] **Step 3: Format + lint**

```bash
cargo fmt --manifest-path src/libs/colmena/Cargo.toml --all -- --check; echo "fmt: $?"
cargo clippy --manifest-path src/libs/colmena/Cargo.toml --lib -- -D warnings 2>&1 | tail -3
```

Expected: both exit 0 and no errors.

- [ ] **Step 4: Hexagonal compliance script (from HTML PR, should still apply)**

```bash
./scripts/check_hexagonal_documents.sh
```

Expected: ✅.

- [ ] **Step 5: Build with gcs feature (sanity)**

```bash
cargo build --lib -p colmena_dag_engine --features gcs 2>&1 | tail -3
```

Expected: success.

No commit; this is verification only.

### Task F.2: Push branch + open PR

**Files:** none

- [ ] **Step 1: Verify branch state**

```bash
cd /Users/danielgarcia/startti/colmena/.claude/worktrees/revert-layered
git log --oneline origin/develop..HEAD
git branch --show-current
```

Expected: ~10 commits, branch `feature/revert-layered-skills`.

- [ ] **Step 2: Push**

```bash
git push -u origin feature/revert-layered-skills 2>&1 | tail -5
```

- [ ] **Step 3: Create PR**

```bash
gh pr create --base develop --head feature/revert-layered-skills --title "feat(skills): revert layered-tool-context + recursive references + skills_path" --body "$(cat <<'EOF'
## Summary

Reverts the layered-tool-context feature (layers 1a + 1b + 2 + plumbing) — leaving only `llm_call.skills` + `load_skill` as the mechanism for getting content to the LLM. Adds two replacement features:

- **Recursive references** — files under `references/` can declare their own `references:` frontmatter. `load_reference("skill", "path/to/sub")` navigates the tree (max depth 5; cycles rejected).
- **`skills_path` / `skills_paths` on `llm_call`** — load all skills under a directory without enumerating by name. Coexists with the existing `skills: [...]` array (union by name, dedup).

## Why

The 3-layer model was overengineering:
- Magic-by-convention (`node_type:` frontmatter that auto-attaches) is fragile across renames.
- Auto-injected content violates the "LLM elige qué cargar" principle.
- One mechanism (skills referenced explicitly + recursive nesting) covers all the use cases.

## Breaking changes

| What | Migration |
|---|---|
| Skill frontmatter `node_type:` | Remove the field. Reference the skill from `llm_call.skills` instead. |
| `tool_configuration.skills` | Move names to `llm_call.skills`. |
| `ExecutableNode::tool_description_supplement` | Author a skill markdown describing the policy. Validators still enforce at runtime. |
| Built-in `sql_query-guide` skill | Removed. If desired, author it as a path-based skill. |

See full migration guide: `docs/CHANGELOG_2026-05.md`.

## Test plan

- [x] `cargo test --lib -p colmena_dag_engine` → green
- [x] All integration test files → green
- [x] `cargo fmt -- --check` → clean
- [x] `cargo clippy -- -D warnings` → clean
- [x] `cargo build --features gcs` → clean
- [x] Hexagonal compliance script → green
- [ ] Reviewer: confirm legacy graphs (with `tool_configuration.skills` or `node_type`) fail to load with clear migration errors.

## References

- Design: [docs/superpowers/specs/2026-05-31-revert-layered-tool-context-design.md](docs/superpowers/specs/2026-05-31-revert-layered-tool-context-design.md)
- Plan: [docs/superpowers/plans/2026-05-31-revert-layered-tool-context.md](docs/superpowers/plans/2026-05-31-revert-layered-tool-context.md)
- Supersedes: [docs/superpowers/specs/2026-05-29-layered-tool-context-design.md](docs/superpowers/specs/2026-05-29-layered-tool-context-design.md)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)" 2>&1 | tail -5
```

- [ ] **Step 4: Watch CI**

```bash
gh pr checks --watch $(gh pr view --json number --jq .number) 2>&1 | tail -10
```

Expected: all checks pass. If `Validate Commit Messages` fails, the offending commit's message format needs adjustment.

---

## Coverage check vs spec

| Spec section | Covered by task(s) |
|---|---|
| §3 Goals (revert layers 1a, 1b, 2) | Tasks A.1, A.2, A.3, A.4 |
| §4.1 Código a borrar (tool_context.rs, etc.) | Task A.1 (auto-deleted by revert) |
| §4.1 Borrar sql_query-guide | Task A.4 (auto-deleted by revert d40e5bc) |
| §4.1 Borrar e2e graphs layered | Task B.1 |
| §4.2 Parser permite references frontmatter en reference files | Task C.2 |
| §4.2 load_reference acepta path | Task C.4 |
| §4.2 Validar ciclos | Task C.3 |
| §4.2 llm_call acepta skills_path / skills_paths | Tasks D.1, D.2 |
| §5.1 ReferenceMeta nested | Task C.1 |
| §5.3 llm_call config nuevo schema | Task D.1 |
| §5.4 load_reference synthetic tool con path | Task C.4 |
| §6 Validaciones (cycle, depth=5, missing path) | Tasks C.3, D.2 |
| §6 Hard error for legacy node_type frontmatter | Task B.2 |
| §9 Migración (errores claros) | Tasks A.x (revert) + B.2 (legacy detector) |
| §10 Tests | Tasks C.1-C.4, D.1, D.2 + F.1 regression |
| §12 Docs (24_skills.md, superseded, CHANGELOG) | Tasks E.1, E.2 |

**No spec section unmapped.**

---

## Notes for executors

- The revert phases (A) may produce conflicts if a layered commit was edited by a later non-layered commit. If so, `git status` will show `UU` files. Inspect each, prefer the post-revert state (the layered code REMOVED), `git add`, continue.
- If a revert removes a function that's still referenced by other code we don't expect to touch (e.g., a stray import), remove the stale references during the same task — note in the commit message.
- `cargo clippy` may flag warnings introduced by code that was modified by revert (e.g., unused imports). Fix them in the same task as the revert; the commit message should mention "+ remove stale imports".
- All test code in the plan uses placeholder names like `PathSkillRepository::load`, `LlmNode::resolve_skill_names`, `SkillError::ReferenceCycle`. If the actual codebase uses different names, ADAPT the test code while preserving the test intent. Report the adaptation in your task report.
