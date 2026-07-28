# src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs

**Layer:** infrastructure  
**Purpose:** Implements the `for_each` executable node — runs an embedded target tool once per row of a list deterministically (iteration in Rust, not LLM re-calling). Supports both graph-node and LLM-tool usage, with incremental/final results-sheet writing and progress events.

## Symbols

- `MAX_CONCURRENCY` (const) — upper clamp (64) on concurrency to prevent unbounded concurrent target dispatches from misconfigured or LLM-supplied values
- `FORWARDED_CONTEXT_KEYS` (const) — array of 3 ambient context keys (`__colmena_subgraph_depth`, `__colmena_session_id`, `__colmena_agent_session_id`) forwarded from for_each's inputs into every per-row target dispatch to preserve recursion/session context across the boundary
- `ForEachNode` (struct, pub) — the main executable node struct holding a `registry: Arc<OnceLock<Arc<dyn NodeRegistryPort>>>` for lazy node lookup
- `ForEachNode::default()` (impl Default) — returns `Self::new()`
- `ForEachNode::new()` (pub fn) — constructor initializing registry with empty `OnceLock`
- `apply_column_selection()` (pub(crate) fn) — selects a single column from each row, renaming it to `as_name` (or the column name if absent), or passes rows unchanged if no column specified
- `cfg_or_input()` (fn, private) — reads a config field with fallback to the inputs map, honoring config-first precedence (used by both graph-node and tool-path execution modes)
- `value_type_name()` (fn, private) — maps a `serde_json::Value` to a short type name string for error messages
- `resolve_rows_async()` (async fn, private) — resolves the list of rows from `items` (inline array) → `items_from` (data-source handle, v1: "sheet") → default input edge, or errors if none provided
- `row_key()` (fn, private) — derives a stable per-row progress key: first scalar field as "key=value" or "index=N" if no scalars found
- `parse_policy()` (fn, private) — parses `ExecPolicy` (on_error, concurrency, max_items) from config/inputs, clamping concurrency to `[1, MAX_CONCURRENCY]`
- `ResultsSink` (struct, private) — parsed sink configuration for `results_to` output (title, mode: "final" or "incremental")
- `parse_results_to()` (fn, private) — parses `results_to` value into a `ResultsSink` or error message; v1 supports only `sink: "sheet"`
- `results_sheet_header()` (fn, private) — builds the header row for results sheet: `["index"] + <sorted input column names> + ["status", "result"]`, or `["input"]` for scalar rows
- `results_sheet_row()` (fn, private) — builds a single data row for results sheet matching `results_sheet_header` column order, extracting input cells per column and appending status/result
- `ForEachNode::execute()` (impl ExecutableNode, async fn) — main execution: validates target config, resolves rows, parses policy/results_to, creates results sheet if needed (once, before iteration), dispatches target to each row concurrently or serially per policy, collects progress/completion events, writes results incrementally (per-row inside dispatch) or finally (batch after), returns `{ output: { total, ok, err, results[], results_sheet?, results_sheet_error? } }`
- `ForEachNode::default_output()` (impl ExecutableNode, fn) — returns `Some("output")`
- `ForEachNode::schema()` (impl ExecutableNode, fn) — returns node schema JSON describing inputs (target, items, on_error, concurrency, max_items) and outputs
- `ForEachNode::description()` (impl ExecutableNode, fn) — returns user-facing description of the node's purpose and input modes
- `StubRegistry` (struct, tests) — test stub implementing `NodeRegistryPort` with selectable "add" and optional "echo" nodes
- `StubRegistry::get_node()` (impl NodeRegistryPort, fn) — returns registered test nodes by type
- `StubRegistry::get_all_nodes()` (impl NodeRegistryPort, fn) — returns empty hashmap (stub behavior)
- `EchoNode` (struct, tests) — minimal stub node that echoes back all received inputs as its output (used to observe per-row dispatch context)
- `EchoNode::execute()` (impl ExecutableNode, async fn) — returns `{ output: { all inputs as a map } }`
- `EchoNode::schema()` (impl ExecutableNode, fn) — returns empty schema JSON

## File-level notes

- **Dispatch closure**: The per-row dispatch (lines 400–502) is a large async closure capturing registry, target type/schema, observer, forwarded context, and incremental sink state. It merges the row into target schema, validates required params (including container-nested fields via `param_to_container` logic), executes the target node, detects HITL suspension as per-row error (fail-closed), optionally writes incremental results, and propagates batch-item-finished events. Properly structured for concurrent execution via `run_list`.

- **Results sheet dual-mode**: Incremental mode writes per-row ranges inside the dispatch closure (race-safe via distinct row addresses); final mode bulk-writes all rows after iteration completes. Both modes create a destination sheet exactly once before iteration (never touches input sheet). Failures are logged as warnings; write errors captured in output.

- **Ambient context forwarding**: `FORWARDED_CONTEXT_KEYS` preserves recursion depth and session context across the for_each boundary so subgraph targets don't reset `MAX_SUBGRAPH_TOOL_DEPTH` and conversational memory survives fan-out. `__node_id` is deliberately excluded to prevent collision with target's own path logic.

- **Config-first with inputs fallback**: The `cfg_or_input` pattern (mirroring suspend node) allows the same dispatch code to serve both graph-node execution (config static, inputs empty) and LLM-tool execution (config `{}`, all args in inputs).

- **Test coverage**: 19 tests covering column selection, row resolution, policy parsing, results sheet headers/rows, required-param validation (including container nesting), error paths (unknown sink/mode, non-array items, missing target), empty lists, concurrency clamping, and context forwarding. All pass.

- **No unfinished or dead code** — all functions are used; all dispatch paths are error-handled; no `todo!()` or `unimplemented!()` patterns.
