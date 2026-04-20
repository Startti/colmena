# Design: Skills in LLM Nodes

**Status:** Approved for planning
**Date:** 2026-04-20
**Author:** Daniel Garcia (brainstormed with Claude)
**Target version:** 0.4.0

## Summary

Add a **Skills** feature to Colmena's LLM node. A skill is a markdown document (`SKILL.md`) with YAML frontmatter that packages specialized instructions. Skills are loaded on-demand by the LLM at runtime via a synthetic tool (`load_skill`), following the same progressive-disclosure model used by Claude Code. Skills can be either built-in (compiled into the Rust crate) or user-provided (loaded from filesystem paths referenced in the graph JSON). The feature is opt-in per `llm_call` node — no overhead when not configured.

## Motivation

Today the only way to extend an LLM node's behavior is via the `system_message` field, which is always injected in full. When a node needs several different specialized behaviors (e.g., "be good at Python, at SQL, at security review"), the system prompt balloons and wastes tokens on skills that are not relevant to the current request.

Claude Code and Gemini CLI solve this with Skills: a directory of markdown files, loaded only when the model decides one applies. Colmena should offer the same capability, adapted to its DAG-based execution model.

## Goals

- Allow LLM nodes to reference a set of skills (built-in and/or user-provided).
- Let the LLM decide at runtime which skills to load, based on a name+description catalog.
- Support secondary reference documents (`references/*.md`) loaded on-demand within a skill.
- Emit observability events when skills are loaded (SSE + final summary).
- Enforce a whitelist of allowed filesystem directories for user skills.
- Zero overhead for LLM nodes that do not use the feature.

## Non-goals

- Prompt caching (Anthropic `cache_control`) — deferred.
- Python/TypeScript binding first-class parameters — users can pass `skills` as part of a config dict and it will work via `serde_json`; explicit typed parameters deferred.
- Variable interpolation inside skill bodies — skills are read as plain text.
- Executing code from skills — skills are text only.
- Mitigating prompt injection from hostile skill content — treated as a trust-the-author contract (documented explicitly).
- Caching skills across executions on disk — skills are re-read each run.

## Architecture

### Layers (hexagonal)

Three components, in a new top-level module `src/libs/colmena/src/skills/`:

**Domain** (`skills/domain/`):
- `Skill` — value object (name, description, body, references metadata).
- `SkillReference` — value object (name, description, body).
- `SkillRepository` — trait (port) with `list_available`, `load_skill`, `load_reference`.
- `SkillConfig` / `SkillSource` — parsed form of the user's config.
- `SkillError` — typed errors via `thiserror`.

**Infrastructure** (`skills/infrastructure/`):
- `frontmatter_parser.rs` — parses YAML frontmatter + returns body without it.
- `builtin_skill_repository.rs` — reads skills compiled with `include_dir!` from `src/libs/colmena/skills/`.
- `filesystem_skill_repository.rs` — reads from user paths; validates against the allowed-dirs whitelist.
- `composite_skill_repository.rs` — merges builtin + filesystem; detects name collisions at construction time.

**Integration with the LLM node** (`src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/`):
- `load_skill_tool.rs` — builds the `ToolDefinition` for `load_skill`, parses tool-call args, dispatches to `SkillRepository`.

### High-level flow

```
Graph JSON → LlmNode.execute()
   ├─ Parse config.skills; if none present → skip everything below
   ├─ Build CompositeSkillRepository (builtin + paths)
   ├─ Validate at load time (names, frontmatter, refs, paths, sizes) — fail fast
   ├─ Build load_skill ToolDefinition with catalog in description
   ├─ Register load_skill in the ToolExecutor alongside user tools
   └─ AgentService.run() — LLM sees load_skill and decides when to call it
```

### Activation condition

Skills are **optional**. Everything below activates only when the config contains at least one skill:

- Not configured / empty arrays → no `SkillRepository` constructed, no `load_skill` tool registered, no new events emitted. Node behaves exactly as today.
- At least one skill (builtin or path) → full pipeline activates.

## Config schema

```json
{
  "type": "llm_call",
  "config": {
    "provider": "openai",
    "api_key": "${OPENAI_API_KEY}",
    "model": "gpt-4o-mini",
    "system_message": "You are a helpful assistant.",

    "skills": {
      "builtin": ["python-expert", "security-review"],
      "paths": [
        "./my-skills/customer-context",
        "./my-skills/internal-apis"
      ]
    }
  }
}
```

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `skills.builtin` | `string[]` | No (default `[]`) | Names of skills compiled into the crate. Each name must exist in the built-in registry. |
| `skills.paths` | `string[]` | No (default `[]`) | Paths to user skill directories. Relative paths resolve against the graph JSON's directory. Each must be a directory containing a `SKILL.md`. |

No other sub-fields. No `family`, no `override`, no `metadata` — YAGNI.

## Skill structure on disk

```
python-expert/                     ← directory name = canonical skill name
  SKILL.md                         ← entry point (required)
  references/                      ← directory (optional)
    frameworks.md
    testing.md
```

## `SKILL.md` format

```markdown
---
name: python-expert
description: Use when the user asks about Python typing, async patterns, or Python standard library internals. Not for general programming questions or questions about other languages.
references:
  - name: frameworks
    description: Detailed information about Django, FastAPI, and Flask patterns
  - name: testing
    description: pytest fixtures, mocking strategies, and test organization
---

# Python Expert

You are an expert in modern Python (3.11+).

## Core principles
- Prefer type hints over runtime validation
- Use `asyncio` for I/O-bound concurrency
```

### Frontmatter rules

- `name` (required): must exactly match the directory name. Mismatch → graph load error.
- `description` (required): non-empty string. This is what the LLM sees in the catalog to decide when to invoke the skill. Should include "when to use" and "when NOT to use".
- `references` (optional): array of `{name, description}` entries. For each entry, a file `references/<name>.md` must exist on disk. Declared but missing reference → graph load error.

## Security model

### Path resolution

At graph load time, each path in `skills.paths` is resolved as follows:

```
1. Read env var COLMENA_SKILLS_ALLOWED_DIRS
   (separator: ':' on Unix, ';' on Windows)
2. allowed_dirs = [dir_of_graph_json] + parsed_env_var_dirs
3. For each path in skills.paths:
   a. Resolve relative to dir_of_graph_json if relative
   b. std::fs::canonicalize(path)          ← resolves symlinks, normalizes
   c. Verify canonical path starts with    ← prevents symlink escape
      at least one canonicalized allowed_dir
   d. Verify it is a directory
   e. Verify it contains SKILL.md
   f. Fail fast on any violation
```

`canonicalize()` is essential: it defeats `../` escapes and symlinks that escape allowed dirs.

**Residual risk not mitigated:** TOCTOU between validation and read. Requires local filesystem access, and skill content is never executed — acceptable.

### Prompt injection

**Not mitigated.** A hostile skill can contain instructions that mislead the model ("ignore previous instructions"). Documented explicitly in `docs/developer_guide/24_skills.md`:

> Activating a skill = trusting whoever authored it. Skills are equivalent to system prompts provided by the user. If the author is hostile, the LLM can be manipulated. Colmena does not validate the semantic content of skill markdown.

**What we do mitigate:**
- LLM can only load skills from the catalog (enforced by `enum` in the tool schema).
- Skills are never executed as code — text only.
- Catalog is fixed at graph load; LLM cannot cause new skills to be loaded at runtime.

### DoS limits

| Limit | Value | Enforced at |
|-------|-------|-------------|
| `SKILL.md` size | 64 KB | Graph load |
| Each reference file size | 64 KB | Graph load |
| Total active skills per node | 50 | Graph load |
| Allowed file extension | `.md` only | Graph load |

## `load_skill` tool

### Tool definition exposed to the LLM

Built dynamically from configured skills:

```json
{
  "name": "load_skill",
  "description": "Load a specialized knowledge skill on demand when the user's task benefits from it. Call this tool BEFORE responding when you identify that one of the skills below applies. You may call it multiple times to load several skills or to load a skill's reference material.\n\nAvailable skills:\n- python-expert: Use when the user asks about Python typing, async patterns, or Python standard library internals. Not for general programming questions.\n- security-review: Use when the user requests a security audit of code...\n\nAfter loading a skill, if its content mentions available references, you may call load_skill again with the `reference` parameter to load that additional material.",
  "parameters": {
    "type": "object",
    "properties": {
      "name": {
        "type": "string",
        "description": "The name of the skill to load",
        "enum": ["python-expert", "security-review"]
      },
      "reference": {
        "type": "string",
        "description": "Optional name of a reference file within the skill. Only use after loading the skill and seeing it declares this reference."
      }
    },
    "required": ["name"]
  }
}
```

The catalog lives inside the tool description — the `system_message` is not modified. This keeps skills co-located with the affordance that uses them.

### Tool output (returned to the LLM)

**Load main SKILL.md (no reference):**

```
[SKILL.md body, with frontmatter stripped]

---

Available references for this skill:
- frameworks: Detailed information about Django, FastAPI, and Flask patterns
- testing: pytest fixtures, mocking strategies, and test organization

To load a reference, call load_skill again with the `reference` parameter.
```

If the skill has no `references` in its frontmatter, the trailing block is omitted.

**Load a reference:**

```
[Literal content of references/<name>.md]
```

**Errors (returned as structured tool output so the LLM sees them):**

```
Error: skill 'X' does not declare a reference named 'foo'.
Available references: frameworks, testing
```

### Dispatch integration (Option A from brainstorming)

`DagToolExecutor.execute()` gains one conditional: if the tool name is `load_skill`, dispatch to the `SkillRepository`; otherwise normal dispatch. Simple and localized. If a second synthetic tool appears later (e.g., `memory.recall`), refactor to a `SyntheticTool` trait with two concrete cases in hand.

## End-to-end execution flow

```
1. LlmNode.execute() starts
   ├─ Parse config.skills
   ├─ Validate (names, frontmatter, files, sizes, allowed_dirs) — fail fast
   ├─ Construct CompositeSkillRepository
   └─ If at least one skill, register load_skill in the tool registry

2. AgentService.run() — iteration 1
   ├─ LLM receives: system_message + tools (including load_skill with catalog)
   ├─ LLM decides: "I need python-expert"
   └─ Tool call: load_skill(name="python-expert")

3. ToolExecutor dispatches
   ├─ Detects load_skill (synthetic tool)
   ├─ Calls skill_repository.load_skill("python-expert")
   ├─ Emits SSE event "skill_loaded" (see Observability)
   └─ Returns body as tool result

4. AgentService — iteration 2
   ├─ Messages now: [system, user, assistant-tool-call, tool-result]
   ├─ LLM reads the skill and responds (or requests another skill / reference)

5. Eventually LLM returns final answer → loop terminates
```

## Observability

### SSE stream

When the LLM invokes `load_skill`, three events are emitted:

```
event: tool_call
data: {
  "tool_name": "load_skill",
  "tool_call_id": "call_abc123",
  "args": {"name": "python-expert"}
}

event: skill_loaded                       ← new enriched event
data: {
  "tool_call_id": "call_abc123",
  "skill_name": "python-expert",
  "reference": null,
  "source": "builtin",
  "size_bytes": 2450
}

event: tool_result
data: {
  "tool_call_id": "call_abc123",
  "output": "<skill content>"
}
```

`tool_call` and `tool_result` fire automatically because `load_skill` is a tool like any other — no new code needed there. `skill_loaded` is the single new event type, emitted only by the `load_skill` dispatcher. Frontends that don't care about skills ignore it; frontends that want distinctive UI filter on it.

`source` values: `"builtin"` or `"path"`.

### Final summary

The executor's final summary gains a `skills_used` section, shown only when at least one skill was loaded:

```json
{
  "tokens": {"input": 1240, "output": 892},
  "tool_calls": [
    {"name": "load_skill", "count": 2},
    {"name": "create_blog_post", "count": 1}
  ],
  "skills_used": [
    {
      "name": "python-expert",
      "source": "builtin",
      "references_loaded": ["frameworks"],
      "load_count": 1
    }
  ]
}
```

If no skills were loaded (or the node didn't configure any), the `skills_used` field is **absent** — not an empty array.

## Error handling

### Validated at graph load (fail fast)

| Condition | Error |
|-----------|-------|
| `SKILL.md` missing | `SkillNotFound` |
| `SKILL.md` > 64 KB | `FileTooLarge` |
| Frontmatter invalid YAML | `InvalidFrontmatter` |
| `name` field missing | `MissingField("name")` |
| `description` missing | `MissingField("description")` |
| `name` ≠ directory name | `NameMismatch` |
| Reference declared but file missing | `ReferenceFileMissing` |
| Reference file > 64 KB | `FileTooLarge` |
| > 50 total skills | `TooManySkills` |
| Path outside allowed_dirs | `PathNotAllowed` |
| Path is not a directory | `NotADirectory` |
| Name collision between any two skills | `SkillNameCollision` |

### Runtime errors (returned as tool output)

| Condition | Tool output |
|-----------|-------------|
| LLM requests unknown skill (defensive, should be blocked by `enum`) | `Error: skill 'X' not found. Available: ...` |
| LLM requests reference not declared in that skill | `Error: skill 'X' does not declare reference 'Y'. Available: ...` |

## Files

### New files

| Path | Purpose |
|------|---------|
| `src/libs/colmena/skills/` | Built-in skills directory (markdown, compiled via `include_dir!`) |
| `src/libs/colmena/skills/README.md` | Contributor guide for adding skills |
| `src/libs/colmena/skills/python-expert/SKILL.md` | First built-in skill (sample) |
| `src/libs/colmena/skills/python-expert/references/frameworks.md` | Sample reference |
| `src/libs/colmena/skills/sql-optimizer/SKILL.md` | Second built-in skill (sample) |
| `src/libs/colmena/skills/sql-optimizer/references/query-plans.md` | Sample reference |
| `src/libs/colmena/src/skills/mod.rs` | Module entry |
| `src/libs/colmena/src/skills/domain/mod.rs` | Domain entry |
| `src/libs/colmena/src/skills/domain/skill.rs` | `Skill`, `SkillReference` value objects |
| `src/libs/colmena/src/skills/domain/skill_repository.rs` | `SkillRepository` trait |
| `src/libs/colmena/src/skills/domain/skill_config.rs` | `SkillConfig`, `SkillSource`, parsing |
| `src/libs/colmena/src/skills/domain/skill_error.rs` | `SkillError` (thiserror) |
| `src/libs/colmena/src/skills/infrastructure/mod.rs` | Infrastructure entry |
| `src/libs/colmena/src/skills/infrastructure/frontmatter_parser.rs` | YAML frontmatter + body split |
| `src/libs/colmena/src/skills/infrastructure/builtin_skill_repository.rs` | Uses `include_dir!` |
| `src/libs/colmena/src/skills/infrastructure/filesystem_skill_repository.rs` | Validates against allowed_dirs |
| `src/libs/colmena/src/skills/infrastructure/composite_skill_repository.rs` | Merges builtin + filesystem |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` | Submodule entry |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs` | `load_skill` tool logic |
| `tests/graphs/agents/skills_basic.json` | E2E test graph |
| `tests/skills_integration.rs` | Integration tests with mocked LLM |
| `docs/developer_guide/24_skills.md` | User-facing guide |

### Modified files

| Path | Change |
|------|--------|
| `src/libs/colmena/src/lib.rs` | `pub mod skills;` |
| `src/libs/colmena/Cargo.toml` | Add `include_dir = "0.7"`, `serde_yaml = "0.9"` |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Resolve `skills` from config; build `SkillRepository`; register `load_skill` tool if present; include `skills_used` in summary |
| `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` | Conditional dispatch for `load_skill` |
| `docs/node_configurations.json` | Add `skills` schema under `llm_call` |
| `docs/developer_guide/14_llm_deep_dive.md` | Add section referencing `24_skills.md` |
| `docs/DEVELOPER_GUIDE.md` | Index entry for `24_skills.md` |
| `CLAUDE.md` | Add `24_skills.md` to the doc list |

### Dependencies added

```toml
include_dir = "0.7"        # compile built-in skills into the binary
serde_yaml = "0.9"         # parse frontmatter
```

### Not modified

- LLM provider adapters (`openai_adapter.rs`, `anthropic_adapter.rs`, `gemini_adapter.rs`) — skills ride the existing tool-calling path.
- `AgentService` (`application/agent_service.rs`) — ReAct loop is tool-agnostic.
- `python_bindings/`, `node_bindings/` — deferred (users can pass `skills` via dict, `serde_json` path works).

## Testing

### Rust unit tests (inline `#[cfg(test)]`)

**`frontmatter_parser`:**
- Valid frontmatter with all fields → extracts correctly, strips from body.
- No `references` field → `references: []`.
- Empty body after frontmatter → valid.
- Malformed YAML → `InvalidFrontmatter`.
- Missing `name` / `description` → `MissingField`.
- No frontmatter at all → `MissingFrontmatter`.
- Body contains `---` separators unrelated to frontmatter → not confused with boundaries.

**`filesystem_skill_repository`:**
- Path outside allowed_dirs → `PathNotAllowed`.
- Symlink escaping allowed_dirs → `PathNotAllowed`.
- Path is a file, not a directory → `NotADirectory`.
- Directory without `SKILL.md` → `SkillNotFound`.
- `SKILL.md` > 64 KB → `FileTooLarge`.
- Reference declared but missing → `ReferenceFileMissing`.
- Reference file > 64 KB → `FileTooLarge`.
- `name` mismatch with directory → `NameMismatch`.

**`builtin_skill_repository`:**
- `list_available` returns all embedded skills.
- `load_skill` of known built-in returns correct body.
- `load_skill` of unknown → `SkillNotFound`.

**`composite_skill_repository`:**
- Collision between builtin and path with same name → construction error.
- Collision between two paths with same name → construction error.
- > 50 total skills → `TooManySkills`.
- `list_available` returns union.

**`load_skill_tool`:**
- `ToolDefinition` includes all names in `enum`.
- Description includes each skill with its description.
- Dispatch with `name=X` no reference → body + references-available block when applicable.
- Dispatch with `name=X, reference=Y` → reference body.
- Dispatch with undeclared reference → structured error in output.

### Integration test (`tests/skills_integration.rs`)

- Graph with one built-in skill, `MockAdapter` configured to tool-call `load_skill("python-expert")` → verify output contains skill body.
- Graph with skill from path → verify filesystem read works.
- Graph with invalid skill (missing file) → verify graph load error.

Uses `mockall` for `LlmRepository` so tests don't hit real APIs.

### End-to-end test graph

`tests/graphs/agents/skills_basic.json`:

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/test-skills",
        "method": "POST",
        "test_payload": {
          "prompt": "Explain how to write a typed async function in Python 3.11 with proper error handling"
        }
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-4o-mini",
        "system_message": "You are a helpful coding assistant.",
        "skills": {
          "builtin": ["python-expert", "sql-optimizer"]
        }
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    {"from": "trigger", "to": "agent"},
    {"from": "agent", "to": "log"}
  ]
}
```

### Success criteria for E2E

- Log shows `skill_loaded` event with `skill_name: "python-expert"`.
- Log does NOT show `skill_loaded` for `sql-optimizer` (the LLM must discriminate).
- Final LLM response reflects Python 3.11 async + type-hints knowledge consistent with `python-expert/SKILL.md`.

### Validation commands

```bash
cargo test --lib skills
cargo test --lib load_skill_tool
cargo test --test skills_integration
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/skills_basic.json
```

### Not tested

- Real OpenAI/Anthropic/Gemini APIs in unit or integration tests (only in manual E2E).
- Frontmatter fuzzing (handled upstream by `serde_yaml`).
- Benchmarks (not performance-critical; measure if needed later).

## Open questions / deferred

- **Prompt caching:** when we enable Anthropic `cache_control`, skills become an obvious candidate (static, large). Deferred until caching is a broader priority.
- **Python/TS bindings first-class support:** add typed `skills` parameter to PyO3 / napi-rs structs once the JSON-DAG implementation has stabilized.
- **Skill families / tags:** could be added as an optional frontmatter field `family: "python"` plus a config option `skills.families: ["python"]`. Not in v1.
- **Skills from remote sources (S3, HTTP):** possible via a new `SkillRepository` impl. Deferred.
- **Versioning of skills:** no `version` field in v1. If needed later, add to frontmatter.

## Trust model (to be documented in `24_skills.md`)

> Activating a skill is equivalent to injecting a system prompt authored by someone else. Colmena validates that skills are syntactically correct and live in allowed directories, but does **not** validate their semantic content. A hostile skill can mislead the LLM. Only activate skills from authors you trust.
