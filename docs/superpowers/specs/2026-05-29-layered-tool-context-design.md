# Spec — Layered tool context for LLM nodes

> ⚠️ **SUPERSEDED on 2026-05-31** by
> [2026-05-31-revert-layered-tool-context-design.md](2026-05-31-revert-layered-tool-context-design.md).
> The 3-layer mechanism was reverted; only `llm_call.skills` survives.
> Recursive references and `skills_path` were added as replacements.

**Date:** 2026-05-29
**Status:** SUPERSEDED — see above
**Supersedes scope of:** the inline "inject SQL policy into prompt" idea from the
schema-provisioning work — absorbed here as one half of layer 1.

## Problem

When a node is connected as a tool to an `llm_call`, the model has very little
context about how to use it well or what it's allowed to do:

- `ToolConfiguration.description` is the only authored text; it's whatever the
  graph author wrote, with no convention or enforcement.
- Permission/policy information (e.g. `sql_query` `allowed_schemas`,
  `read_write` vs `full`) lives in the node's fixed config and **never reaches
  the model** — only the runtime validator enforces it, surfaced as errors
  after the fact.
- Domain knowledge ("how to analyze sales" for a `consultar_ventas` tool) has
  no home tied to the tool; the only existing mechanism is free-standing
  Skills (`load_skill`), which the model loads from a flat catalog with no
  relationship to the tools in front of it.
- Lazy tool loading (`describe_tool`) is a perfect on-demand channel for
  detailed per-tool content, but today returns only the description + a
  parameter table.

We need the model to **deeply understand each tool's context the moment it
considers using it**: what the tool is for, what it's allowed to do for *this*
instance, best practices for *this kind* of node, and which specific domain
skills are relevant to *this* connection.

## Goal

Give every tool a layered, structured context block that:

1. Is **authored once per node-type** for generic best practices (reused by every
   instance of that node type, across graphs).
2. Is **derived from config per instance** for the policy (operations, schemas,
   limits).
3. Optionally exposes **domain-specific skills tied to that connection**, loadable
   on demand only after the model expresses intent to use the tool.
4. Reuses the existing Skills infrastructure (`include_dir!`, frontmatter, 64 KB
   limits, references) as the single authoring/storage substrate for all
   markdown content.
5. Routes content automatically through the existing lazy/eager bifurcation —
   no new "when to inject" rules.

## Non-goals

- We are **not** authoring guides for every node type in this spec. This spec
  ships the **mechanism** plus the reference implementation on `sql_query`.
  Per-node guides are follow-up work, one PR per node type.
- We are **not** changing how `load_skill` validates skill content (same trust
  model: structural validation, not semantic).
- We are **not** changing the LLM-issued `CREATE SCHEMA` block in `sql_query` or
  any other validator.
- We are **not** introducing a new synthetic tool. We reuse `describe_tool` and
  `load_skill` as-is and only enrich what they return / what's in their catalogs.

## Decisions (confirmed during brainstorming)

1. **Three layers, single skills pool.** All markdown content (guides, specific
   skills, free-standing skills) lives in one pool — built-in via `include_dir!`
   under `src/libs/colmena/skills/` and/or user paths. The *role* of a skill is
   derived from frontmatter + where it's referenced, not from physical location.
2. **Layer 1 (node-type guide + config-derived policy)** auto-folds into
   `describe_tool` output (lazy) and into `ToolDefinition.description`
   (eager/non-lazy). Always reaches the model when the tool is in play.
3. **Layer 2 (tool-scoped domain skills)** lives in `tool_configuration.skills`.
   Gated by **visibility**: appears in the `load_skill` catalog only after
   the parent tool enters the `discovered_set` (i.e. after `describe_tool`).
   In non-lazy/eager mode (no `describe_tool`), visible from turn 1.
4. **Layer 3 (free-standing general skills)** continues to work as today —
   referenced in `llm_call.skills`, always in the `load_skill` catalog.
5. **Binding for layer 1**: the skill's `SKILL.md` frontmatter declares
   `node_type: <name>`. The engine auto-associates that skill with every tool
   whose `node_type` matches. Adding a guide for a new node = drop a `.md` with
   the right frontmatter into the pool. No code change.
6. **Backward compat is total**: graphs without `node_type` in any skill,
   without `tool_configuration.skills`, and without authored layer-1 guides
   behave exactly as today.

## Architecture

### Layer model

| Layer | Content source | Authoring | Delivery |
|---|---|---|---|
| **1 — policy** | computed from fixed config by the node | `ExecutableNode::tool_description_supplement(&fixed_config)` (Rust) | folded into the tool context block |
| **1 — node-type guide** | markdown `SKILL.md` with `node_type: X` frontmatter | author drops a `.md` in the pool | folded into the tool context block |
| **2 — tool-scoped specific** | markdown `SKILL.md` (no `node_type`) referenced in `tool_configuration.skills` | author + per-graph wiring | `load_skill`, gated by visibility on discovered_set |
| **3 — free-standing general** | markdown `SKILL.md` (no `node_type`) referenced in `llm_call.skills` | author + llm_call wiring | `load_skill`, always in catalog (unchanged from today) |

### Single-pool classification rule

For each skill loaded into the `SkillRepository`:

- If frontmatter has `node_type: X` → it's a **layer-1 guide** for node type X.
  Excluded from the `load_skill` catalog entirely; surfaced only via
  auto-folding.
- Else if referenced by some `tool_configuration.skills` of the current
  `llm_call` → **layer 2**, scoped to that tool, gated by visibility.
- Else if referenced by `llm_call.skills` → **layer 3**, always available.
- The same skill name cannot be both a layer-1 guide and a layer-2/3 reference
  (validated at graph load, see Validation).

### Tool context block

A pure function `build_tool_context_block(cfg, node, fixed_config_effective, repo)`
produces the canonical block. Used at two injection points:

1. **`generate_tool_markdown`** (lazy `describe_tool`): the full block.
2. **`generate_tool_definition`** (eager / non-lazy): the block **minus the
   Parameters section** is appended to `ToolDefinition.description` (the schema
   already travels typed in `tools[]`).

The block is composed of optional sections (each omitted when its input is
empty), in this fixed order:

```
# {tool_name}

{tool_description}                            # always; cfg.description trimmed

## Access policy                              # if node.tool_description_supplement(...) → Some
{policy_text, multi-line}

## Best practices                              # if a SKILL.md with node_type matches
{markdown content of the matching skill}

## Parameters                                  # only in the lazy describe_tool variant
| Name | Type | Required | Description |
| ...  | ...  | ...      | ...         |

## Related knowledge                           # if cfg.skills not empty
Load with `load_skill(name)` when your task matches:
- {skill_name}: {skill_description}
- ...
```

### Routing through existing channels

- **Lazy + non-eager tool**: `describe_tool(X)` returns the full block. The
  `load_skill` catalog rebuilds per request based on the `discovered_set`,
  bringing the layer-2 skills of every discovered tool into view.
- **Eager tool, OR `lazy_tool_loading: false`**: the block (minus Parameters)
  is concatenated to `ToolDefinition.description` from turn 1. Layer-2 skills
  of those tools are visible in `load_skill` from turn 1 (no `describe_tool`
  step exists to gate them).

This preserves the user-stated rule: *"si se elige eager o si el modelo deja de
ser lazy, toda esta info se agrega al prompt siempre"*.

## Components

### A. Skills infrastructure (`src/libs/colmena/src/llm/.../skill_*.rs`)

- Parse `node_type: Option<String>` from `SKILL.md` frontmatter.
- `SkillRepository` indexes two views over the same pool:
  - `by_name: HashMap<String, Skill>` (existing).
  - `by_node_type: HashMap<String, String>` (new) — node type → skill name.
- Validation at load time:
  - Duplicate `node_type` across two skills → hard error.
  - `node_type` referencing an unregistered node → warning + skip at
    build-block time (does not block graph load, since feature-gated nodes may
    not exist in every build).

### B. `ExecutableNode` trait (`dag_engine/domain/node.rs`)

```rust
/// Optional config-derived text appended to the tool's description / context
/// block. Pure function of the fixed config — no I/O.
fn tool_description_supplement(&self, _fixed_config: &Value) -> Option<String> { None }
```

Default `None`. `SqlNode` overrides it. Other nodes may override over time.

### C. `ToolConfiguration` (`dag_engine/domain/tool_configuration.rs`)

```rust
pub struct ToolConfiguration {
    // ... existing fields
    #[serde(default)]
    pub skills: Vec<String>,    // layer-2 skill names scoped to this tool
}
```

Resolved against the active `SkillRepository`. Unknown names → hard error at
graph load.

### D. Tool context builder (new module `llm_synthetic_tools/tool_context.rs`)

Pure function:

```rust
pub fn build_tool_context_block(
    cfg: &ToolConfiguration,
    node: &dyn ExecutableNode,
    fixed_config_effective: &Value,
    skill_repo: Option<&dyn SkillRepository>,
    variant: BlockVariant,        // Lazy | EagerOrNonLazy
) -> String;
```

`BlockVariant::Lazy` includes the Parameters section; `EagerOrNonLazy` omits it.

`fixed_config_effective` is built from either `parsed.fixed_values`
(node_schema branch) or `tool_config.fixed_config` (other branches) — the same
flattened map used at execution time.

### E. `describe_tool.rs`

`generate_tool_markdown` becomes a thin wrapper that calls
`build_tool_context_block(..., BlockVariant::Lazy)` and appends the existing
"now available / call it on your next turn" footer.

### F. `dag_tool_executor.rs::generate_tool_definition`

In each of the four return branches that produce a `ToolDefinition` from a
configured tool, append the result of
`build_tool_context_block(..., BlockVariant::EagerOrNonLazy)` to the
`description` field.

### G. `llm.rs` — dynamic `load_skill` catalog

- Today: `build_load_skill_tool_definition(repo)` is called once per execute.
- Change: the catalog is **rebuilt per request**, taking the current
  `discovered_set` as input. Mirror the existing per-request rebuild of
  `tools[]` used by the lazy system.
- Catalog inclusion rules (computed per request):
  - Skills with `node_type` in frontmatter → **excluded** (layer 1, never
    user-loaded).
  - Skills referenced in `llm_call.skills` and no `node_type` → **included
    always** (layer 3).
  - Skills referenced in some `tool_configuration.skills` → **included only
    if that tool is in `discovered_set`** (layer 2, gated).
  - In non-lazy mode: treat all configured tools as "discovered" → layer 2
    visible from turn 1.

## Data flow — end-to-end example

Setup: `lazy_tool_loading: true`, tool `consultar_ventas` backed by `sql_query`
with `permissions: { preset: "read_write", allowed_schemas: ["public",
"analytics"] }` and `skills: ["sales-analysis", "expense-analysis"]`. Pool
contains `sql_query-guide` (frontmatter `node_type: sql_query`) and the two
analysis skills.

| Turn | Model action | What happens |
|---|---|---|
| 1 | sees catalog: `consultar_ventas — "Consulta ventas"`; `load_skill` catalog: (empty for these skills, none discovered) | classifier resolved: `sql_query-guide` → layer 1 for `consultar_ventas`; `sales-analysis`, `expense-analysis` → layer 2 of `consultar_ventas`, hidden until discovery |
| 1 | calls `describe_tool("consultar_ventas")` | returns the full block: description + access policy (SELECT/INSERT/UPDATE on public/analytics) + best practices markdown + parameters table + announcement of `sales-analysis` and `expense-analysis` |
| 2 | `discovered_set = {consultar_ventas}`; `load_skill` catalog now includes both layer-2 skills | rebuilt for this request |
| 2 | calls `load_skill("sales-analysis")` | returns the skill markdown |
| 3 | calls `consultar_ventas` with a query informed by both the policy and the sales skill | normal tool call path |

If `lazy_tool_loading: false`: turn 1 already has `consultar_ventas` typed in
`tools[]` with the full block (minus Parameters) in its description, AND
`sales-analysis`/`expense-analysis` already in the `load_skill` catalog.

## Validation

### At graph load (hard errors)

| Condition | Error |
|---|---|
| Two skills declare the same `node_type` | `"node_type 'X' is claimed by skills 'A' and 'B'; only one guide per node_type is allowed"` |
| `tool_configuration.skills` references an unknown skill name | `"tool '<name>' references unknown skill '<skill>'"` |
| `tool_configuration.skills` lists a skill whose frontmatter has `node_type` | `"skill '<skill>' is a node-type guide (frontmatter node_type:<X>); it cannot be referenced in tool.skills"` |

### At graph load (warnings, non-fatal)

| Condition | Behavior |
|---|---|
| `llm_call.skills` lists a skill with `node_type` in frontmatter | warning, skill ignored at that reference (auto-loaded for matching tools instead) |
| Skill `node_type` points to a node type not in the registry | warning, skill skipped when building blocks |

### Existing limits inherited

- 64 KB per `SKILL.md`.
- 64 KB per reference.
- 50 active skills per node.

## Edge cases

| Case | Behavior |
|---|---|
| Tool whose `node_type` has no guide in the pool | Block omits the "Best practices" section. |
| Node's `tool_description_supplement` returns `None` | Block omits the "Access policy" section. |
| Tool with `skills: []` | Block omits "Related knowledge". |
| Same `node_type` across multiple tools in one `llm_call` | Each tool gets the same guide markdown but its own policy + own layer-2 list. |
| Truncation drops both `describe_tool(X)` and direct calls to X | X exits `discovered_set`; its layer-2 skills disappear from the `load_skill` catalog. Re-describing re-enables them. |
| Tool discovered via "rule 2" of lazy (direct call without prior `describe_tool`) | Layer-2 skills become visible from that turn; same as if `describe_tool` had been called. |
| Same skill (no `node_type`) referenced in BOTH `tool_configuration.skills` and `llm_call.skills` | No error. `llm_call.skills` reference wins → skill is layer 3 (always visible). The tool's "Related knowledge" announcement still lists it; the redundancy is benign. |

## Observability

- Reuse existing `ToolDescribed`, `skill_loaded`, `LlmToolCallStart/Finish` events.
- Add `tool_context_blocks` to the `llm_call` final `extra_info`, present only
  when at least one block was built:
  ```json
  {
    "tool_context_blocks": {
      "consultar_ventas": {
        "node_guide": "sql_query-guide",
        "policy_lines": 5,
        "scoped_skills": ["sales-analysis", "expense-analysis"]
      }
    }
  }
  ```

## Testing strategy

### Unit

- `SkillRepository`: parse `node_type` frontmatter; `find_by_node_type`;
  duplicate-detection error path.
- `build_tool_context_block`: each section present/absent per input combination;
  stable ordering; `Lazy` vs `EagerOrNonLazy` produce the expected difference
  (Parameters section).
- `ToolConfiguration::skills`: serde round-trip; resolution against repo;
  unknown-name error path; node-guide-used-as-scoped error path.
- `SqlPermissions::describe_policy_for_llm(max_rows)`: read_only, read_write,
  full presets; empty allowed_schemas → "all"; with `deny` list; with sandbox
  customization.
- `load_skill` catalog rebuild: matrix of `discovered_set` × tool-skill
  references × `llm_call.skills` produces the expected included names.

### Integration (`#[ignore]` when external deps required)

- Lazy mode: tool with layer-1 guide and two layer-2 skills. `describe_tool`
  returns the full block; layer-2 absent from `load_skill` until then; present
  in the next turn after describe.
- Non-lazy mode: same config; full block in `description` from turn 1; layer-2
  available from turn 1.
- `llm_call.skills` lists a skill with `node_type` → warning logged, skill not
  in the catalog (the auto-fold path is the only consumer).
- Truncation: prune the `describe_tool` from history; verify layer-2 reverts to
  hidden.

### E2E (real graph + real LLM, Gemini Flash + Postgres)

`tests/graphs/agents/sql_layered_tool_context.json` — `llm_call` with
`consultar_ventas` (sql_query, `read_write`, `["public", "analytics"]`,
`skills: ["sales-analysis", "expense-analysis"]`) and `sql_query-guide` in the
built-in pool. Verify a real-LLM run does:

1. `describe_tool("consultar_ventas")` → block contains all sections.
2. `load_skill("sales-analysis")` when the user prompt is sales-related.
3. The eventual SQL query matches the policy (no `DELETE`, schemas in
   `public/analytics`).

## Risks

| Risk | Mitigation |
|---|---|
| Token blow-up in non-lazy with many tools each carrying long guides | Documented guidance: > 5 tools with non-trivial guides → enable `lazy_tool_loading`. The lazy/eager bifurcation is exactly the lever for this. |
| Badly written guide induces wrong model behavior | Same trust model as Skills: structural validation only. Documented in the trust-model section of the dev guide. |
| Frontmatter `node_type` references a renamed node | Warning at build-block time; surfaces during testing of the affected graph. |
| Dynamic `load_skill` catalog disrupts prompt caching | Catalog only changes when `discovered_set` changes (a discovery event). On unchanged turns the catalog hashes identically. |

## Backward compatibility

This work is additive. A graph that:

- has no `tool_configuration.skills`,
- references no skill with `node_type` in frontmatter,
- runs against a `SqlNode` whose `tool_description_supplement` returns `None`
  (i.e. before that override ships) or with `permissions` unset,

produces a `describe_tool` markdown and a `ToolDefinition.description`
**byte-identical to today**. The new sections appear only when their inputs do.

## Scope / decomposition

This spec ships the **mechanism** end to end plus the **first node** to use it
(`sql_query`):

- Layer 1 policy: implement `SqlNode::tool_description_supplement` using the
  existing `SqlPermissions`.
- Layer 1 guide: author `sql_query-guide` markdown in the built-in pool.
- Layer 2: wire one E2E example (`sales-analysis` + `expense-analysis`).

**Follow-up work (separate specs / PRs, one per node type)**: author layer-1
guides for `http_request`, `socketio_request`, `python_script`, `tavily_client`,
`current_time`, `api_explorer`, the document_* synthetic tools, and any other
node that benefits. Each is a `.md` drop + tests against a real-LLM graph.

## Open items

None for the mechanism. Per-node guide content is intentionally out of scope
for this spec.
