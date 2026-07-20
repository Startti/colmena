# Spec: Deterministic List Tool Execution — `for_each` node

> Status: SPEC / design contract — 2026-07-20. Not yet implemented.
> Brainstormed with owner (daniel@startti.co). MVP scope locked.
> Next artifact: integration plan in `docs/superpowers/plans/`.

---

## 1. Problem

"Apply this tool to N items" today depends entirely on the LLM's ReAct loop:

1. the model must **enumerate** all N items, and
2. emit **N tool calls** without stopping early or skipping any.

Both are non-deterministic. The ReAct loop even dispatches tools **strictly serially**
([`agent_service.rs:365`](../../../src/libs/colmena/src/llm/application/agent_service.rs)) —
there is no fan-out. Weak models (e.g. gemini-flash) narrate intent or go empty instead
of completing the list (the same failure that forced the gsheets single-round-trip
"Option A"). There is **no map / foreach / batch primitive anywhere** in the engine.

We want to move the *iteration* out of the model's hands: the model (or a CSV / XLSX /
Sheet / prior node) only produces the **list of configurations**; **code** executes the
target tool exactly N times and aggregates the results.

## 2. The shape: ONE node, used two ways

Following the canonical Colmena pattern (a single `ExecutableNode` is both a graph node
**and** an LLM tool via `tool_configurations` — exactly like `http_request`, `sql_query`,
`subgraph`), this is **one node: `for_each`**. No separate meta-tool.

- **As a graph node** — wired with edges; an upstream node feeds it a table; it runs its
  embedded target tool once per row and emits the results table to the next node. No LLM
  in the loop.
- **As an LLM tool** — declared in `tool_configurations`; the agent calls it once with the
  list (`items` / `items_from`); the node iterates. The target and policy are **fixed by
  the operator**, so the LLM only supplies the list — it doesn't even pick the tool.

Both paths share one iteration engine; nothing is duplicated.

## 3. Target = embedded (operator-configured)

The crux: a plain `ExecutableNode` only receives `(inputs, config, state, observer)` — it
does **not** see "the agent's other tools". So the target cannot be referenced by sibling
name in v1. Instead the target is **embedded in the node's config**, self-contained:

```jsonc
{
  "node_type": "for_each",
  "target": {                          // the tool to run per row (embedded)
    "node_type": "http_request",
    "node_schema": {
      "base_url": { "fixed": "https://api.example.com" },
      "method":   { "fixed": "PATCH" },
      "user_id":  { "required": true, "description": "..." },
      "plan":     { "required": true, "description": "..." }
    }
  },
  "on_error": "continue",              // continue | abort   (default continue)
  "concurrency": 5                     // 1 = sequential; >1 = bounded parallel (default 1)
}
```

**Core contract — a row = exactly the args the target tool would receive.** Per row the
node merges the row into `target.node_schema` using the **same merge logic** as
`DagToolExecutor::execute_inner` (extracted into a shared helper), then invokes the target
`ExecutableNode` via an injected registry/dispatch handle:

| Concept | Maps to | Existing mechanism |
|---|---|---|
| Static config (shared by all rows) | `fixed` fields of `target.node_schema` | `fixed_values` in `ParsedNodeSchema` |
| Per-row config | each row's keys, matched by **column name = param name** | `param_to_container` + per-row merge |
| Run the target | invoke target `ExecutableNode` with merged inputs | shared merge helper + injected registry handle |

`target` may also be a `child_graph` (subgraph) — then each row is injected as the child's
input(s) (`task` or structured), reusing `run_subgraph` with an ephemeral path per row.

## 4. Exposing `for_each` as a tool

Uses the standard node-as-tool mechanism — `target`/`on_error`/`concurrency` are `fixed`,
`items`/`items_from` are the LLM-visible params. The operator gives it a descriptive alias:

```jsonc
"tool_configurations": {
  "batch_update_users": {
    "node_type": "for_each",
    "node_schema": {
      "target":      { "fixed": { "node_type": "http_request", "node_schema": { ... } } },
      "on_error":    { "fixed": "continue" },
      "concurrency": { "fixed": 5 },
      "items":       { "required": false, "description": "Array of {user_id, plan} objects" },
      "items_from":  { "required": false, "description": "Handle to a table source" }
    }
  }
}
```

The LLM sees a tool `batch_update_users(items?, items_from?)` and calls it **once**; the
node runs `http_request` for every row. Fully deterministic in the iteration.

## 5. Where the list comes from

The node resolves its list in priority order: explicit `items` → `items_from` handle →
the default input edge (graph-node case).

- **`items` (inline)** — array of objects (or scalars, see column selection). Used when the
  list comes from the model's own reasoning or an upstream node's output.
- **`items_from` (handle)** — the deterministic path for data-sourced lists; the engine
  resolves the table so the model never re-types it. v1 sources: `attachment` (CSV/XLSX,
  reuse `data_run_python` tabular parsing) and `sheet` (reuse `dispatch_gsheets_read`).
  Optional **single-column selection**:
  ```jsonc
  "items_from": { "source": "sheet", "ref": "<id+range>",
                  "column": "user_id",   // optional: take ONLY this column
                  "as": "user_id" }      // optional: rename to the target param if the header differs
  ```
  - No `column` → each row is an object (multi-param, column name = param name).
  - With `column` → each item = `{ "<as|column>": <value> }` (one scalar per row) — covers
    "a list of IDs living in one column of a wide table".
- **Input edge** (graph-node only) — an upstream node's table lands on the default input.

Each row is validated against the target tool's schema **before** execution; an invalid
row becomes an `err` result (respecting `on_error`). Rows must be **homogeneous**.

## 6. Result & persistence

**Result** (compacted via `tool_digest.rs` for the LLM; full array retained internally).
The table is **assembled incrementally** — each item is streamed on completion (§7), so the
consumer accumulates rows live; the full table is returned/emitted at the end:
```jsonc
{
  "total": 2, "ok": 1, "err": 1,
  "results": [
    {"index": 0, "input": {"user_id":1,...}, "status": "ok",  "output": {...}},
    {"index": 1, "input": {"user_id":2,...}, "status": "err", "error": "..."}
  ]
}
```

**Persistence stance (v1):** the results table is the single source of truth for progress,
held **in-memory in the node** for the duration of the run (ephemeral `Vec<ItemResult>`);
the final table is the node's output → as a tool it flows into `llm_node_history` like any
tool result. Partial state is **not** in the DB — but because each item is emitted the
moment it finishes (§7), the SSE consumer already holds the partial table if the run dies
mid-way (client-side progress, for free). The node stays **pure** — it never writes back to
the user's source. Durable server-side checkpointing and a `results_to` write-sink are v1.1
(§10).

## 7. Execution semantics, streaming & progress

- **Memory: Mode B (isolated).** Each row is an independent invocation; for `child_graph`
  targets = `run_subgraph` with an ephemeral path per row (already stateless today). No
  history crosses rows → safe to parallelize.
- **Policy.** `on_error: continue` (collect per-item ok/err) or `abort` (fail-fast,
  short-circuit). `concurrency: N` → sequential (N=1) or `buffer_unordered(N)`; results
  reordered by index on aggregate.
- **HITL fail-closed.** If a row returns SUSPENDED, that row becomes `err` with a clear
  message. SUSPENDED never bubbles out of the batch (resumable fan-out = backlog).
- **Caps.** `max_items` guard; on truncation, `log()` what was dropped — never silent.
- **Streaming / progress (SSE).** Runs on the `observer` already threaded via
  `with_observer` (the `subgraph-*` channel + PR #146 per-frame `level`/`path`). Three
  granularities so the user knows where the run is **without parsing raw frames**:
  1. **`batch-progress`** (coarse — a bar): `{ total, completed, ok, err, in_flight }`,
     emitted at start, after each item, and at end.
  2. **`batch-item-finished`** (medium — a live checklist): `{ index, key, status }`,
     emitted the moment each item completes (also what lets the consumer accumulate the
     results table incrementally).
  3. **`batch-item[k]/...`** (fine — the child's full internal stream), collapsible in UI.
  > **Correction (verified against code):** `NodeEvent` (`domain/observer.rs`) and
  > `DagExecutionEvent` (`events.rs`) are **closed enums** — there is NO open/string custom
  > event path. `subgraph-*` is not a custom type; it reuses the single
  > `NodeEvent::SubgraphChildEvent(Value)` variant wrapping a `DagExecutionEvent`, and the
  > `subgraph-` prefix is added by the SSE mapper (`events.rs`, `SubgraphWrapped`).
  > So `batch-progress` / `batch-item-finished` need **additive** work in three places:
  > (1) new `DagExecutionEvent` variants (`events.rs`), (2) the SSE mapper handling them,
  > and either (3a) new `NodeEvent` variants or (3b) emit them wrapped in the existing
  > `SubgraphChildEvent(Value)` (no `NodeEvent` change — the lighter path). This is a small
  > but real domain touch, not free. Graph-node usage emits the same events. ADP frontend
  > renders the new SSE types (ADP-side follow-up).

## 8. Architecture

- **Node** `for_each` (`dag_engine/infrastructure/nodes/for_each.rs`) — implements
  `ExecutableNode`; registered in `registry.rs`; exposed as a tool via the standard
  node-as-tool path (no synthetic meta-tool). Holds an injected registry/dispatch handle
  (OnceLock port, mirroring `subgraph`'s `SubGraphExecutorPort` injection) so it can run
  its embedded target.
- **Iteration engine** `ListToolExecutor` (`dag_engine/application/`) — `(rows, dispatch_fn,
  policy, observer) -> Vec<ItemResult>`. Owns iteration, concurrency, policy, ordering,
  event emission. Backing-agnostic (takes a `dispatch_fn` closure). Kept separate from the
  node purely for unit-testability.
- **Shared merge helper** — extract `execute_inner`'s per-call merge
  (`parse_node_schema` → `fixed_values` + `param_to_container` → merged inputs) into a
  reusable function callable by both `execute_inner` and `for_each`'s `dispatch_fn`.
- **Injection** — `for_each` holds `Arc<OnceLock<Arc<dyn NodeRegistryPort>>>` (or a small
  dispatch port), set after construction, mirroring `set_subgraph_executor`
  (`registry.rs:361`, injected at `engine.rs:299`). Verified feasible: `DagToolExecutor`
  already carries the registry (`llm.rs:2294`) and `get_node(node_type).execute(...)` is
  callable at runtime (`dag_tool_executor.rs:765`).
- **New traits/ports**: none new conceptually — reuses `NodeRegistryPort`,
  `SubGraphExecutorPort`, `ExecutionObserver`; adds an injected handle to `for_each`.
- **Event enums (additive domain touch)**: add `batch-progress` / `batch-item-finished` as
  new `DagExecutionEvent` variants + SSE-mapper handling (`events.rs`), emitted either via
  new `NodeEvent` variants or wrapped in the existing `NodeEvent::SubgraphChildEvent(Value)`.
  Closed enums → this is required, not optional (see §7 correction).
- **Binding impact**: none (new node_type only). ADP unaffected (additive; frontend adds
  rendering for the new SSE event types as a separate ADP follow-up).

## 9. Edge cases & risks

| Risk / edge case | Handling |
|---|---|
| Model enumerates a huge inline `items` (partial determinism) | Promote `items_from` in the tool description as the deterministic path for data lists. |
| Suspend/HITL inside a row during fan-out | v1 fail-closed: SUSPENDED row → `err`. |
| Row keys don't match target params | Validate per row vs `target.node_schema`; invalid → `err` per policy. |
| Heterogeneous rows | Contract = homogeneous; validate + fail early with diagnostics. |
| Rate limits / cost on large fan-out | `concurrency` cap + `max_items` guard + `log()`. |
| Recursion (target is a `for_each` / subgraph) | Reuse `MAX_SUBGRAPH_TOOL_DEPTH`; forbid `for_each` as its own target. |
| Empty list | `total: 0`, empty `results` — no error. |
| Tool-def generation for a complex `fixed` `target` object | `node_schema` `fixed` already holds arbitrary JSON; `items`/`items_from` are the only visible params. Verify generation handles an array-typed visible param. |

## 10. Open questions

- **Resolved**: `node_type = "for_each"`. The tool name is an operator-chosen alias per
  `tool_configurations` instance (e.g. `batch_update_users`), not fixed by the node.
- **Non-blocking**: default `max_items` / `concurrency` values.
- **Deferred (v1.1) — `target_tool` by-name**: let the node reference one of the agent's
  already-owned tools by name (instead of an embedded target), resolved via a sibling-tool
  dispatch handle the `DagToolExecutor` injects only in the tool context. Restores "batch a
  tool you already have" + lets the LLM pick the target. Needs the injection plumbing.
- **Deferred (v1.1) — `results_to` sink**: opt-in dump of the results table to a **new**
  Sheet (never mutate the source → safe). Thin sink **on top of** the node (reuses gsheets
  `SheetsClient` create+write / the `data_run_python` path), NOT in the core. Modes `final`
  (one `batchUpdate` at end, default) + `incremental` (append per row — "watch the sheet
  fill"; append + per-row `index` → no races under concurrency). Generalizable to `sql`.
- **Deferred (v1.1) — `items_from: tool_result`**: needs a per-turn `tool_call_id -> output`
  cache in `DagToolExecutor`.
- **Deferred (v1.1) — durable checkpoint store** keyed by `(batch_id + item_key)` for crash
  recovery / resume-skipping-done (§6 persistence stance).
- **Deferred (backlog) — Mode A** (shared memory across rows, sequential-only, agent-only).

## 11. Validation (acceptance)

E2E against **both** target kinds (locked), each exercised as a graph node AND as a tool:
- a simple node target (`http_request`/`sql_query`/`python_script`) over inline IDs;
- a `child_graph`/subgraph target (isolated per row);
- `items_from: attachment` (CSV) and `items_from: sheet`, incl. single-column selection;
- `on_error: continue` vs `abort`; `concurrency: N` throughput + result ordering;
- progress events (`batch-progress`, `batch-item-finished`) observed on the stream.

Real nodes only (no `log` placeholders), per CLAUDE.md. Run with `--agent-session-id`,
save SSE to `/tmp/colmena_e2e/`, verify frames and digest correctness.
