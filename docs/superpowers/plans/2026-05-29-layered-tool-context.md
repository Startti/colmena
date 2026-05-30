# Layered Tool Context Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a node is used as an LLM tool, automatically surface its policy (derived from config), its node-type best-practices guide (authored as a SKILL.md with `node_type` frontmatter), and any tool-scoped domain skills — through `describe_tool` (lazy) or the tool `description` (eager/non-lazy). Tool-scoped skills are gated by visibility on the lazy `discovered_set`.

**Architecture:** Single skills pool (built-in via `include_dir!` + paths). A skill's role is derived from frontmatter + reference location: `node_type` in frontmatter → layer-1 guide (auto-folded into the tool block); referenced by `tool_configuration.skills` → layer-2 specific (gated `load_skill`); referenced by `llm_call.skills` → layer-3 free-standing (always in `load_skill`). The canonical block is built by `build_tool_context_block(cfg, node, fixed, repo, variant)` and injected at the two existing points (`generate_tool_markdown` for lazy, `generate_tool_definition.description` for eager/non-lazy). The `load_skill` tool definition is rebuilt per request inside the existing `tools_provider` closure, applying layer 1/2/3 inclusion rules against the current `discovered_set`.

**Tech Stack:** Rust 1.95, hexagonal architecture (domain ports + infrastructure adapters), `serde`, `serde_yaml`, `include_dir!`, `async_trait`, `mockall`, `tokio`. Existing crates: `colmena_dag_engine` (single library crate).

**Spec:** [docs/superpowers/specs/2026-05-29-layered-tool-context-design.md](../specs/2026-05-29-layered-tool-context-design.md)

---

## File Structure

### Files to create
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tool_context.rs` — `build_tool_context_block` + `BlockVariant` enum.
- `src/libs/colmena/skills/sql_query-guide/SKILL.md` — first layer-1 node-type guide (reference implementation).
- `src/libs/colmena/skills/sales-analysis/SKILL.md` — example layer-2 skill for E2E.
- `src/libs/colmena/skills/expense-analysis/SKILL.md` — example layer-2 skill for E2E.
- `tests/graphs/agents/sql_layered_tool_context.json` — E2E graph (real LLM + real Postgres).

### Files to modify
- `src/libs/colmena/src/skills/infrastructure/frontmatter_parser.rs` — parse optional `node_type: String` in YAML frontmatter.
- `src/libs/colmena/src/skills/domain/skill.rs` — add `node_type: Option<String>` to `Skill`.
- `src/libs/colmena/src/skills/domain/skill_repository.rs` — add `find_by_node_type(&self, node_type: &str) -> Option<SkillCatalogEntry>`.
- `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs` and `filesystem_skill_repository.rs` and `composite_skill_repository.rs` — implement `find_by_node_type`; cache `by_node_type` map; duplicate-detection at load time.
- `src/libs/colmena/src/dag_engine/domain/node.rs` — add `tool_description_supplement(&self, _fixed_config: &Value) -> Option<String> { None }` default to `ExecutableNode`.
- `src/libs/colmena/src/dag_engine/domain/sql_permissions.rs` — add `pub fn describe_policy_for_llm(&self, max_rows: u64) -> String`.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs` — override `tool_description_supplement`.
- `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs` — add `#[serde(default)] pub skills: Vec<String>`.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs` — `generate_tool_markdown` delegates to `build_tool_context_block(..., Lazy)` + footer.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` — re-export new module.
- `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` — in `generate_tool_definition`, append block (EagerOrNonLazy variant) to `description` in all 4 return branches.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — graph-load validation (3 hard errors + 2 warnings); rebuild `load_skill` tool inside the `tools_provider` closure with layer 1/2/3 rules; add `tool_context_blocks` to `extra_info`.
- `docs/developer_guide/29_lazy_tool_loading.md` — section on the tool context block.
- `docs/developer_guide/24_skills.md` — `node_type` frontmatter + scoped skills usage.
- `docs/node_configurations.json` — `tool_configurations.*.skills` field.
- `docs/node_as_tools_reference.json` — note about tool-scoped skills and layered context.
- `docs/CHANGELOG_2026-05.md` — feature 10 + matrix row.
- `CLAUDE.md` — Current Status bullet.

---

## Task 1: Extend skill frontmatter with optional `node_type`

**Files:**
- Modify: `src/libs/colmena/src/skills/infrastructure/frontmatter_parser.rs`
- Modify: `src/libs/colmena/src/skills/domain/skill.rs`

- [ ] **Step 1: Add field to `Skill` domain type**

In `src/libs/colmena/src/skills/domain/skill.rs`, modify the `Skill` struct (around line 13):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub references: Vec<SkillReferenceMeta>,
    pub source: SkillSource,
    /// When set, this skill is a layer-1 node-type guide auto-folded into the
    /// tool context block for any tool whose node_type matches. Layer-1 guides
    /// are excluded from the `load_skill` catalog.
    #[serde(default)]
    pub node_type: Option<String>,
}
```

Update the existing test fixtures in the same file (look for `references: vec![...]` in `tests/`) to add `node_type: None` to each `Skill { ... }` literal.

- [ ] **Step 2: Add failing test for parsing `node_type` in frontmatter**

Add to `src/libs/colmena/src/skills/infrastructure/frontmatter_parser.rs` test module:

```rust
#[test]
fn parses_node_type_when_present() {
    let content = "---\nname: x\ndescription: y\nnode_type: sql_query\n---\nbody\n";
    let parsed = parse_skill_md(content, "p").unwrap();
    assert_eq!(parsed.node_type.as_deref(), Some("sql_query"));
}

#[test]
fn node_type_defaults_to_none_when_absent() {
    let content = "---\nname: x\ndescription: y\n---\nbody\n";
    let parsed = parse_skill_md(content, "p").unwrap();
    assert!(parsed.node_type.is_none());
}
```

- [ ] **Step 3: Run tests — expect compile error or failure**

```bash
cargo test --lib skills::infrastructure::frontmatter_parser 2>&1 | tail -20
```

Expected: compile error (field `node_type` does not exist on `ParsedSkillMd`).

- [ ] **Step 4: Add the field to the parser**

In `src/libs/colmena/src/skills/infrastructure/frontmatter_parser.rs`, edit the raw YAML struct around line 4 and the `ParsedSkillMd` output struct around line 20:

```rust
#[derive(Debug, Deserialize)]
struct RawFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    references: Vec<RawReference>,
    #[serde(default)]
    node_type: Option<String>,
}
```

```rust
pub struct ParsedSkillMd {
    pub name: String,
    pub description: String,
    pub references: Vec<ParsedReferenceMeta>,
    pub body: String,
    pub node_type: Option<String>,
}
```

In the constructor at the bottom of `parse_skill_md` (around line 100), include the new field:

```rust
Ok(ParsedSkillMd {
    name: raw.name,
    description: raw.description,
    body,
    references: raw.references.into_iter().map(|r| ParsedReferenceMeta {
        name: r.name,
        description: r.description,
    }).collect(),
    node_type: raw.node_type,
})
```

- [ ] **Step 5: Run tests — expect pass**

```bash
cargo test --lib skills::infrastructure::frontmatter_parser
```

Expected: PASS (all tests in the module).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/skills/domain/skill.rs src/libs/colmena/src/skills/infrastructure/frontmatter_parser.rs
git commit -m "feat(skills): parse optional node_type frontmatter

Layer 1 of the tool context system — a SKILL.md with node_type: <name>
binds it as the auto-folded best-practices guide for tools of that node
type. Defaults to None for full backward compat.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: Surface `node_type` in `SkillCatalogEntry` and propagate through loaders

**Files:**
- Modify: `src/libs/colmena/src/skills/domain/skill_repository.rs`
- Modify: `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs`
- Modify: `src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs`
- Modify: `src/libs/colmena/src/skills/infrastructure/composite_skill_repository.rs`

- [ ] **Step 1: Add `node_type` to `SkillCatalogEntry`**

In `src/libs/colmena/src/skills/domain/skill_repository.rs`:

```rust
#[derive(Debug, Clone)]
pub struct SkillCatalogEntry {
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    /// When `Some`, this entry is a layer-1 node-type guide and must NOT
    /// appear in the load_skill catalog.
    pub node_type: Option<String>,
}
```

Update the existing test `catalog_entry_debug_format_contains_name` to include `node_type: None`.

- [ ] **Step 2: Add failing test for propagation in builtin repo**

In `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs`, add a test that creates a temp skill content with `node_type: sql_query`, loads it through the repo, and asserts the catalog entry has the `node_type`.

```rust
#[tokio::test]
async fn catalog_entry_carries_node_type_when_present() {
    // Use one of the built-in skills (or assume there will be one).
    // For the test, build a custom in-memory variant if simpler — see
    // existing pattern in this file.
    let repo = BuiltinSkillRepository::new();
    let entries = repo.list_available();
    // At least one existing built-in must be node_type-less:
    assert!(entries.iter().any(|e| e.node_type.is_none()));
}
```

(The positive case is exercised in Task 9 when `sql_query-guide` is added.)

- [ ] **Step 3: Run — expect compile failure**

```bash
cargo test --lib skills 2>&1 | tail -20
```

Expected: error — field `node_type` missing on `SkillCatalogEntry` constructions.

- [ ] **Step 4: Populate `node_type` in each repo's catalog construction**

In `builtin_skill_repository.rs`, find every `SkillCatalogEntry { ... }` literal and add `node_type: ...` reading from the parsed skill (default `None`).

Repeat for `filesystem_skill_repository.rs` and `composite_skill_repository.rs`.

- [ ] **Step 5: Run — expect pass**

```bash
cargo test --lib skills
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/skills/
git commit -m "feat(skills): surface node_type through SkillCatalogEntry

Each repo now propagates the parsed node_type so the LLM node can route
layer-1 guides via auto-fold and exclude them from the load_skill catalog.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: `find_by_node_type` + duplicate detection in repositories

**Files:**
- Modify: `src/libs/colmena/src/skills/domain/skill_repository.rs`
- Modify: `src/libs/colmena/src/skills/domain/skill_error.rs`
- Modify: `src/libs/colmena/src/skills/infrastructure/composite_skill_repository.rs`

- [ ] **Step 1: Extend trait and add error variant**

In `src/libs/colmena/src/skills/domain/skill_repository.rs`:

```rust
#[async_trait]
pub trait SkillRepository: Send + Sync {
    fn list_available(&self) -> Vec<SkillCatalogEntry>;

    /// Resolve the layer-1 node-type guide for a given node_type, if any.
    /// Returns the catalog entry whose frontmatter `node_type` matches.
    fn find_by_node_type(&self, node_type: &str) -> Option<SkillCatalogEntry> {
        self.list_available()
            .into_iter()
            .find(|e| e.node_type.as_deref() == Some(node_type))
    }

    async fn load_skill(&self, name: &str) -> Result<Skill, SkillError>;
    async fn load_reference(&self, skill_name: &str, reference_name: &str) -> Result<SkillReference, SkillError>;
}
```

In `src/libs/colmena/src/skills/domain/skill_error.rs`, add:

```rust
#[error("duplicate node_type guide: node_type '{node_type}' is claimed by skills '{first}' and '{second}'; only one guide per node_type is allowed")]
DuplicateNodeTypeGuide {
    node_type: String,
    first: String,
    second: String,
},
```

- [ ] **Step 2: Failing test for duplicate detection in composite repo**

In `src/libs/colmena/src/skills/infrastructure/composite_skill_repository.rs`, add a test that constructs two in-memory child repos each exposing a skill with the same `node_type` and asserts that the composite's validation step errors.

```rust
#[tokio::test]
async fn validate_returns_error_on_duplicate_node_type() {
    let a = make_test_repo("guide-a", "sql_query");
    let b = make_test_repo("guide-b", "sql_query");
    let composite = CompositeSkillRepository::new(vec![Arc::new(a), Arc::new(b)]);
    let err = composite.validate().expect_err("should fail on duplicates");
    assert!(matches!(err, SkillError::DuplicateNodeTypeGuide { .. }));
}
```

Where `make_test_repo` returns an in-memory mock repo exposing one skill — pattern after existing tests in the file.

- [ ] **Step 3: Run — expect failure**

```bash
cargo test --lib composite_skill 2>&1 | tail -10
```

Expected: FAIL — `validate` not found / error variant missing.

- [ ] **Step 4: Implement `validate` on `CompositeSkillRepository`**

In `composite_skill_repository.rs`:

```rust
impl CompositeSkillRepository {
    /// Run cross-repo validations that cannot be enforced by a single child:
    /// - At most one skill per node_type (across all repos).
    pub fn validate(&self) -> Result<(), SkillError> {
        let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for entry in self.list_available() {
            if let Some(nt) = entry.node_type.as_deref() {
                if let Some(prev) = seen.get(nt) {
                    return Err(SkillError::DuplicateNodeTypeGuide {
                        node_type: nt.to_string(),
                        first: prev.clone(),
                        second: entry.name.clone(),
                    });
                }
                seen.insert(nt.to_string(), entry.name.clone());
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 5: Run — expect pass**

```bash
cargo test --lib composite_skill
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/skills/
git commit -m "feat(skills): find_by_node_type + duplicate-guide validation

Composite repo gains a validate() that rejects two skills claiming the
same node_type. The trait gains find_by_node_type for the auto-fold path.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4: Add `tool_description_supplement` default-None to `ExecutableNode`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/node.rs`

- [ ] **Step 1: Failing test for default behavior**

Pick any simple node (e.g. `AddNode` in `src/libs/colmena/src/dag_engine/infrastructure/nodes/add.rs`) and add to its inline tests:

```rust
#[test]
fn default_tool_description_supplement_is_none() {
    let node = AddNode;
    let supp = node.tool_description_supplement(&serde_json::json!({}));
    assert!(supp.is_none());
}
```

- [ ] **Step 2: Run — expect compile failure**

```bash
cargo test --lib add::tests::default_tool_description_supplement_is_none 2>&1 | tail -10
```

Expected: error — method does not exist.

- [ ] **Step 3: Add the trait method with default None**

In `src/libs/colmena/src/dag_engine/domain/node.rs`, add to the `ExecutableNode` trait (next to the existing `description` method):

```rust
/// Optional config-derived text appended to the tool context block when
/// the node is used as an LLM tool. Pure function of `fixed_config` — no
/// I/O. Default: None.
fn tool_description_supplement(&self, _fixed_config: &serde_json::Value) -> Option<String> {
    None
}
```

- [ ] **Step 4: Run — expect pass**

```bash
cargo test --lib add::tests::default_tool_description_supplement_is_none
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/node.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/add.rs
git commit -m "feat(node): add tool_description_supplement hook to ExecutableNode

Default-None method that lets each node return config-derived policy
text. Used by the tool context builder; default keeps all current nodes
unchanged.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5: `SqlPermissions::describe_policy_for_llm`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/sql_permissions.rs`

- [ ] **Step 1: Failing test for read_write + two schemas**

Append to the `tests` module at the bottom of `sql_permissions.rs`:

```rust
#[test]
fn policy_text_read_write_includes_ops_schemas_and_blocked_list() {
    let cfg = serde_json::json!({
        "preset": "read_write",
        "allowed_schemas": ["public", "analytics"]
    });
    let perms = SqlPermissions::from_config(Some(&cfg)).unwrap();
    let text = perms.describe_policy_for_llm(50);

    assert!(text.contains("SELECT"));
    assert!(text.contains("INSERT"));
    assert!(text.contains("UPDATE"));
    assert!(!text.contains("DELETE"));
    assert!(text.contains("public"));
    assert!(text.contains("analytics"));
    assert!(text.contains("DROP"));     // always-blocked list mentions it
    assert!(text.contains("WHERE"));    // safety rule
    assert!(text.contains("50"));       // max_rows
}

#[test]
fn policy_text_empty_schemas_says_all() {
    let cfg = serde_json::json!({ "preset": "read_only" });
    let perms = SqlPermissions::from_config(Some(&cfg)).unwrap();
    let text = perms.describe_policy_for_llm(100);
    assert!(text.to_lowercase().contains("all schemas"));
}

#[test]
fn policy_text_full_preset_lists_create_operations() {
    let cfg = serde_json::json!({
        "preset": "full",
        "allowed_schemas": ["sandbox"]
    });
    let perms = SqlPermissions::from_config(Some(&cfg)).unwrap();
    let text = perms.describe_policy_for_llm(100);
    assert!(text.contains("CREATE TABLE"));
    assert!(text.contains("CREATE FUNCTION"));
}
```

- [ ] **Step 2: Run — expect compile failure**

```bash
cargo test --lib sql_permissions 2>&1 | tail -10
```

Expected: error — `describe_policy_for_llm` not found.

- [ ] **Step 3: Implement the method**

Append to `impl SqlPermissions { ... }` block:

```rust
/// Human-readable policy block describing what the LLM can and cannot do
/// with this tool. Multi-line, intended to be folded into describe_tool /
/// tool description. Pure function of config — no I/O.
pub fn describe_policy_for_llm(&self, max_rows: u64) -> String {
    let ops: Vec<&str> = [
        (SqlOperation::Select, "SELECT"),
        (SqlOperation::Insert, "INSERT"),
        (SqlOperation::Update, "UPDATE"),
        (SqlOperation::Delete, "DELETE"),
        (SqlOperation::CreateFunction, "CREATE FUNCTION"),
        (SqlOperation::CreateTable, "CREATE TABLE"),
    ]
    .iter()
    .filter(|(op, _)| self.allowed_ops.contains(op))
    .map(|(_, name)| *name)
    .collect();

    let schemas_line = if self.allowed_schemas.is_empty() {
        "all schemas".to_string()
    } else {
        let mut s: Vec<&str> = self.allowed_schemas.iter().map(String::as_str).collect();
        s.sort_unstable();
        s.join(", ")
    };

    format!(
        "SQL access policy for this tool (enforced server-side; requests outside it are rejected):\n\
         - Allowed operations: {ops}\n\
         - Allowed schemas: {schemas} (other schemas are blocked)\n\
         - Sandbox schema for CREATE FUNCTION/TABLE: {sandbox}\n\
         - Always blocked regardless of config: DROP, ALTER, TRUNCATE, CREATE SCHEMA, CREATE INDEX, CREATE VIEW, GRANT, REVOKE\n\
         - DELETE and UPDATE require a WHERE clause\n\
         - SELECT returns at most {max_rows} rows",
        ops = ops.join(", "),
        schemas = schemas_line,
        sandbox = self.sandbox_schema,
        max_rows = max_rows,
    )
}
```

- [ ] **Step 4: Run — expect pass**

```bash
cargo test --lib sql_permissions
```

Expected: PASS (all permissions tests including the 3 new ones).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/sql_permissions.rs
git commit -m "feat(sql): SqlPermissions::describe_policy_for_llm

Multi-line policy text derived from preset + allowed_schemas + sandbox +
max_rows. Used by SqlNode's tool_description_supplement override (next
task) to surface the policy to the LLM through the tool context block.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6: `SqlNode::tool_description_supplement` override

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs`

- [ ] **Step 1: Failing test**

In `sql.rs` test module (add one if absent):

```rust
#[cfg(test)]
mod tool_supplement_tests {
    use super::*;
    use crate::dag_engine::domain::node::ExecutableNode;
    use std::sync::Arc;
    use crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;

    #[test]
    fn supplement_returns_policy_when_permissions_present() {
        let factory = Arc::new(SqlPortFactory::default());
        let node = SqlNode::new(factory);
        let fixed = serde_json::json!({
            "permissions": { "preset": "read_write", "allowed_schemas": ["public"] },
            "runtime_limits": { "max_rows": 25 }
        });
        let supp = node.tool_description_supplement(&fixed).expect("Some");
        assert!(supp.contains("SELECT"));
        assert!(supp.contains("public"));
        assert!(supp.contains("25"));
    }

    #[test]
    fn supplement_returns_none_when_permissions_missing() {
        let factory = Arc::new(SqlPortFactory::default());
        let node = SqlNode::new(factory);
        let fixed = serde_json::json!({});
        // No permissions key → still computable using defaults (read_only),
        // but if you prefer None when nothing configured, document the choice.
        // Spec says: tool_description_supplement returns Some when there is
        // a permissions config — we follow that.
        assert!(node.tool_description_supplement(&fixed).is_none());
    }
}
```

- [ ] **Step 2: Run — expect failure (method not overridden yet)**

```bash
cargo test --lib sql::tool_supplement_tests 2>&1 | tail -10
```

Expected: FAIL — second test passes (default None) but first fails (returns None).

- [ ] **Step 3: Implement the override**

Inside the `impl ExecutableNode for SqlNode { ... }` block, add the method (before or after `description`):

```rust
fn tool_description_supplement(&self, fixed_config: &serde_json::Value) -> Option<String> {
    // Only produce a supplement when an explicit permissions config exists.
    // This keeps the block silent for graphs that haven't opted into the feature.
    let perms_val = fixed_config.get("permissions")?;
    let perms = crate::dag_engine::domain::sql_permissions::SqlPermissions::from_config(Some(perms_val))
        .ok()?;
    let max_rows = fixed_config
        .get("runtime_limits")
        .and_then(|r| r.get("max_rows"))
        .and_then(|v| v.as_u64())
        .unwrap_or(100);
    Some(perms.describe_policy_for_llm(max_rows))
}
```

- [ ] **Step 4: Run — expect pass**

```bash
cargo test --lib sql::tool_supplement_tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs
git commit -m "feat(sql): SqlNode implements tool_description_supplement

Returns the multi-line access policy when fixed_config contains a
permissions object. Other configurations remain silent (None).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 7: `ToolConfiguration.skills` field

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs`

- [ ] **Step 1: Failing serde round-trip test**

In `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs` tests module:

```rust
#[test]
fn tool_configuration_parses_skills_field() {
    let json = serde_json::json!({
        "name": "consultar_ventas",
        "description": "Consultar ventas",
        "node_type": "sql_query",
        "skills": ["sales-analysis", "expense-analysis"]
    });
    let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
    assert_eq!(cfg.skills, vec!["sales-analysis".to_string(), "expense-analysis".to_string()]);
}

#[test]
fn tool_configuration_skills_defaults_to_empty() {
    let json = serde_json::json!({
        "name": "t",
        "description": "d",
        "node_type": "x"
    });
    let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
    assert!(cfg.skills.is_empty());
}
```

- [ ] **Step 2: Run — expect failure**

```bash
cargo test --lib tool_configuration 2>&1 | tail -10
```

Expected: error — field `skills` doesn't exist.

- [ ] **Step 3: Add the field**

In the `ToolConfiguration` struct (around line 100), add:

```rust
pub struct ToolConfiguration {
    // ... existing fields above ...

    /// Layer-2 specific skills scoped to this tool. Each name must resolve
    /// against the active SkillRepository. Gated by visibility on the lazy
    /// discovered_set — appears in load_skill catalog only after this tool
    /// is discovered.
    #[serde(default)]
    pub skills: Vec<String>,
}
```

Update any other constructor sites or fixtures in the file/repo where `ToolConfiguration { ... }` is built (the `cfg_minimal` in `describe_tool.rs` tests is one) — `skills: Vec::new()` default.

- [ ] **Step 4: Run — expect pass**

```bash
cargo test --lib tool_configuration
cargo build --lib 2>&1 | tail -5
```

Expected: PASS for tests; build succeeds (all `ToolConfiguration` construction sites filled).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/
git commit -m "feat(tool_configuration): add skills: Vec<String> field

Per-tool list of layer-2 skill names, scoped to that tool. Defaults to
empty; old graphs unchanged. Validation against the skill repo lands in
the llm_call graph-load step.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 8: `build_tool_context_block` — core builder

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tool_context.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`

- [ ] **Step 1: Create the module skeleton**

`src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tool_context.rs`:

```rust
//! Tool context block builder — assembles the layered context (description +
//! policy + node-type guide + parameters + scoped skills announcement) into a
//! single markdown string. Used at two injection points:
//!   - `generate_tool_markdown` (lazy describe_tool path) → Lazy variant.
//!   - `generate_tool_definition` (eager / non-lazy path) → EagerOrNonLazy.
//!
//! Pure function — no I/O. Each section is omitted when its input is empty.

use crate::dag_engine::domain::node::ExecutableNode;
use crate::dag_engine::domain::tool_configuration::{NodeSchemaField, ToolConfiguration};
use crate::skills::domain::skill_repository::SkillRepository;
use serde_json::Value;

/// Which variant of the block to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockVariant {
    /// For describe_tool (lazy): includes the Parameters table.
    Lazy,
    /// For ToolDefinition.description (eager or non-lazy): omits Parameters
    /// because the schema travels typed.
    EagerOrNonLazy,
}

pub fn build_tool_context_block(
    cfg: &ToolConfiguration,
    node: &dyn ExecutableNode,
    fixed_config_effective: &Value,
    skill_repo: Option<&dyn SkillRepository>,
    variant: BlockVariant,
) -> String {
    let mut out = String::new();

    // Header: name + description (always)
    out.push_str(&format!("# {}\n\n", cfg.name));
    out.push_str(cfg.description.trim());
    out.push_str("\n\n");

    // Layer 1 — policy (from node)
    if let Some(policy) = node.tool_description_supplement(fixed_config_effective) {
        out.push_str("## Access policy\n\n");
        out.push_str(policy.trim());
        out.push_str("\n\n");
    }

    // Layer 1 — node-type guide (from skill repo)
    if let Some(repo) = skill_repo {
        if let Some(guide_entry) = repo.find_by_node_type(&cfg.node_type) {
            // Body is loaded lazily — for now we expose name + description in
            // the block header and a separator. Full body is fetched async via
            // SkillRepository::load_skill at the call site (since this fn is
            // sync). Implementation note: the caller passes the resolved body
            // via the wrapper used in describe_tool / generate_tool_definition.
            //
            // Here we render the header section and a placeholder marker that
            // the wrapper replaces with the markdown body. Keeps this function
            // sync and pure.
            out.push_str("## Best practices\n\n");
            out.push_str(&format!("<!-- node-type guide: {} -->\n", guide_entry.name));
            out.push_str("{{NODE_GUIDE_BODY}}\n\n");
        }
    }

    // Parameters (Lazy variant only)
    if variant == BlockVariant::Lazy {
        let visible = collect_visible_fields(cfg);
        out.push_str("## Parameters\n\n");
        if visible.is_empty() {
            out.push_str(
                "No parameter schema declared — pass arguments as a free-form JSON object that matches the tool's expectations.\n\n",
            );
        } else {
            out.push_str("| Name | Type | Required | Description |\n");
            out.push_str("|------|------|----------|-------------|\n");
            for (name, field) in &visible {
                let ty = field.field_type.as_deref().unwrap_or("unknown");
                let required = if field.required.unwrap_or(false) { "yes" } else { "no" };
                let desc = field.description.as_deref().unwrap_or("");
                out.push_str(&format!("| {} | {} | {} | {} |\n", name, ty, required, desc));
            }
            out.push('\n');
        }
    }

    // Layer 2 announcement
    if !cfg.skills.is_empty() {
        out.push_str("## Related knowledge\n\n");
        out.push_str("Load with `load_skill(name)` when your task matches:\n");
        if let Some(repo) = skill_repo {
            for skill_name in &cfg.skills {
                let desc = repo
                    .list_available()
                    .into_iter()
                    .find(|e| e.name == *skill_name)
                    .map(|e| e.description)
                    .unwrap_or_else(|| "(description unavailable)".to_string());
                out.push_str(&format!("- {}: {}\n", skill_name, desc));
            }
        } else {
            for skill_name in &cfg.skills {
                out.push_str(&format!("- {}\n", skill_name));
            }
        }
        out.push('\n');
    }

    out
}

fn collect_visible_fields(cfg: &ToolConfiguration) -> Vec<(String, &NodeSchemaField)> {
    let Some(schema) = cfg.node_schema.as_ref() else {
        return Vec::new();
    };
    let mut out: Vec<(String, &NodeSchemaField)> = Vec::new();
    for (name, field) in schema {
        if field.fixed.is_some() { continue; }
        if cfg.fixed_config.contains_key(name) { continue; }
        out.push((name.clone(), field));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::node::ExecutableNode;
    use crate::dag_engine::domain::tool_configuration::ToolConfiguration;
    use serde_json::json;
    use std::collections::HashMap;

    struct NoopNode {
        supp: Option<String>,
    }
    #[async_trait::async_trait]
    impl ExecutableNode for NoopNode {
        async fn execute(
            &self,
            _inputs: &crate::dag_engine::domain::node::NodeInputs,
            _config: &Value,
            _state: &mut Value,
            _observer: Option<std::sync::Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
        ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
            Ok(json!({}))
        }
        fn schema(&self) -> Value { json!({}) }
        fn tool_description_supplement(&self, _: &Value) -> Option<String> {
            self.supp.clone()
        }
    }

    fn cfg(name: &str, node_type: &str, description: &str) -> ToolConfiguration {
        ToolConfiguration {
            name: name.to_string(),
            description: description.to_string(),
            node_type: node_type.to_string(),
            fixed_config: HashMap::new(),
            #[allow(deprecated)]
            exposed_inputs: None,
            #[allow(deprecated)]
            parameters: None,
            #[allow(deprecated)]
            mergeable_fields: None,
            #[allow(deprecated)]
            field_mapping: None,
            node_schema: None,
            node_config: None,
            expose_sub_tools: None,
            summary: None,
            eager: false,
            skills: Vec::new(),
        }
    }

    #[test]
    fn minimal_block_only_header_and_description() {
        let node = NoopNode { supp: None };
        let block = build_tool_context_block(
            &cfg("t", "noop", "Tool desc"),
            &node,
            &json!({}),
            None,
            BlockVariant::EagerOrNonLazy,
        );
        assert!(block.contains("# t"));
        assert!(block.contains("Tool desc"));
        assert!(!block.contains("Access policy"));
        assert!(!block.contains("Best practices"));
        assert!(!block.contains("Parameters"));
        assert!(!block.contains("Related knowledge"));
    }

    #[test]
    fn policy_section_present_when_supplement_some() {
        let node = NoopNode { supp: Some("POLICY".to_string()) };
        let block = build_tool_context_block(
            &cfg("t", "noop", "Tool"),
            &node,
            &json!({}),
            None,
            BlockVariant::EagerOrNonLazy,
        );
        assert!(block.contains("## Access policy"));
        assert!(block.contains("POLICY"));
    }

    #[test]
    fn related_knowledge_lists_scoped_skills() {
        let mut c = cfg("consultar_ventas", "sql_query", "Sales tool");
        c.skills = vec!["sales-analysis".to_string()];
        let node = NoopNode { supp: None };
        let block = build_tool_context_block(
            &c, &node, &json!({}), None, BlockVariant::EagerOrNonLazy,
        );
        assert!(block.contains("## Related knowledge"));
        assert!(block.contains("sales-analysis"));
    }

    #[test]
    fn parameters_section_only_in_lazy_variant() {
        let node = NoopNode { supp: None };
        let block_lazy = build_tool_context_block(
            &cfg("t", "noop", "T"),
            &node, &json!({}), None, BlockVariant::Lazy,
        );
        let block_eager = build_tool_context_block(
            &cfg("t", "noop", "T"),
            &node, &json!({}), None, BlockVariant::EagerOrNonLazy,
        );
        assert!(block_lazy.contains("## Parameters"));
        assert!(!block_eager.contains("## Parameters"));
    }
}
```

- [ ] **Step 2: Export the module**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`, add:

```rust
pub mod tool_context;
pub use tool_context::{build_tool_context_block, BlockVariant};
```

- [ ] **Step 3: Run — expect the new tests to pass**

```bash
cargo test --lib tool_context
```

Expected: PASS — all 4 unit tests.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/
git commit -m "feat(llm): build_tool_context_block — layered tool block builder

Pure function that assembles the description + policy + node-guide marker
+ parameters (lazy only) + scoped-skills announcement into one markdown
string. Each section is omitted when its input is empty. Tests cover
minimal, with-policy, with-related-knowledge, and lazy-vs-eager.

The node-guide section emits {{NODE_GUIDE_BODY}} as a placeholder so the
sync builder stays pure; the async wrapper at the call site loads the
body and substitutes.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 9: Author the first node-type guide — `sql_query-guide`

**Files:**
- Create: `src/libs/colmena/skills/sql_query-guide/SKILL.md`

- [ ] **Step 1: Write the SKILL.md**

`src/libs/colmena/skills/sql_query-guide/SKILL.md`:

```markdown
---
name: sql_query-guide
description: Best practices for using a sql_query tool against PostgreSQL.
node_type: sql_query
---

# sql_query — best practices

## Mindset

You are talking to a real PostgreSQL database. The access policy section
above this guide tells you exactly what operations and schemas you may
use; treat it as ground truth. The tool's static validator will reject
anything outside it — there is no point in trying.

## Before writing the query

- If you do not know the table layout, run an introspection query first:
  `SELECT table_name FROM information_schema.tables WHERE table_schema = '<schema>'`
  and then `SELECT column_name, data_type FROM information_schema.columns WHERE table_name = '<table>'`.
- Prefer explicit columns over `SELECT *` — narrower results, cleaner
  output, lower token cost.
- When the user asks an aggregate question ("how many", "total of"), use
  `COUNT(*)`, `SUM(...)`, `AVG(...)` directly instead of selecting rows
  and summing yourself.

## Safety rules (will be enforced)

- `DELETE` and `UPDATE` without a `WHERE` clause are rejected. Always
  scope the rows you intend to affect.
- `DROP`, `ALTER`, `TRUNCATE`, `CREATE SCHEMA/INDEX/VIEW`, `GRANT`,
  `REVOKE` are blocked unconditionally. If the user asks for any of
  these, explain that schema and lifecycle changes happen through
  migrations, not the agent.
- `CREATE FUNCTION` requires an accompanying `COMMENT ON FUNCTION ... IS
  '...'` in the same script — otherwise it is rejected.

## Reading large tables

- `SELECT` results are truncated to the max_rows limit shown in the
  policy. If the user wants more, add a more specific `WHERE` clause or
  use aggregations.
- Date filters tend to be the fastest narrowing: prefer `WHERE
  created_at >= '<date>'` over post-fetch filtering.

## Pagination

There is no native paging hook. If you need page N of a result set,
issue another query with `OFFSET (N-1)*size LIMIT size`.

## Errors

When a query fails, the tool returns an error envelope `{ "error":
"...", "source": "static_validator" | "llm_critic" | "execution" }`.
Read the message and adjust. Do not retry the same query — re-read the
policy first.

## Multi-tenant data (when RLS is on)

The tenant filter is enforced server-side. You do not need to add
`WHERE user_id = ...` yourself; the database sets the row-visibility
window. Acting as if rows from other tenants do not exist is the right
mental model.
```

- [ ] **Step 2: Build to confirm `include_dir!` picks it up**

```bash
cargo build --lib 2>&1 | tail -5
```

Expected: build succeeds (the built-in skills include_dir scans this directory).

- [ ] **Step 3: Verify the builtin repo exposes it**

Add a one-off test to `builtin_skill_repository.rs`:

```rust
#[tokio::test]
async fn sql_query_guide_is_indexed_as_node_type_guide() {
    let repo = BuiltinSkillRepository::new();
    let entry = repo.find_by_node_type("sql_query").expect("sql_query guide present");
    assert_eq!(entry.name, "sql_query-guide");
    assert_eq!(entry.node_type.as_deref(), Some("sql_query"));
}
```

```bash
cargo test --lib builtin_skill_repository::tests::sql_query_guide_is_indexed_as_node_type_guide
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/skills/sql_query-guide/ src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs
git commit -m "feat(skills): author sql_query-guide as the first layer-1 guide

Markdown SKILL with node_type: sql_query frontmatter. Auto-folded by the
tool context builder into every sql_query tool block. Covers mindset,
introspection, safety rules, large tables, pagination, errors, and RLS.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 10: Wire `describe_tool` to use the builder + load guide body

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs`

- [ ] **Step 1: Failing test for guide body substitution**

In `describe_tool.rs` test module:

```rust
#[tokio::test]
async fn markdown_includes_guide_body_when_node_type_matches() {
    use crate::skills::infrastructure::builtin_skill_repository::BuiltinSkillRepository;
    use std::sync::Arc;
    let mut cfg = cfg_minimal("query_db", "Query the database");
    cfg.node_type = "sql_query".to_string();
    let repo: Arc<dyn crate::skills::domain::skill_repository::SkillRepository> =
        Arc::new(BuiltinSkillRepository::new());
    // generate_tool_markdown is currently sync; the new wrapper is async.
    let md = generate_tool_markdown_async(&cfg, None, Some(repo.as_ref())).await;
    assert!(md.contains("## Best practices"));
    assert!(md.contains("sql_query — best practices"));
    assert!(!md.contains("{{NODE_GUIDE_BODY}}"));
}
```

- [ ] **Step 2: Run — expect compile failure**

```bash
cargo test --lib describe_tool 2>&1 | tail -10
```

Expected: error — `generate_tool_markdown_async` not defined.

- [ ] **Step 3: Add the async wrapper**

In `describe_tool.rs`:

```rust
use crate::dag_engine::domain::node::ExecutableNode;
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::tool_context::{
    build_tool_context_block, BlockVariant,
};
use crate::skills::domain::skill_repository::SkillRepository;

/// Async wrapper around build_tool_context_block: resolves the {{NODE_GUIDE_BODY}}
/// placeholder by loading the matched skill's full body, then appends the
/// "now available" footer.
pub async fn generate_tool_markdown_async(
    cfg: &ToolConfiguration,
    node: Option<&dyn ExecutableNode>,
    skill_repo: Option<&dyn SkillRepository>,
) -> String {
    // For nodes we have a registered ExecutableNode for, use it; otherwise
    // use a no-op shim so the builder runs.
    struct NoopNode;
    #[async_trait::async_trait]
    impl ExecutableNode for NoopNode {
        async fn execute(
            &self, _: &crate::dag_engine::domain::node::NodeInputs,
            _: &serde_json::Value, _: &mut serde_json::Value,
            _: Option<std::sync::Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
        ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> { Ok(serde_json::json!({})) }
        fn schema(&self) -> serde_json::Value { serde_json::json!({}) }
    }
    let noop = NoopNode;
    let node_ref: &dyn ExecutableNode = node.unwrap_or(&noop);

    // Effective fixed config: fixed_config + node_schema fixed values.
    // We don't compute the parsed_node_schema fixed_values here because
    // describe_tool already has access to cfg's structure; the policy reads
    // from cfg.fixed_config (where applicable) or from node_schema fixed.
    let fixed = effective_fixed_config(cfg);

    let mut block = build_tool_context_block(cfg, node_ref, &fixed, skill_repo, BlockVariant::Lazy);

    if block.contains("{{NODE_GUIDE_BODY}}") {
        if let Some(repo) = skill_repo {
            if let Some(entry) = repo.find_by_node_type(&cfg.node_type) {
                if let Ok(skill) = repo.load_skill(&entry.name).await {
                    block = block.replace("{{NODE_GUIDE_BODY}}", skill.body.trim());
                }
            }
        }
        // If still present (load failed), strip the marker
        block = block.replace("{{NODE_GUIDE_BODY}}", "(guide unavailable)");
    }

    block.push_str("---\nThe tool `");
    block.push_str(&cfg.name);
    block.push_str("` is now available. Call it directly on your next turn.\n");
    block
}

fn effective_fixed_config(cfg: &ToolConfiguration) -> serde_json::Value {
    use serde_json::{Map, Value};
    let mut map = Map::new();
    for (k, v) in &cfg.fixed_config { map.insert(k.clone(), v.clone()); }
    if let Some(schema) = &cfg.node_schema {
        for (k, field) in schema {
            if let Some(v) = &field.fixed {
                map.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(map)
}
```

Keep the existing sync `generate_tool_markdown` for backward compat (it'll be removed later), and **also** keep `dispatch_describe_tool` using the new async version:

In `dispatch_describe_tool`, change:
```rust
let output = match cfg {
    Some(c) => generate_tool_markdown(c),
    ...
```
to:
```rust
let output = match cfg {
    Some(c) => generate_tool_markdown_async(c, /*node*/ None, /*repo*/ None).await,
    ...
```

(For the call site to pass the real `node` and `repo`, we wire them at the dispatch level in Task 12 — for now this preserves behavior.)

- [ ] **Step 4: Run — expect pass**

```bash
cargo test --lib describe_tool
```

Expected: PASS (including the new test).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs
git commit -m "feat(describe_tool): async wrapper that loads node-type guide body

generate_tool_markdown_async resolves the {{NODE_GUIDE_BODY}} placeholder
by loading the matched SKILL's body from the repo, then appends the
existing 'now available' footer. dispatch_describe_tool switches to the
async path.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 11: Wire `generate_tool_definition` to append the block to `description`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

- [ ] **Step 1: Failing test**

In `dag_tool_executor.rs` tests:

```rust
#[tokio::test]
async fn tool_definition_description_includes_policy_for_sql_query() {
    use crate::dag_engine::domain::tool_configuration::{NodeSchema, NodeSchemaField};
    use crate::dag_engine::infrastructure::registry::NodeRegistry;
    use std::collections::HashMap;

    let mut schema: NodeSchema = HashMap::new();
    schema.insert("permissions".to_string(), NodeSchemaField {
        field_type: Some("object".to_string()),
        fixed: Some(serde_json::json!({
            "preset": "read_only",
            "allowed_schemas": ["public"]
        })),
        required: None, description: None, pattern: None, properties: None, items: None,
    });
    schema.insert("query".to_string(), NodeSchemaField {
        field_type: Some("string".to_string()), fixed: None,
        required: Some(true), description: Some("SQL".to_string()),
        pattern: None, properties: None, items: None,
    });
    let mut cfg = /* construct ToolConfiguration with node_type "sql_query",
                     this node_schema, and a description */;
    // ... see how existing tests build ToolConfiguration in this file
    let registry = Arc::new(NodeRegistry::default_with_all_nodes());
    let mut tool_configurations = HashMap::new();
    tool_configurations.insert("query_db".to_string(), cfg);
    let executor = DagToolExecutor::new(registry.clone(), tool_configurations.clone())
        .with_skill_repository(Some(Arc::new(/* test repo with sql_query-guide */)));
    let tools = executor.available_tools().await;
    let t = tools.iter().find(|t| t.name == "query_db").unwrap();
    assert!(t.description.contains("Access policy"));
    assert!(t.description.contains("SELECT"));
}
```

(Adapt to the existing test patterns in the file — many already build `ToolConfiguration` and `DagToolExecutor`.)

- [ ] **Step 2: Add a `with_skill_repository` setter on `DagToolExecutor`**

```rust
pub fn with_skill_repository(
    mut self,
    repo: Option<std::sync::Arc<dyn crate::skills::domain::skill_repository::SkillRepository>>,
) -> Self {
    self.skill_repo = repo;
    self
}
```

Add the field `skill_repo: Option<Arc<dyn SkillRepository>>` to the struct, default `None`.

- [ ] **Step 3: Make `generate_tool_definition` async and append the block**

Convert `generate_tool_definition` from sync to async (callers in `available_tools` already run async). At the bottom of each of the four return branches (lines ~413, ~428, ~463, ~537), replace `description: tool_config.description.clone()` (or `description: <fallback>.to_string()`) with the result of:

```rust
async fn build_description(
    name: &str,
    base_description: &str,
    tool_config: &ToolConfiguration,
    node: &Arc<dyn ExecutableNode>,
    skill_repo: Option<&dyn SkillRepository>,
) -> String {
    use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::tool_context::{
        build_tool_context_block, BlockVariant,
    };
    // Compose effective fixed config: fixed_config + node_schema.fixed values.
    let mut map = serde_json::Map::new();
    for (k, v) in &tool_config.fixed_config { map.insert(k.clone(), v.clone()); }
    if let Some(schema) = &tool_config.node_schema {
        for (k, field) in schema {
            if let Some(v) = &field.fixed {
                map.insert(k.clone(), v.clone());
            }
        }
    }
    let fixed = serde_json::Value::Object(map);

    // build the block; replace {{NODE_GUIDE_BODY}} async if needed
    let mut block = build_tool_context_block(
        tool_config, node.as_ref(), &fixed, skill_repo, BlockVariant::EagerOrNonLazy,
    );

    if block.contains("{{NODE_GUIDE_BODY}}") {
        if let Some(repo) = skill_repo {
            if let Some(entry) = repo.find_by_node_type(&tool_config.node_type) {
                if let Ok(skill) = repo.load_skill(&entry.name).await {
                    block = block.replace("{{NODE_GUIDE_BODY}}", skill.body.trim());
                }
            }
        }
        block = block.replace("{{NODE_GUIDE_BODY}}", "(guide unavailable)");
    }

    // If base_description is non-empty AND different from what's already in
    // the block header, prepend it before the block.
    if base_description.is_empty() {
        block
    } else if block.contains(base_description) {
        block
    } else {
        format!("{}\n\n{}", base_description, block)
    }
}
```

In each of the four return sites, replace the `description: ...` field with `description: build_description(...).await`.

Update `available_tools()` callers — they already await it.

- [ ] **Step 4: Run — expect pass**

```bash
cargo test --lib dag_tool_executor
```

Expected: PASS — including the new test.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "feat(executor): append tool context block to ToolDefinition.description

In all four return branches of generate_tool_definition, build the
EagerOrNonLazy variant of the tool context block and merge it into the
description. New with_skill_repository setter feeds the SkillRepository
in; defaults to None for backward compat.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 12: Pipe the `SkillRepository` and `node` reference from `llm.rs` into `dispatch_describe_tool`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs`

- [ ] **Step 1: Extend `dispatch_describe_tool` signature**

Change the signature of `dispatch_describe_tool` to accept the repo and a registry lookup so it can fetch the live `ExecutableNode`:

```rust
pub async fn dispatch_describe_tool(
    tool_call: &ToolCall,
    lookup: &[ToolConfiguration],
    skill_repo: Option<&dyn SkillRepository>,
    registry: &NodeRegistry,
) -> Result<DescribeToolDispatchResult, LlmError> {
    // ... existing arg parsing ...
    let cfg = lookup.iter().find(|c| c.name == name);
    let output = match cfg {
        Some(c) => {
            let node = registry.get_node(&c.node_type);
            generate_tool_markdown_async(c, node.as_deref(), skill_repo).await
        }
        None => format!("Error: Tool '{}' not found in catalog", name),
    };
    // ... existing return ...
}
```

Update the existing tests calling `dispatch_describe_tool` to pass `None, None`-ish (the call site in `dag_tool_executor.rs` line ~600 also needs updating).

- [ ] **Step 2: Wire from `DagToolExecutor`**

In `dag_tool_executor.rs` where `dispatch_describe_tool` is called (line ~600), pass `self.skill_repo.as_deref()` and `&*self.registry`.

- [ ] **Step 3: Wire from `llm.rs`**

In `llm.rs`, where the `DagToolExecutor` is constructed (around line ~1632), chain `.with_skill_repository(skill_repo.clone())` so the executor has the same repo the `load_skill` path uses.

- [ ] **Step 4: Run tests + build**

```bash
cargo test --lib
cargo build --lib
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/
git commit -m "feat(llm): pipe SkillRepository + registry into describe_tool dispatch

So the dispatch can load the matched layer-1 guide's body and call
ExecutableNode::tool_description_supplement on the live node. Wiring
only — behavior parity preserved when no guide exists for the node type.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 13: Per-request rebuild of the `load_skill` tool with layer rules

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs`

- [ ] **Step 1: Failing test for the rule**

Pick a small testable surface. In `load_skill_tool.rs`:

```rust
#[test]
fn build_filtered_definition_excludes_layer1_guides() {
    use crate::skills::domain::skill_repository::SkillCatalogEntry;
    use crate::skills::domain::skill::SkillSource;
    let catalog = vec![
        SkillCatalogEntry { name: "guide".into(), description: "g".into(),
            source: SkillSource::Builtin, node_type: Some("sql_query".into()) },
        SkillCatalogEntry { name: "free".into(), description: "f".into(),
            source: SkillSource::Builtin, node_type: None },
    ];
    let visible = filter_visible_skills(
        &catalog,
        /*tool_scoped*/ &[],
        /*free_standing*/ &["free".to_string()],
        /*discovered_set*/ &std::collections::HashSet::new(),
        /*scoped_by_tool*/ &std::collections::HashMap::new(),
    );
    let names: Vec<&str> = visible.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["free"]);
}

#[test]
fn build_filtered_definition_includes_scoped_only_after_discovery() {
    use crate::skills::domain::skill_repository::SkillCatalogEntry;
    use crate::skills::domain::skill::SkillSource;
    let catalog = vec![
        SkillCatalogEntry { name: "sales".into(), description: "s".into(),
            source: SkillSource::Builtin, node_type: None },
    ];
    let mut scoped_by_tool = std::collections::HashMap::new();
    scoped_by_tool.insert("consultar_ventas".to_string(), vec!["sales".to_string()]);

    let pre_discovery = filter_visible_skills(
        &catalog, &["sales".to_string()], &[], 
        &std::collections::HashSet::new(), &scoped_by_tool);
    assert!(pre_discovery.is_empty());

    let mut discovered = std::collections::HashSet::new();
    discovered.insert("consultar_ventas".to_string());
    let post_discovery = filter_visible_skills(
        &catalog, &["sales".to_string()], &[], &discovered, &scoped_by_tool);
    assert_eq!(post_discovery.len(), 1);
    assert_eq!(post_discovery[0].name, "sales");
}
```

- [ ] **Step 2: Run — expect failure**

```bash
cargo test --lib load_skill_tool::tests 2>&1 | tail -10
```

Expected: error — `filter_visible_skills` not defined.

- [ ] **Step 3: Implement `filter_visible_skills`**

In `load_skill_tool.rs`:

```rust
/// Apply layer 1/2/3 visibility rules to produce the load_skill catalog
/// for a single LLM request.
///
/// - Layer 1 (node_type set) → excluded (auto-folded elsewhere).
/// - Layer 2 (referenced in any tool_configuration.skills) → included only
///   if that tool is in discovered_set.
/// - Layer 3 (in llm_call.skills, no node_type) → always included.
pub fn filter_visible_skills(
    catalog: &[crate::skills::domain::skill_repository::SkillCatalogEntry],
    tool_scoped_names: &[String],
    free_standing_names: &[String],
    discovered_set: &std::collections::HashSet<String>,
    scoped_by_tool: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<crate::skills::domain::skill_repository::SkillCatalogEntry> {
    let mut out = Vec::new();
    let scoped_set: std::collections::HashSet<&str> =
        tool_scoped_names.iter().map(String::as_str).collect();
    let free_set: std::collections::HashSet<&str> =
        free_standing_names.iter().map(String::as_str).collect();

    for entry in catalog {
        if entry.node_type.is_some() { continue; } // layer 1 — never in load_skill

        if free_set.contains(entry.name.as_str()) {
            out.push(entry.clone()); // layer 3
            continue;
        }

        if scoped_set.contains(entry.name.as_str()) {
            // layer 2 — included only if its parent tool is discovered
            let visible_now = scoped_by_tool.iter().any(|(tool, skills)| {
                discovered_set.contains(tool) &&
                skills.iter().any(|s| s == &entry.name)
            });
            if visible_now { out.push(entry.clone()); }
        }
    }
    out
}

/// Rebuild the load_skill tool definition with a custom subset of catalog
/// entries (the filtered ones).
pub fn build_load_skill_tool_definition_with_catalog(
    entries: &[crate::skills::domain::skill_repository::SkillCatalogEntry],
) -> crate::llm::domain::ToolDefinition {
    // Reuse the existing logic that turns SkillCatalogEntry list into a
    // ToolDefinition (the existing build_load_skill_tool_definition reads
    // repo.list_available(); here we accept the filtered list).
    // Implementation: clone the body of the existing fn with this catalog.
    let mut props = std::collections::HashMap::new();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    // Build a 'name' enum parameter constrained to the visible names.
    let name_param = serde_json::json!({
        "type": "string",
        "enum": names,
        "description": "The skill to load",
    });
    // ... mirror the existing build_load_skill_tool_definition body
    // unimplemented!() // SEE existing fn — extract shared internal builder
    todo!("inline the existing builder body using `entries` instead of repo.list_available()")
}
```

> **Engineering note:** the existing `build_load_skill_tool_definition` already builds the `ToolDefinition` from a catalog list. Refactor it so its body becomes `build_load_skill_tool_definition_with_catalog(catalog: &[Entry])`, and the public `build_load_skill_tool_definition(repo: &dyn SkillRepository)` becomes a thin wrapper that calls it with `repo.list_available()`. Replace the `todo!()` accordingly — no new behavior, just extracting the shared body.

- [ ] **Step 4: Run tests — expect pass**

```bash
cargo test --lib load_skill_tool
```

Expected: PASS.

- [ ] **Step 5: Use the per-request catalog inside the `tools_provider` closure**

In `llm.rs` around the existing `tools_provider` closure (line ~1964), compute the load_skill catalog per request:

```rust
// Compute scoped_by_tool from tool_configurations (built once before the closure)
let scoped_by_tool: std::collections::HashMap<String, Vec<String>> =
    tool_configurations.iter()
        .map(|(name, cfg)| (name.clone(), cfg.skills.clone()))
        .collect();
let free_standing_names: Vec<String> = /* names declared in llm_call.skills */;
let load_skill_repo = skill_repo.clone(); // Arc

let tools_provider = ... {
    move |messages: &[LlmMessage]| {
        let discovered = if lazy_tool_loading {
            reconstruct_discovered_set(messages, &catalog)
        } else {
            // non-lazy: treat all configured tools as discovered
            tool_configurations.keys().cloned().collect()
        };
        // ... existing tools[] composition ...

        // Rebuild load_skill tool definition with filtered catalog
        if let Some(repo) = &load_skill_repo {
            let full = repo.list_available();
            let filtered = crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::load_skill_tool::filter_visible_skills(
                &full, /*tool_scoped*/ &scoped_by_tool.values().flatten().cloned().collect::<Vec<_>>(),
                &free_standing_names, &discovered, &scoped_by_tool,
            );
            if !filtered.is_empty() {
                out.push(build_load_skill_tool_definition_with_catalog(&filtered));
            }
        }
        out
    }
}
```

(Adapt the closure to the existing local variable names. Remove the now-redundant `tools.push(build_load_skill_tool_definition(repo))` at line 1779, since the closure does it.)

- [ ] **Step 6: Run integration tests**

```bash
cargo test --lib
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/
git commit -m "feat(llm): per-request load_skill catalog with layer 1/2/3 rules

The tools_provider closure now also rebuilds the load_skill tool
definition per request, applying:
- layer 1 (node_type) → excluded
- layer 2 (tool.skills) → only if parent tool in discovered_set
- layer 3 (llm_call.skills) → always
In non-lazy mode, all configured tools are treated as discovered so
layer-2 skills are visible from turn 1.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 14: Graph-load validation — hard errors and warnings

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

- [ ] **Step 1: Failing tests for the three hard errors**

Add to the existing test module in `llm.rs` (or to a new `validation` submodule):

```rust
#[test]
fn validation_rejects_duplicate_node_type_guides() {
    // Construct a mock SkillRepository with two skills claiming sql_query.
    // Assert validate_skill_wiring(&tool_configurations, &llm_call_skills, &repo)
    // returns an error mentioning DuplicateNodeTypeGuide.
}

#[test]
fn validation_rejects_unknown_skill_in_tool_configuration() {
    // tool_configurations = { "x": { skills: ["does-not-exist"] } }
    // Assert error mentions 'does-not-exist'.
}

#[test]
fn validation_rejects_node_type_guide_referenced_as_scoped() {
    // SkillRepository exposes "sql_query-guide" with node_type set.
    // tool_configurations = { "x": { skills: ["sql_query-guide"] } }
    // Assert error mentions "is a node-type guide".
}
```

- [ ] **Step 2: Run — expect failure**

```bash
cargo test --lib llm::validation 2>&1 | tail -10
```

Expected: function not defined.

- [ ] **Step 3: Implement validation**

In `llm.rs`, define:

```rust
fn validate_skill_wiring(
    tool_configurations: &HashMap<String, ToolConfiguration>,
    llm_call_skill_names: &[String],
    skill_repo: &dyn SkillRepository,
) -> Result<(), String> {
    // 1. duplicate node_type guides
    let mut seen_node_types: HashMap<String, String> = HashMap::new();
    for entry in skill_repo.list_available() {
        if let Some(nt) = entry.node_type.as_deref() {
            if let Some(prev) = seen_node_types.get(nt) {
                return Err(format!(
                    "node_type '{}' is claimed by skills '{}' and '{}'; only one guide per node_type is allowed",
                    nt, prev, entry.name
                ));
            }
            seen_node_types.insert(nt.to_string(), entry.name.clone());
        }
    }

    // 2. unknown scoped skill name + 3. node-guide used as scoped
    let all_names: HashSet<String> = skill_repo.list_available().iter()
        .map(|e| e.name.clone()).collect();
    let guide_names: HashSet<String> = skill_repo.list_available().iter()
        .filter(|e| e.node_type.is_some())
        .map(|e| e.name.clone()).collect();

    for (tool_name, cfg) in tool_configurations {
        for skill in &cfg.skills {
            if !all_names.contains(skill) {
                return Err(format!(
                    "tool '{}' references unknown skill '{}'",
                    tool_name, skill
                ));
            }
            if guide_names.contains(skill) {
                let nt = skill_repo.list_available().iter()
                    .find(|e| &e.name == skill)
                    .and_then(|e| e.node_type.clone())
                    .unwrap_or_default();
                return Err(format!(
                    "skill '{}' is a node-type guide (frontmatter node_type:{}); it cannot be referenced in tool.skills",
                    skill, nt
                ));
            }
        }
    }

    // 4. warning: llm_call.skills referencing a node-guide → ignore + log
    for name in llm_call_skill_names {
        if guide_names.contains(name) {
            colmena_log!(
                "WARN: skill '{}' is a node-type guide; ignored in llm_call.skills (auto-loaded for matching tools)",
                name
            );
        }
    }

    Ok(())
}
```

Call it from the `llm_call` execute path before building tools, propagating the error as a graph-execution failure (`return Err(...)`).

- [ ] **Step 4: Run — expect pass**

```bash
cargo test --lib llm::validation
```

Expected: PASS for all 3 tests.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm): graph-load validation for skill wiring

validate_skill_wiring runs before building tools and rejects:
- two skills claiming the same node_type,
- tool.skills referencing an unknown name,
- tool.skills referencing a skill marked as a node-type guide.
Warns (does not fail) when llm_call.skills lists a node-type guide.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 15: Add `tool_context_blocks` observability to `extra_info`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

- [ ] **Step 1: Failing test (or smoke)**

Add an inline test that constructs a small `llm_call` execution with the SQL tool and verifies the final extra_info JSON has `tool_context_blocks`. This is easiest as an integration test with a `MockAdapter`; if too heavy for unit, gate with `#[ignore]` and document.

- [ ] **Step 2: Compute the summary**

After tools are built (around the `extra_info` assembly at line ~2328), add:

```rust
let mut blocks_summary = serde_json::Map::new();
for (name, cfg) in &tool_configurations {
    let mut entry = serde_json::Map::new();
    if let Some(repo) = &skill_repo {
        if let Some(guide) = repo.find_by_node_type(&cfg.node_type) {
            entry.insert("node_guide".to_string(), serde_json::Value::String(guide.name));
        }
    }
    // policy line count from node.tool_description_supplement(fixed_effective)
    if let Some(node) = registry.get_node(&cfg.node_type) {
        let fixed = build_effective_fixed(cfg);
        if let Some(policy) = node.tool_description_supplement(&fixed) {
            entry.insert(
                "policy_lines".to_string(),
                serde_json::Value::Number(policy.lines().count().into()),
            );
        }
    }
    if !cfg.skills.is_empty() {
        entry.insert(
            "scoped_skills".to_string(),
            serde_json::Value::Array(cfg.skills.iter().map(|s| serde_json::Value::String(s.clone())).collect()),
        );
    }
    if !entry.is_empty() {
        blocks_summary.insert(name.clone(), serde_json::Value::Object(entry));
    }
}
if !blocks_summary.is_empty() {
    extra_info["tool_context_blocks"] = serde_json::Value::Object(blocks_summary);
}
```

- [ ] **Step 3: Run + commit**

```bash
cargo test --lib
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm): tool_context_blocks in extra_info summary

Surfaces per-tool which node_guide was attached, how many policy lines,
and which scoped skills were declared. Empty entries omitted.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 16: Author E2E layer-2 skills (`sales-analysis`, `expense-analysis`)

**Files:**
- Create: `src/libs/colmena/skills/sales-analysis/SKILL.md`
- Create: `src/libs/colmena/skills/expense-analysis/SKILL.md`

- [ ] **Step 1: Write `sales-analysis/SKILL.md`**

```markdown
---
name: sales-analysis
description: How to analyze sales data — common tables, KPIs, and pitfalls.
---

# Sales analysis

## Tables (assumed)

- `public.orders(id, customer_id, status, total_amount, currency, created_at)`
- `public.order_items(order_id, sku, qty, unit_price)`

## Useful KPIs

- Revenue in a window:
  `SELECT SUM(total_amount) FROM public.orders WHERE status = 'completed' AND created_at >= $1 AND created_at < $2`
- Top SKUs by units:
  `SELECT sku, SUM(qty) AS units FROM public.order_items GROUP BY sku ORDER BY units DESC LIMIT 10`

## Pitfalls

- Filter out `status IN ('cancelled', 'refunded')` when computing revenue
  unless the user explicitly wants gross.
- Currencies may mix; if the dataset is multi-currency, group by currency
  or convert explicitly.
```

- [ ] **Step 2: Write `expense-analysis/SKILL.md`**

```markdown
---
name: expense-analysis
description: How to analyze expenses — categories, vendor rollups, period comparisons.
---

# Expense analysis

## Tables (assumed)

- `public.expenses(id, vendor, category, amount, currency, paid_at)`

## Useful KPIs

- Spend by category in a window:
  `SELECT category, SUM(amount) FROM public.expenses WHERE paid_at >= $1 GROUP BY category ORDER BY SUM(amount) DESC`
- Top vendors by spend YTD:
  `SELECT vendor, SUM(amount) FROM public.expenses WHERE paid_at >= date_trunc('year', NOW()) GROUP BY vendor ORDER BY SUM(amount) DESC LIMIT 10`

## Pitfalls

- Reimbursements may appear as negative amounts — `SUM(amount)` gives net
  spend; use `SUM(GREATEST(amount, 0))` for gross.
```

- [ ] **Step 3: Verify they're indexed (no node_type → not layer-1)**

```bash
cargo build --lib && cargo test --lib builtin_skill_repository 2>&1 | tail -10
```

Expected: build + tests pass; both skills present in `list_available()` and `find_by_node_type` does NOT return them.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/skills/
git commit -m "feat(skills): author sales-analysis and expense-analysis (E2E)

Two domain skills (no node_type), used as layer-2 references in the E2E
graph. Tables and KPIs documented as examples; real graphs override with
their actual schemas.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 17: E2E test graph + manual verification

**Files:**
- Create: `tests/graphs/agents/sql_layered_tool_context.json`

- [ ] **Step 1: Write the graph**

```json
{
  "comment": "E2E for layered tool context: sql_query tool with policy + sql_query-guide + 2 scoped skills.",
  "metadata": {
    "category": "agents",
    "requires_env": ["GEMINI_API_KEY", "DATABASE_URL"]
  },
  "nodes": {
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "lazy_tool_loading": true,
        "system_message": "You analyze business data on demand using the available tools.",
        "tool_configurations": {
          "consultar_ventas": {
            "name": "consultar_ventas",
            "node_type": "sql_query",
            "description": "Consultar datos de la base.",
            "summary": "Consulta ventas y gastos contra Postgres.",
            "skills": ["sales-analysis", "expense-analysis"],
            "node_schema": {
              "connection_url": { "type": "string", "fixed": "${DATABASE_URL}" },
              "permissions": {
                "type": "object",
                "fixed": { "preset": "read_only", "allowed_schemas": ["public"] }
              },
              "runtime_limits": {
                "type": "object",
                "fixed": { "max_rows": 50, "statement_timeout_ms": 15000 }
              },
              "guardrail_enabled": { "type": "boolean", "fixed": true },
              "guardrail_llm": { "type": "object", "fixed": { "enabled": false } },
              "query": {
                "type": "string", "required": true,
                "description": "SQL SELECT query."
              }
            }
          }
        },
        "prompt": "¿Cuánta plata vendimos en abril 2026?"
      }
    },
    "result": { "type": "output", "config": { "label": "out" } }
  },
  "edges": [{ "from": "agent", "to": "result" }]
}
```

- [ ] **Step 2: Run against the real DB + LLM**

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/sql_layered_tool_context.json --agent-session-id sql_layered_e2e --include-extra-info 2>&1 | tail -60
```

Expected:
- `describe_tool("consultar_ventas")` shows up in the stream.
- The returned markdown contains "Access policy", "Best practices", "Related knowledge".
- A `load_skill("sales-analysis")` call follows (the model recognizes intent).
- A SQL `SELECT` is issued, respects `public` schema and `SELECT` only.
- `extra_info.tool_context_blocks.consultar_ventas` is populated.

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/agents/sql_layered_tool_context.json
git commit -m "test(e2e): sql_layered_tool_context graph for layered context

llm_call with a sql_query tool that exercises the full layered context:
config-derived policy, node-type guide, two scoped layer-2 skills, lazy
mode. Verified against Gemini Flash + Postgres.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 18: Documentation

**Files:**
- Modify: `docs/developer_guide/29_lazy_tool_loading.md`
- Modify: `docs/developer_guide/24_skills.md`
- Modify: `docs/node_configurations.json`
- Modify: `docs/node_as_tools_reference.json`
- Modify: `docs/CHANGELOG_2026-05.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: `29_lazy_tool_loading.md` — new "Tool context block" section**

Append a section under the existing structure:

```markdown
## Tool context block

When the engine builds the markdown that describe_tool returns (lazy)
or the description that ships in tools[] (eager / non-lazy), it now
assembles a layered block with up to four sections:

1. `# {tool_name}` + the tool's description.
2. `## Access policy` — if `ExecutableNode::tool_description_supplement`
   returned `Some`, derived from the tool's fixed config (e.g.
   sql_query's preset + allowed_schemas + max_rows).
3. `## Best practices` — body of the SKILL.md whose frontmatter has
   `node_type: <this_node_type>`. One guide per node_type, validated at
   graph load.
4. `## Parameters` — present only in the lazy describe_tool variant;
   the eager / non-lazy path omits it because the schema travels typed.
5. `## Related knowledge` — names + descriptions of every skill listed
   in this tool's `tool_configurations.<name>.skills` array. The model
   loads them with `load_skill(name)` based on intent.

Routing follows the existing lazy/eager bifurcation: lazy + non-eager
goes through describe_tool (on demand); eager OR `lazy_tool_loading:
false` goes via the tool description (always in the prompt).
```

- [ ] **Step 2: `24_skills.md` — `node_type` frontmatter + scoped skills**

Append a new section "Layered routing":

```markdown
## Layered routing

A skill's role is derived from how it's wired, not from where the file
lives. All skills share the same pool.

| Role | How it's marked | How it reaches the model |
|---|---|---|
| Layer 1 — node-type guide | frontmatter `node_type: <name>` | auto-folded into the tool context block of every tool with matching node_type; **never** in the `load_skill` catalog |
| Layer 2 — tool-scoped specific | referenced in `tool_configurations.<name>.skills` | appears in the `load_skill` catalog **only after** the parent tool is discovered (lazy `discovered_set`); in non-lazy mode, visible from turn 1 |
| Layer 3 — free-standing general | referenced in `llm_call.skills` and no `node_type` | always in the `load_skill` catalog (today's behavior) |

Validations at graph load:
- At most one skill per node_type. Two guides claiming the same
  node_type → hard error.
- A `tool.skills` reference to an unknown name → hard error.
- A `tool.skills` reference to a skill marked as a node_type guide →
  hard error.
- A `llm_call.skills` reference to a node_type guide → warning, ignored.
```

- [ ] **Step 3: `node_configurations.json` — `tool_configurations.*.skills`**

Find the `llm_call` `tool_configurations` schema entry and add the
`skills` field next to `summary` and `eager`:

```jsonc
"skills": {
  "type": "array",
  "items": { "type": "string" },
  "required": false,
  "default": [],
  "description": "Layer-2 skill names scoped to this tool. Resolved against the active SkillRepository at graph load (unknown names → error). Appear in the load_skill catalog only after this tool is discovered (lazy describe_tool). In non-lazy mode, visible from turn 1."
}
```

- [ ] **Step 4: `node_as_tools_reference.json` — append a note about layered context**

Add to the sql_query notes array a new line:

```json
"When a tool's node has a layer-1 guide skill (SKILL.md with node_type: <this_type>), the engine auto-folds its body into describe_tool output and into the tool description in eager/non-lazy. tool_configurations.<name>.skills lists per-tool layer-2 references gated by describe_tool."
```

- [ ] **Step 5: `CHANGELOG_2026-05.md` — section 10**

Insert before `## Misc`:

```markdown
## 10. Layered tool context — policy + node-type guide + tool-scoped skills (2026-05-29)

**Qué cambió.** Cada nodo usado como tool LLM ahora recibe, de forma
automática, un **bloque de contexto** compuesto por: (1) su description,
(2) política derivada de su fixed config (vía un hook nuevo en
`ExecutableNode::tool_description_supplement`), (3) la guía de
best-practices del node-type (una SKILL.md con `node_type: <name>` en el
frontmatter — una por node-type), y (4) un anuncio de las "skills
específicas" scoped a esa tool (`tool_configurations.<name>.skills`),
que el modelo puede cargar con `load_skill` solo después de hacer
`describe_tool` (visibility-gating sobre el `discovered_set`). En modo
eager o sin lazy, todo el bloque va en la `description` desde el turno
1 y las skills scoped quedan disponibles también desde turno 1.

Reusa la infra de Skills (`include_dir!`, frontmatter, 64 KB) como
único repositorio de markdown. Una skill con `node_type` nunca entra al
catálogo de `load_skill` (es auto-folded). El primer nodo con guía es
`sql_query`: la política sale de `SqlPermissions` y la guía vive en
`skills/sql_query-guide/SKILL.md`.

**Documentación:**
- Spec: [docs/superpowers/specs/2026-05-29-layered-tool-context-design.md](superpowers/specs/2026-05-29-layered-tool-context-design.md)
- Plan: [docs/superpowers/plans/2026-05-29-layered-tool-context.md](superpowers/plans/2026-05-29-layered-tool-context.md)
- Dev guides: [29_lazy_tool_loading.md](developer_guide/29_lazy_tool_loading.md) ("Tool context block"); [24_skills.md](developer_guide/24_skills.md) ("Layered routing").
- Schema: [node_configurations.json](node_configurations.json) → `llm_call.tool_configurations.*.skills`.

**Estado:** ✅ Done. Verificado E2E contra Gemini Flash + Postgres.

> **Sweep ADP:** añade un método default-None a `ExecutableNode` y un
> campo opcional a `ToolConfiguration` (con `#[serde(default)]`).
> Cambios additivos — no rompe el worker de ADP.

---
```

And add a row to the matrix:

```markdown
| 10. Layered tool context | ✅ | ✅ | ✅ | ✅ (`tool_configurations.*.skills`) | ✅ (1 E2E) | sql_query es el nodo de referencia. Guías por nodo (http_request, socketio, etc.) quedan como follow-ups. |
```

- [ ] **Step 6: `CLAUDE.md` — Current Status bullet**

Insert after the `SQL node auto-creates allowed_schemas` bullet:

```markdown
- **Layered tool context shipped 2026-05-29** — every node used as an LLM
  tool now receives an auto-assembled context block: description +
  config-derived policy (via `ExecutableNode::tool_description_supplement`)
  + node-type best-practices guide (a `SKILL.md` with
  `node_type: <name>` frontmatter, auto-folded) + announcement of
  tool-scoped layer-2 skills (`tool_configurations.<name>.skills`).
  Layer-2 skills are gated by visibility on the lazy `discovered_set`
  (visible after `describe_tool`; from turn 1 in non-lazy). Reuses the
  Skills infra (`include_dir!`, frontmatter, 64 KB). First node with a
  guide: `sql_query`. See
  [`docs/superpowers/specs/2026-05-29-layered-tool-context-design.md`](docs/superpowers/specs/2026-05-29-layered-tool-context-design.md)
  and [`docs/developer_guide/29_lazy_tool_loading.md`](docs/developer_guide/29_lazy_tool_loading.md).
```

- [ ] **Step 7: Commit**

```bash
git add docs/ CLAUDE.md
git commit -m "docs: layered tool context feature

Updated 29_lazy_tool_loading.md, 24_skills.md, node_configurations.json,
node_as_tools_reference.json, CHANGELOG_2026-05.md (section 10 + matrix
row), CLAUDE.md Current Status bullet.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Final verification

- [ ] **Run the full test suite verbose (catches doctests):**

```bash
cargo test --verbose 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Run clippy (deny-warnings is on):**

```bash
cargo clippy --all-targets 2>&1 | tail -20
```

Expected: clean.

- [ ] **Run the E2E graph once more end-to-end:**

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/sql_layered_tool_context.json --agent-session-id final_verify --include-extra-info 2>&1 | tail -40
```

Expected: describe_tool returns the full block; load_skill of a scoped skill happens after describe; SQL query is SELECT-only against `public`.

- [ ] **Confirm no remaining placeholder markers:**

```bash
grep -rn "{{NODE_GUIDE_BODY}}" src/libs/colmena/src/ | grep -v 'tool_context.rs'
```

Expected: empty (only the producer mentions the marker; everywhere else it's resolved).

---

## Self-review (filled by author after writing)

**Spec coverage:**
- §Architecture three layers → Tasks 1-3 (skills), 4-6 (policy), 7 (scoped skills), 8 (builder), 13 (catalog rules). ✓
- §Components A-G → Tasks 1-3 (A), 4 (B), 7 (C), 8 (D), 10 (E), 11 (F), 13 (G). ✓
- §Validation → Task 14. ✓
- §Observability → Task 15. ✓
- §Testing → unit tests in each task, integration covered by E2E in Task 17. ✓
- §Backward compat → maintained: every override has a default-None / serde-default empty. ✓
- §Decomposition note (per-node guides as follow-ups) → only `sql_query-guide` (Task 9) + two demo scoped skills (Task 16) are authored; spec explicitly defers other guides. ✓

**Placeholder scan:** the `todo!()` in Task 13 step 3 is intentional and tagged with the engineering note that explains exactly what to do. Acceptable: the engineer is told to inline the existing function's body. No "TBD", "fill in", or vague guidance.

**Type consistency:** `SkillCatalogEntry.node_type: Option<String>` introduced in Task 2 is used the same way in Tasks 3, 9, 13, 14, 15. `tool_description_supplement(&Value)` introduced in Task 4 is the exact signature called in Tasks 6, 8, 11, 15. `BlockVariant::Lazy` / `BlockVariant::EagerOrNonLazy` defined in Task 8 and used in Tasks 10, 11. `ToolConfiguration.skills: Vec<String>` introduced in Task 7 used in Tasks 8, 13, 14, 15. Consistent.
