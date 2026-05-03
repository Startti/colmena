# Lazy Tool Loading — Design

**Date:** 2026-05-03
**Status:** Approved for planning
**Related:** [docs/developer_guide/24_skills.md](../../developer_guide/24_skills.md) (skills feature, shares internal infrastructure)

---

## Goal

Allow an `llm_call` node to expose many tools (10+, even 50+) without paying the cost of injecting every tool's full parameter schema into every request. The LLM sees a lightweight catalog up front and only "discovers" the full schema of tools it actually intends to use, via a synthetic tool `describe_tool`.

## Motivation

Today, every tool in `tool_configurations` is registered eagerly: the full `name`, `description`, and `node_schema` go into `tools[]` of every provider request. Empirically, models start mis-routing or hallucinating arguments when the tool list grows past ~10 entries — even when the conversation only needs 1-2 of them. The competing schemas dilute attention.

The skills feature already proved the pattern: a synthetic loader (`load_skill`) reveals heavy content on demand. This design applies the same idea to tool definitions, with the twist that revealing a tool must also make it callable in subsequent turns.

## Architecture overview

The mechanism is **progressive registration**: the provider's `tools[]` array grows as the LLM calls `describe_tool`. Each call adds one tool name to a per-session `discovered_set`; subsequent requests rebuild `tools[]` to include that tool's full schema.

```
tools[] = [describe_tool if any pending]   // synthetic loader
        + [tools with eager: true]          // explicit always-on
        + [tools in discovered_set]         // revealed during this conversation
```

`discovered_set` lives in `LlmNode` state during the agent loop. With memory enabled, it is reconstructed at session start by scanning the conversation history for prior `describe_tool` tool calls — no new database schema.

The `describe_tool` interception happens in `DagToolExecutor::execute`, before the node-type lookup, exactly mirroring how `load_skill` is intercepted. An observer callback notifies `LlmNode` to insert the discovered name into `discovered_set` and emit an SSE event.

Backwards compatibility is total: when `tool_loading` is absent or set to `"eager"`, the engine behaves identically to today. The lazy path is opt-in at the `llm_call` level.

## Configuration schema

### `llm_call.config`

One new optional field:

```jsonc
{
  "type": "llm_call",
  "config": {
    "tool_loading": "lazy",       // absent | "eager" | "lazy"
    "tool_configurations": { ... }
  }
}
```

- Absent or `"eager"` → current behavior. `describe_tool` is not registered. `discovered_set` does not exist.
- `"lazy"` → activates the feature.

### `ToolConfiguration`

Two new fields, both optional:

```jsonc
{
  "name": "search_orders",
  "summary": "Find historical orders. Use when the user asks about past purchases.",
  "description": "Search the orders table by date range, status, customer ID, or product SKU. Returns up to 100 rows sorted by created_at descending. Combine with lookup_invoice to fetch line items. Date format: ISO 8601...",
  "node_type": "sql_query",
  "node_schema": { ... },
  "fixed_config": { ... },
  "eager": false                  // optional, default false
}
```

- **`summary`** (optional, string) — used in the catalog inside `describe_tool`'s description. Must be ≤ 200 chars; longer values trigger a warning at graph load and are truncated.
- **`eager`** (optional, boolean, default `false`) — only meaningful when `tool_loading: "lazy"`. A tool with `eager: true` is registered with its full schema in every request and does **not** appear in the `describe_tool` catalog. Use for tools called in (almost) every turn.

### Validation rules at graph load

| Condition | Action |
|-----------|--------|
| `tool_loading: "lazy"` and `tool_configurations` empty | Warning. `describe_tool` is not injected; lazy mode silently no-ops. |
| `summary` > 200 chars | Warning. Truncated to 200 chars (word boundary). |
| `summary` absent | Fallback: first ~120 chars of `description`, cut on word boundary. |
| `eager: true` without `tool_loading: "lazy"` | Silently ignored. Flag has no effect outside lazy mode. |

`enabled_tools` continues to work unchanged: it filters which tool configurations participate. Tools filtered out by `enabled_tools` enter neither the catalog nor the eager list.

## The `describe_tool` synthetic tool

### Definition exposed to the provider

```jsonc
{
  "name": "describe_tool",
  "description": "Reveal the full parameter schema and usage notes for one of the tools below. Call this BEFORE invoking a tool so you know its parameters and return shape. Available tools:\n- search_orders: Find historical orders. Use when the user asks about past purchases.\n- lookup_invoice: Retrieve invoice details by ID. Use after search_orders.\n- send_email: Send transactional emails via SMTP.\n\nOnly call describe_tool when you've decided you actually need the tool — not preemptively for every tool.",
  "parameters": {
    "type": "object",
    "properties": {
      "name": { "type": "string", "enum": ["search_orders", "lookup_invoice", "send_email"] }
    },
    "required": ["name"]
  }
}
```

The catalog (description text + enum) is rebuilt on every request from `(catalog_entries) − (discovered_set)`. As tools are revealed, they leave the catalog. When `pending == ∅`, `describe_tool` is omitted entirely from `tools[]`. Catalog entries are sorted alphabetically for determinism.

### Output of `describe_tool("search_orders")`

A curated markdown document, returned as the tool result body:

```markdown
# search_orders

Search the orders table by date range, status, customer ID, or product SKU. Returns up to 100 rows sorted by created_at descending. Combine with lookup_invoice to fetch line items.

## Parameters

| Name | Type | Required | Description |
|------|------|----------|-------------|
| start_date | string | yes | ISO 8601 date "YYYY-MM-DD" |
| end_date | string | yes | ISO 8601 date "YYYY-MM-DD" |
| status | string | no | One of: pending, completed, cancelled |

## Returns
JSON array of `{order_id, customer_id, total, status, created_at}`.

---
The tool `search_orders` is now available. Call it directly on your next turn.
```

Generation rules:

- Only LLM-visible parameters are listed. Fields with `fixed: true` in `node_schema` or fields populated entirely by `fixed_config` are filtered out.
- Fields marked `secure: true` are omitted from the markdown entirely (never leak the existence of a secret slot).
- If `node_schema` is absent or empty, the markdown lists no parameter table and instead notes "No parameter schema declared — pass arguments as a free-form JSON object that matches the tool's expectations."
- The trailing line `"The tool ... is now available. Call it directly on your next turn."` is the semantic anchor that tells the LLM the discovery succeeded and the tool will appear typed in the next request's `tools[]`.

### Dispatch

Mirrors `load_skill`. In `DagToolExecutor::execute`, before the `node_type` lookup:

```rust
if tool_call.name == DESCRIBE_TOOL_NAME {
    let result = dispatch_describe_tool(&tool_call, &tool_catalog);
    // observer callback notifies LlmNode → inserts name into discovered_set
    if let Some(observer) = &self.tool_describe_observer {
        observer(&result);
    }
    return Ok(into_tool_result(tool_call.id, &result));
}
```

The observer is `Arc<dyn Fn(&DescribeToolDispatchResult) + Send + Sync>`, parallel to the existing `SkillObserver` type alias. It runs synchronously inside `execute` and updates the `Arc<Mutex<HashSet<String>>>` held by `LlmNode`.

### Idempotency

The `enum` in the `name` parameter excludes already-discovered tools, so the LLM **cannot** call `describe_tool("X")` twice for the same X — the provider rejects the second call before it reaches the executor. Defensive handling in dispatch returns `tool_result` with an error message if it ever does (e.g. an LLM that bypasses provider validation).

### Errors

| Case | When | Result |
|------|------|--------|
| Invalid `name` (not in catalog) | Provider rejects via `enum` validation | Executor never sees it. Defensive fallback: return `tool_result` with `"Error: Tool 'X' not found in catalog"`. |
| Tool config corrupt (markdown gen fails) | At dispatch time | Return `tool_result` with `"Error: Could not generate documentation for tool 'X'"`. Log details. |

## Multi-turn flow + persistence

### Per-request rebuild of `tools[]`

The `LlmNode` already rebuilds the request on every iteration of the ReAct loop in `AgentService`. The change is local to `tools[]` construction:

```rust
let tools = if tool_loading == ToolLoading::Lazy {
    let pending: Vec<&CatalogEntry> = catalog.iter()
        .filter(|e| !discovered_set.contains(&e.name))
        .collect();

    let mut t = Vec::new();
    if !pending.is_empty() {
        t.push(build_describe_tool_definition(&pending));
    }
    for cfg in tool_configurations.values() {
        if cfg.eager || discovered_set.contains(&cfg.name) {
            t.push(build_tool_definition(cfg));
        }
    }
    t
} else {
    build_all_tools_eager(&tool_configurations)  // existing path
};
```

`describe_tool_definition` and `catalog` are computed once at graph load (catalog) plus once per turn (description text, since `pending` shrinks).

### Persistence with memory

When the `llm_call` has `session_id` + `connection_url`, the conversation history is loaded at the start of the call. Reconstruction of `discovered_set` is a derived view over that history:

```rust
let discovered_set: HashSet<String> = messages.iter()
    .flat_map(|msg| msg.tool_calls.iter())
    .filter(|tc| tc.tool_name == DESCRIBE_TOOL_NAME)
    .filter_map(|tc| serde_json::from_str::<DescribeArgs>(&tc.arguments).ok())
    .map(|args| args.name)
    .collect();
```

No new database schema. If the conversation is rolled (truncation policy), tools that fall out of the visible window leave `discovered_set` and the LLM re-discovers them naturally the next time it needs them.

### Edge case: same-turn discover-and-call

A model may emit two parallel tool calls in one assistant response: `describe_tool(X)` and `X(args)`. In that request, `X` was not in `tools[]`, so the provider rejects the second call with an "unknown tool" error. The turn is consumed and the LLM retries the next turn, this time with `X` in `tools[]`.

This is rare in practice — the model typically can't construct valid args for `X` without first reading `describe_tool`'s output. The `describe_tool` markdown explicitly says *"Call it directly on your next turn"* to discourage the pattern. Documented in the developer guide as a known behavior, not a bug.

## Observability

### SSE event

One new variant on `DagExecutionEvent`:

```rust
DagExecutionEvent::ToolDescribed {
    node_id: String,
    tool_id: String,         // id of the describe_tool call
    tool_name: String,       // e.g. "search_orders"
}
```

Mapped in `main.rs` and `api.rs` to the data-stream-protocol line:

```jsonc
{ "type": "tool-described", "nodeId": "...", "toolCallId": "...", "toolName": "search_orders" }
```

Frontends can render this as an intermediate step ("Discovering tool: search_orders") distinct from the actual tool execution that follows.

### Final summary (`extra_info`)

`extra_info` gains a new optional field:

```jsonc
{
  "extra_info": {
    "tool_calls": [...],          // existing
    "skills_used": [...],         // existing
    "tools_discovered": ["search_orders", "lookup_invoice"]
  }
}
```

Present only when `tool_loading: "lazy"` and at least one tool was discovered. Plain array of names in discovery order. `load_count` is not tracked because the `enum` makes `describe_tool` calls effectively unique per name within a session.

## Internal architecture: shared infrastructure with skills

`load_skill` and `describe_tool` share enough mechanical structure that the implementation should factor common pieces. **The LLM-facing API stays as two separate synthetic tools** — no unified `load_resource` or `load(name)` parameter. The unification is internal:

| Shared internally | Per-feature |
|-------------------|-------------|
| Catalog entry rendering helper (sort + format `name: summary`) | Catalog source (skill repository vs tool_configurations) |
| Observer callback type pattern (`Arc<dyn Fn(&T) + Send + Sync>`) | Observer payload (`LoadSkillDispatchResult` vs `DescribeToolDispatchResult`) |
| Synthetic tool intercept slot in `DagToolExecutor::execute` | Tool name constant and dispatch fn |
| `extra_info` aggregation pattern (`Arc<Mutex<Vec<...>>>`) | Aggregation field name (`skills_used` vs `tools_discovered`) |

Both live under `dag_engine/infrastructure/nodes/llm_synthetic_tools/`. The directory currently contains `load_skill_tool.rs`; this feature adds `describe_tool.rs` plus a small `catalog.rs` for the shared rendering helper if extraction proves clean.

Rationale for keeping API separate (rejecting unified loader): skills produce text injection (passive context); tools produce a tool-registration side effect (next turn `tools[]` changes). The output semantics differ, the lifecycle differs, and the catalog content differs. A unified `load(name)` would force the LLM to triage names across heterogeneous resource kinds and conflict-check namespaces, with no tangible win for the user.

## Tests

**Unit:**
- `build_describe_tool_definition` shrinks the catalog and enum correctly as `discovered_set` grows.
- Markdown generator filters `fixed`, omits `secure` fields, handles missing `node_schema`.
- `summary` parsing: present, absent (fallback truncate), too long (warning + truncate).
- `discovered_set` reconstruction from a synthetic conversation history.

**Integration (`tests/lazy_tools_integration.rs`):**
- Agent loop with `MockAdapter` simulating a 3-turn conversation: turn 1 emits `describe_tool("X")`, turn 2 emits `X(args)`, turn 3 finishes. Assert `tools[]` contents at each request — describe_tool present in turn 1 (X in catalog), describe_tool present in turn 2 (X removed from catalog, added as full schema), describe_tool absent if catalog empty after turn 2.
- Reconstruction: build a conversation with prior `describe_tool` calls, instantiate a fresh `LlmNode`, assert `discovered_set` matches.

**E2E:**
- `tests/graphs/agents/tools_lazy_basic.json` — `tool_loading: "lazy"` with 3 tools (1 eager, 2 lazy). Prompt that only triggers one of the lazy tools. Verify SSE stream contains `tool-described` for that one tool only, and final summary has `tools_discovered: ["..."]`.

## Errors and validation summary

Recapped from earlier sections, all in one table for cross-reference:

| Case | Severity | Action |
|------|----------|--------|
| `tool_loading: "lazy"` with empty `tool_configurations` | Warning | Lazy mode no-ops; `describe_tool` not injected. |
| `summary` > 200 chars | Warning | Truncate at 200 chars on word boundary. |
| `summary` absent | Info (no log) | Fallback to first ~120 chars of `description`. |
| `eager: true` without lazy mode | Info (no log) | Flag ignored. |
| Invalid tool name in `describe_tool` call | Defensive only — provider blocks via enum | Return `tool_result` with explicit error. |
| Markdown generation failure (corrupt config) | Error in result | Return `tool_result` with error; log full context. |

## Trust model

Same posture as the skills feature: the engine validates structure (schema validity, summary length) but never validates semantic content. A `summary` written to mislead the LLM (prompt injection) is the configuring author's responsibility. The catalog is fixed at graph load — the LLM cannot add new tools at runtime, only reveal entries from the predefined catalog.

## Non-goals

- **Prompt caching of `describe_tool`'s description.** The current design intentionally rebuilds the catalog per turn (filtering `discovered_set`), which cache-busts. To enable caching later, switch to a "static catalog + dynamic tail" pattern: keep the full catalog stable in the description and append a small "already revealed" notice. This relaxes the enum-as-hard-guard ("LLM cannot re-describe") to a soft norm ("LLM is told not to re-describe"). Out of scope for v1.
- **Cross-session sharing of `discovered_set`.** Each `session_id` is independent.
- **Auto-recommendation of which tools to describe.** No router. The LLM decides.
- **Eviction from `discovered_set` within a session.** No TTL or LRU.
- **Semantic validation of `summary` / `description` content.** No prompt-injection scan.
- **Multimodal `describe_tool` outputs.** Markdown only.

## Open implementation questions

These are best resolved during planning, not specification:

1. Where exactly to draw the boundary for the shared `catalog.rs` helper — minimum viable extraction first, refactor on second feature.
2. Whether to extract `summary` parsing into a tiny `summary.rs` or inline it in tool config validation. Lean inline.
3. Markdown table generation — handwritten format vs. pulling in a markdown crate. Lean handwritten (the format is fixed and small).
