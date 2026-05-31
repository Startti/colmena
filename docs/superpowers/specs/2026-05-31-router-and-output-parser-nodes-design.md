# Router & Output Parser Nodes — Design

**Status:** Draft — pending implementation
**Author:** daniel@startti.co
**Date:** 2026-05-31
**Scope:** Two new DAG nodes for Colmena — `output_parser` and `router`

---

## 1. Motivation

Two recurring needs not covered by existing nodes:

1. **Standalone structured output extraction.** Agents and `llm_call` nodes often produce free-form text. Today, turning that text into structured JSON requires `information_extraction` — a node designed around multiple `texts.{name}` input sources and orchestrator integration. For the common "single upstream text → one structured object" case, the UX is heavy: operators want a thin wrapper they can chain right after an `llm_call`.

2. **Declarative conditional routing.** The DAG engine has no dedicated branching node. Today, branching is done by:
   - Setting `loop_status` config on each node (gating in cyclic graphs only).
   - Returning `None` from a `python_node` (per-edge skip — but no explicit branch labels).
   - Using a `critic` (binary `task_ok` only).

   None of these support a "switch over N labeled branches" pattern. Agents commonly need: *"based on this user message, route to the sales agent, support agent, or billing agent."* That is what this node provides.

Both nodes are independent and can be implemented in parallel.

---

## 2. Goals & Non-Goals

### Goals

- Ship `output_parser` as a thin, well-named wrapper around the existing extraction engine.
- Ship `router` with two modes:
  - **A. LLM-direct** — LLM reads the input and descriptions of each branch, picks one by name.
  - **B. Extract + rules** — LLM extracts a JSON object against a schema; deterministic DSL rules then pick the branch.
- Support optional inline subgraph execution per branch.
- Fail fast on unrecoverable conditions (no silent skips for missing input, no fallback default branch).
- Reuse the schema-from-inline-fields convention already used in `tool_configurations.node_schema` so operators learn one shape.

### Non-Goals

- A non-LLM "deterministic-only" parser (operators can chain `python_node` for that).
- A non-LLM router. Routing without an LLM step would degenerate to a `python_node` decision and offers little value over what already exists.
- Dynamic branch enumeration at runtime (number of branches is fixed by config).
- Streaming the LLM call. Both nodes are request/response.
- Integration with `loop_status` beyond inheriting the standard `loop_status` config field every node already supports.

---

## 3. `output_parser` Node

### 3.1 Identity

| Field | Value |
|---|---|
| `node_type` | `output_parser` |
| `category` | `llm_ai` |
| Default input port | `input` |
| Default output port | none — emits raw extracted JSON (no `{output: ...}` wrapper) |

### 3.2 Config

```json
{
  "type": "output_parser",
  "config": {
    "provider": "google",
    "model": "gemini-2.5-flash",
    "api_key": "${GEMINI_API_KEY}",
    "schema": {
      "intent":     { "type": "string", "required": true,  "description": "User intent: sales | support | billing" },
      "confidence": { "type": "number", "required": false, "description": "0.0 to 1.0" },
      "summary":    { "type": "string", "required": false, "description": "One-line summary" }
    },
    "instructions": "Si no puedes determinar el intent, usa 'unknown'.",
    "temperature": 0.1
  }
}
```

`provider`, `model`, `api_key`, `temperature`, and other LLM knobs follow `llm_shared_fields` — same semantics as `llm_call` and `information_extraction`.

`schema` uses the **inline-required convention**: each field declares its own `type`, `required`, and `description`. This matches `tool_configurations.node_schema` rather than standard JSON Schema. Internally the node converts this inline form to a standard JSON Schema (`{ type: "object", properties: {...}, required: [...] }`) before calling the LLM, because all provider structured-output APIs expect the standard form.

`instructions` is appended to the built-in extraction system prompt (same field as `information_extraction.instructions`).

### 3.3 Ports

| Port | Type | Direction | Description |
|---|---|---|---|
| `input` | any | in (default) | Text or value to parse. Non-string values are serialized to JSON before being sent to the LLM. |
| (default output) | object | out | The extracted JSON matching the schema. Downstream nodes read fields with dotted paths (e.g., `parser.intent`). |

### 3.4 Behavior

1. Read `input`. If missing/empty (see §6.2 for the definition of "empty"), fail with `OutputParserRuntimeError: missing input — nothing to parse`.
2. Serialize non-string `input` to pretty JSON.
3. Convert the inline schema to standard JSON Schema.
4. Build the extraction prompt (system message: built-in extraction template + `instructions`; user message: the input).
5. Force `temperature = 0.1` unless explicitly overridden in config.
6. Call the LLM, strip any markdown code fences from the response, parse JSON.
7. Validate the parsed object against the schema (presence of required fields, type matches per field). On mismatch, fail with `OutputParserRuntimeError: schema validation failed: <details>`.
8. Emit the parsed object as the node's output.

### 3.5 Differences from `information_extraction`

| Aspect | `output_parser` | `information_extraction` |
|---|---|---|
| Input shape | Single `input` port | Multiple `texts.{name}` ports + static `texts` config |
| Schema format | Inline-required | Standard JSON Schema |
| Orchestrator integration | None | Supports `add_tasks` / `delete_tasks` mutations |
| Missing input | Hard error | Silent skip |
| Default use case | Chained after one LLM/agent | Multi-source extraction inside orchestrators |

Both nodes share an internal `extract_with_schema(input_text, json_schema, llm, instructions)` helper that owns the prompt template, the JSON cleanup, and the validation pass. `information_extraction` keeps its outer texts-collection logic; `output_parser` is a thinner caller of the same core.

---

## 4. `router` Node

### 4.1 Identity

| Field | Value |
|---|---|
| `node_type` | `router` |
| `category` | `control_flow` |
| Default input port | `input` |
| Default output port | none — emits per-branch ports + `__decision` |

### 4.2 Modes

| Mode | Config trigger | Decision source |
|---|---|---|
| **A. LLM-direct** | `mode: "llm_direct"` | LLM picks `branch_name` from the enum of declared branches based on `description` per branch. |
| **B. Extract + rules** | `mode: "extract_and_route"` | LLM extracts a JSON against `schema`; declarative `when` rules per branch decide. |

### 4.3 Shared Config

```json
{
  "type": "router",
  "config": {
    "mode": "extract_and_route",
    "provider": "google",
    "model": "gemini-2.5-flash",
    "api_key": "${GEMINI_API_KEY}",
    "branches": [ /* see §4.5 / §4.6 */ ]
  }
}
```

### 4.4 Ports

| Port | Type | Direction | Description |
|---|---|---|---|
| `input` | any | in (default) | Text or value to route. |
| `<branch_name>` (one per declared branch) | object | out | Emits the routed payload **only** when this branch is selected; emits `null` otherwise. |
| `__decision` | object | out | Always emitted (even on routing errors when possible). Contains `selected_branch`, `extracted?`, `reason?`, `error?`. Used for logging / debugging / event stream. |

**Branch payload shape (emitted on the selected port):**

- Mode A: `{ "input": <original_input> }`
- Mode B: `{ "input": <original_input>, "extracted": <extracted_json> }`

Downstream nodes can read with dotted paths: `from: "router.sales"` (full object) or `from: "router.sales.extracted.confidence"` (single field).

### 4.5 Mode A — LLM-direct

```json
"branches": [
  { "name": "sales",   "description": "User wants to buy, asks for pricing, quotes, or available products." },
  { "name": "support", "description": "User has a technical issue or asks how to use something." },
  { "name": "billing", "description": "Invoices, payments, subscriptions, refunds." }
]
```

The LLM is called with structured output enforcing `branch_name ∈ {sales, support, billing}`. If the provider returns anything outside the enum (jailbroken / off-schema), the node fails with `RouterRuntimeError: llm picked unknown branch 'X'`.

`description` is **required** in mode A. `when` is **forbidden** in mode A.

### 4.6 Mode B — Extract + Rules

```json
{
  "mode": "extract_and_route",
  "schema": {
    "intent":     { "type": "string", "required": true,  "description": "sales | support | billing" },
    "urgency":    { "type": "string", "required": false, "description": "low | medium | high" },
    "confidence": { "type": "number", "required": false, "description": "0..1" }
  },
  "branches": [
    {
      "name": "urgent_sales",
      "when": { "all": [
        { "field": "intent",  "equals": "sales" },
        { "field": "urgency", "equals": "high"  }
      ]}
    },
    { "name": "sales",   "when": { "field": "intent", "equals": "sales" } },
    { "name": "support", "when": { "field": "intent", "in": ["support", "technical"] } },
    { "name": "billing", "when": { "field": "intent", "equals": "billing" } }
  ]
}
```

`when` is **required** in mode B. `description` is **forbidden** (the LLM is not making the routing decision — only extracting).

### 4.7 `when` DSL Grammar

| Form | Meaning |
|---|---|
| `{ field, equals: V }` | `extracted[field] == V` |
| `{ field, not_equals: V }` | `extracted[field] != V` |
| `{ field, in: [V1, V2, ...] }` | `extracted[field] ∈ list` |
| `{ field, contains: V }` | string/array contains V |
| `{ field, gt: N }` / `lt: N` / `gte: N` / `lte: N` | numeric comparison |
| `{ field, matches: "regex" }` | regex match on string |
| `{ field, exists: true }` | field present and non-null |
| `{ all: [<when>, ...] }` | AND |
| `{ any: [<when>, ...] }` | OR |
| `{ not: <when> }` | negation |

- `field` supports dotted paths (e.g., `"user.profile.tier"`).
- `equals` / `not_equals` / `contains` / `in` / `matches` are **type-strict** at runtime — e.g., `{equals: 5}` against `"5"` does not match. The extractor schema already constrains types, so this is consistent.
- Regex uses Rust's `regex` crate. Compiled once at init.

### 4.8 Evaluation Order

Branches are evaluated **in declaration order**. The **first** branch whose `when` matches wins (XOR). This lets operators put specific conditions before general ones (e.g., `urgent_sales` before `sales`).

If no branch matches: `RouterRuntimeError: no branch matched. extracted: {...}`. No silent default. (See §5 for the rationale on no-default.)

### 4.9 Inline Subgraphs per Branch

Either mode may optionally attach a subgraph to a branch:

```json
{
  "name": "sales",
  "when": { "field": "intent", "equals": "sales" },
  "subgraph": {
    "child_graph_path": "graphs/agents/sales_agent.json"
  }
}
```

- Reuses `SubGraphNode` internally.
- `child_graph_path` and `child_graph_inline` are mutually exclusive (validated at init).
- The selected branch's `subgraph` (if any) receives the branch payload (`{input, extracted?}`) as its initial state.
- The subgraph's output is what's emitted on the branch port.
- If the subgraph **suspends**, the SUSPENDED object bubbles up via the branch port (same semantics as a standalone `SubGraphNode`).
- If the subgraph **fails**, the error propagates with prefix `router branch '<name>': <upstream error>`.

---

## 5. Why No Default Branch

Earlier draft considered an optional `default` branch as fallthrough. Final decision: **fail-fast, no default**, for both modes.

**Rationale:**
- Silent fallthrough is a known footgun: when the LLM hallucinates an unexpected `intent` or returns low-confidence garbage, a `default` branch quietly swallows it and the operator only finds out via downstream confusion.
- Hard failure forces operators to explicitly handle the "unknown" case as a real branch (e.g., a `clarify` or `triage_unknown` branch with an explicit `when`).
- Consistent with the project's general bias: explicit > implicit, observable failures > silent recovery.

Operators that want a default can simply declare a final branch with `when: { field: "intent", exists: true }` or `when: { any: [...] }` covering remaining values.

---

## 6. Error Handling

### 6.1 Init Validation (config-time, before execution)

| Check | Error |
|---|---|
| `mode` ∉ `{"llm_direct", "extract_and_route"}` | `RouterConfigError: invalid mode '<X>'` |
| `branches` missing or empty | `RouterConfigError: at least one branch required` |
| Duplicate `branches[].name` | `RouterConfigError: duplicate branch name '<X>'` |
| `name` doesn't match `^[a-z][a-z0-9_]{0,63}$` | `RouterConfigError: invalid branch name '<X>'` |
| Mode A: any branch missing `description` | `RouterConfigError: llm_direct requires description per branch` |
| Mode A: any branch with `when` | `RouterConfigError: 'when' not allowed in llm_direct mode` |
| Mode B: `schema` missing or empty | `RouterConfigError: extract_and_route requires schema` |
| Mode B: any branch missing `when` | `RouterConfigError: extract_and_route requires 'when' per branch` |
| Mode B: `when.field` references a field not declared in `schema` | `RouterConfigError: 'when' references unknown field '<X>'` |
| `subgraph` declares both `child_graph_path` and `child_graph_inline` | `RouterConfigError: pick one subgraph source` |
| `subgraph.child_graph_path` not readable | Surfaced by the inner `SubGraphNode` at init |

`output_parser` init validation:

| Check | Error |
|---|---|
| `schema` missing or empty | `OutputParserConfigError: schema required` |
| `schema` field with invalid `type` (not in the supported set) | `OutputParserConfigError: invalid type for field '<X>'` |
| LLM provider fields invalid (missing `api_key`, etc.) | Surfaced by the shared LLM init path |

### 6.2 Runtime Errors (unrecoverable — node fails, engine propagates)

**Definition of "missing/empty input"** (applies to both nodes):

- JSON `null`
- String empty after trim
- Array `[]`
- Object `{}`

Any other value (including `0`, `false`, `"any text"`) is valid input.

| Case | Node | Error |
|---|---|---|
| `input` missing/empty | both | `<Node>RuntimeError: missing input — nothing to parse/route` |
| LLM call fails (network / 5xx / timeout) | both | `<Node>RuntimeError: llm call failed: <upstream>` |
| Mode A: LLM picks branch outside enum | router | `RouterRuntimeError: llm picked unknown branch '<X>'` |
| Mode B / parser: LLM returns invalid JSON or schema mismatch | both | `<Node>RuntimeError: extraction failed: <details>` |
| Mode B: no `when` matches | router | `RouterRuntimeError: no branch matched. extracted: {...}` |
| Selected branch's subgraph fails | router | propagates with prefix `router branch '<name>':` |
| Selected branch's subgraph SUSPENDS | router | SUSPENDED object emitted via the branch port (not an error — same as standalone `SubGraphNode`) |

### 6.3 `__decision` on Errors

When a routing error occurs *after* extraction has succeeded (e.g., no branch matched, or the LLM picked an unknown branch), the node emits `__decision` with whatever is known before raising:

```json
{
  "selected_branch": null,
  "extracted": { "intent": "weird_value" },
  "error": "no branch matched. extracted: {\"intent\": \"weird_value\"}"
}
```

When the failure happens *before* the LLM responds (network error, timeout), no `__decision` is emitted — the node fails seco.

---

## 7. Internal Architecture

### 7.1 Files

```
src/libs/colmena/src/dag_engine/infrastructure/nodes/
  output_parser.rs         # new — OutputParserNode
  router/
    mod.rs                 # RouterNode + ExecutableNode impl
    config.rs              # config types + init validation
    when_dsl.rs            # WhenRule enum + evaluator
    llm_direct.rs          # mode A logic
    extract_and_route.rs   # mode B logic
  util/
    inline_schema.rs       # new — convert inline-required → JSON Schema (shared)
    extract_with_schema.rs # new — extracted from extraction.rs, reused
```

`router/` is a directory because the implementation is large enough that splitting per mode + the DSL evaluator keeps each file focused. `output_parser.rs` is a single file (thin wrapper). The two new shared helpers live under the existing `nodes/util/` directory to match the project's convention for cross-node utilities.

### 7.2 Shared Helpers

- **`inline_schema_to_json_schema(inline: &Value) -> Result<Value>`** — converts `{ field: { type, required, description } }` to standard JSON Schema. Used by `output_parser` and `router` mode B. Lives in `nodes/shared/inline_schema.rs`. Tested in isolation.

- **`extract_with_schema(input_text, json_schema, instructions, llm) -> Result<Value>`** — the core extraction logic factored out of `extraction.rs`. Builds the prompt, calls the LLM, strips markdown fences, validates against the schema. Used by `information_extraction`, `output_parser`, and `router` mode B. After this refactor, `information_extraction` still owns its texts-collection logic and calls into this helper.

- **`SubGraphNode`** is reused as-is for inline subgraphs. The router instantiates a `SubGraphNode` per branch that declares one (at init time), and calls its `execute` method when that branch is selected.

### 7.3 `when` DSL Evaluator

A pure-Rust evaluator over a parsed `WhenRule` enum:

```rust
enum WhenRule {
    Equals { field: String, value: Value },
    NotEquals { field: String, value: Value },
    In { field: String, values: Vec<Value> },
    Contains { field: String, value: Value },
    Gt { field: String, value: f64 },
    Lt { field: String, value: f64 },
    Gte { field: String, value: f64 },
    Lte { field: String, value: f64 },
    Matches { field: String, regex: Regex },
    Exists { field: String },
    All(Vec<WhenRule>),
    Any(Vec<WhenRule>),
    Not(Box<WhenRule>),
}

fn evaluate(rule: &WhenRule, extracted: &Value) -> bool { ... }
```

- `field` paths use a small dotted-path resolver. Missing intermediate keys → `false` (no panic, no error — except for `Exists` which returns its own logic).
- Regex compiled at init time, not per execution.
- Tested with table-driven unit tests covering each operator + nesting.

---

## 8. Testing Strategy

### 8.1 Unit Tests

**`output_parser.rs`:**
- Input string normal → JSON extracted matches schema.
- Input non-string (object/array) is serialized before being sent to the LLM.
- Input null / "" / [] / {} → explicit error.
- LLM returns markdown-fenced JSON → cleaned and parsed OK.
- LLM returns invalid JSON → error.
- LLM returns JSON failing schema validation → error.
- Schema with `required: true` is correctly translated to standard JSON Schema.
- `inline_schema_to_json_schema()` tested in isolation with typical and edge cases.

**`router/`:**

*Init validation:*
- Invalid mode, empty branches, duplicate names, invalid name regex.
- Mode A without `description` / with `when` → error.
- Mode B without `schema` / without `when` → error.
- Mode B `when.field` referencing a field not declared in schema → error.
- `subgraph` with both `child_graph_path` and `child_graph_inline` → error.

*Mode A (LLM-direct):*
- LLM returns valid branch → only that port non-null, others null.
- LLM returns branch outside enum → error.
- Input null/empty → error.
- `__decision` emitted with `selected_branch` and `reason`.

*Mode B (extract + rules):*
- Simple rules (`equals`, `in`, `gt`, etc.) — one test per operator.
- `all` / `any` / `not` combinators — including nested.
- Dotted paths (`user.profile.tier`).
- Order of evaluation: specific branch before general.
- No rule matches → error with `extracted` in the message.
- `__decision` includes `extracted`.

*Subgraph per branch:*
- Branch with subgraph → SubGraphNode runs, output emitted on the port.
- Subgraph suspends → SUSPENDED bubbles via the branch port.
- Subgraph fails → error with prefix `router branch '<name>':`.

*DSL evaluator (when_dsl.rs):*
- Table-driven test per operator with `(rule, extracted, expected_bool)` rows.
- Type-strictness checks (string "5" vs number 5).
- Regex compile failure at init.

**LLM mocking:** all unit tests use `MockAdapter` from `LlmRepository`. Zero real API calls in unit tests.

### 8.2 Integration Tests

New directory: `tests/graphs/control_flow/`.

| File | Description |
|---|---|
| `output_parser_basic.json` | `llm_call → output_parser → log` extracts intent + confidence |
| `router_llm_direct.json` | `input → router (mode A, 3 branches) → 3 distinct downstream logs` |
| `router_extract_rules.json` | `input → router (mode B, schema + when rules) → branches` |
| `router_with_subgraph.json` | Branch with inline subgraph running a mini-agent |
| `router_chained.json` | Two routers chained (output of one → input of the other) |

All integration tests gated with `#[ignore = "requires GEMINI_API_KEY — run with cargo test -- --ignored"]` per project convention.

### 8.3 Shipping Bar

- Unit test coverage at 100% of new code paths (DSL evaluator, inline-schema converter, both router modes, parser).
- At least 3 integration tests passing against a real provider (Gemini default).
- `cargo clippy` clean, `cargo fmt` applied, `cargo test --verbose` (including doctests) green.

---

## 9. Documentation Updates

| File | Change |
|---|---|
| `docs/node_configurations.json` | Add full entries for `output_parser` and `router` (config fields, ports, examples). |
| `docs/agent_context/node_ports_reference.md` | Add per-branch port semantics for `router`. |
| `docs/DEVELOPER_GUIDE.md` | Index entry pointing to the new section. |
| `docs/developer_guide/37_router_and_output_parser.md` | New guide: when to use each, examples for both modes, DSL reference, common patterns (e.g., extract-then-route chained with a downstream agent). |
| `docs/CHANGELOG_*.md` | Entry in the current rolling changelog. |

CLAUDE.md update: add a one-liner under the "Current Status" section once shipped, with the date and the spec link.

---

## 10. Backwards Compatibility & Migration

- Purely additive. No existing nodes changed.
- `information_extraction` is refactored to call `extract_with_schema(...)` internally but its public config, ports, and behavior are unchanged. A regression suite over `tests/graphs/agents/` covering `information_extraction` confirms no observable change.
- No ADP-side changes required: the new nodes are opt-in. Anything that doesn't use them keeps working.

---

## 11. Open Questions (resolved)

| Question | Decision |
|---|---|
| Parser as LLM wrapper vs deterministic? | LLM wrapper (refactor of `information_extraction`). |
| Router decision: LLM-direct, rules, or both? | Both modes in a single node, switched by `mode` config. |
| Default branch on no-match? | No default. Fail-fast with `extracted` in the error. |
| Rule grammar? | Declarative DSL (`equals`, `in`, `gt`, `all`, `any`, `not`, etc.). |
| Branch connection: ports vs internal subgraph dispatch? | Output ports per branch + **optional** inline subgraph per branch. |
| Required-fields declaration in schema? | Inline (`{ type, required, description }`) per field — consistent with `node_schema` in tool configurations. |
| Behavior on missing/empty input? | Hard error (no silent skip), unlike `llm_call`. |

---

## 12. Out of Scope (future work)

- Streaming the LLM extraction (would need event-stream integration).
- Multi-match (allow more than one branch to fire). Current design is XOR.
- Loop-aware routing (router that re-routes on `NEXT_TURN` with accumulated state). Achievable today by combining the router with `loop_controller`.
- Caching extracted results across runs (would benefit `llm_call`-style cache layer; orthogonal to this design).
- Pre-execution dry-run / "test routing" CLI command.

---

## 13. References

- Existing `information_extraction` node: [src/libs/colmena/src/dag_engine/infrastructure/nodes/extraction.rs](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/extraction.rs)
- Existing `SubGraphNode`: [src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs)
- Existing `loop_controller`: [src/libs/colmena/src/dag_engine/infrastructure/nodes/loop_controller.rs](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/loop_controller.rs)
- Node config schema reference: [docs/node_configurations.json](../../node_configurations.json)
- Tool-config inline-schema convention: [docs/node_as_tools_reference.json](../../node_as_tools_reference.json)
- Hexagonal architecture guide: [docs/developer_guide/01_architecture.md](../../developer_guide/01_architecture.md)
- Testing guide: [docs/developer_guide/05_testing.md](../../developer_guide/05_testing.md)
