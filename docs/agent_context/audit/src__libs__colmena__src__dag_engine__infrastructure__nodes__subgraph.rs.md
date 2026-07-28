# src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs

**Layer:** infrastructure  **Purpose:** Implements subgraph node that executes child DAG graphs as isolated runs or as LLM tools, with resume propagation, recursion depth limiting, and streaming boundary events for Fase F UI tree boundaries.

## Symbols

- `SubGraphNode` (struct, pub) — Container for a SubGraphExecutorPort dependency injected via OnceLock
- `impl Default for SubGraphNode` — Delegating default implementation
- `SubGraphNode::new()` (fn, pub) — Creates a new SubGraphNode with uninitialized executor
- `SubGraphNode::MAX_SUBGRAPH_TOOL_DEPTH` (const, pub) — Recursion guard limit set to 5 for subgraph-as-tool nesting
- `SubGraphNode::resolve_child_graph_source()` (fn, private) — Resolves child graph from config (edge-based) or inputs (tool-based) with config precedence; returns inline object or path string
- `SubGraphNode::current_depth()` (fn, private) — Reads `__colmena_subgraph_depth` from inputs; defaults to 0 if absent
- `SubGraphNode::depth_exceeded()` (fn, private) — Returns true if current depth >= MAX_SUBGRAPH_TOOL_DEPTH
- `impl ExecutableNode for SubGraphNode` — Trait implementation for DAG execution engine
- `ExecutableNode::schema()` (fn) — Returns default schema exposing "task" string input for LLM tool exposure
- `ExecutableNode::execute()` (fn, async) — Main execution logic: handles resume answer propagation, graph loading (inline or file), HITL suspend bubble-up, child state mapping (in/out), and boundary event emission for Fase F
- `subgraph_tool_input_config_tests` (mod, cfg(test)) — Unit tests for config/input precedence and graph source resolution
- `resolve_graph_source()` (fn, test) — Test helper wrapping resolve_child_graph_source
- `subgraph_schema_tests` (mod, cfg(test)) — Verifies schema exposes task input for tool builder
- `subgraph_depth_guard_tests` (mod, cfg(test)) — Five tests covering MAX constant, depth boundary conditions, and execute-time rejection
- `subgraph_as_tool_boundary_tests` (mod, cfg(test)) — Fase F: verifies boundary node-start/end events emitted for plain tools using __node_id fallback
- `CapturingObserver` (struct, test) — Mock ExecutionObserver storing events in a Mutex for test inspection
- `StubExecutor` (struct, test) — Minimal SubGraphExecutorPort stub for testing execute logic without full orchestration
- `inner_of()` (fn, test) — Deserializes SubgraphChildEvent wrapper into DagExecutionEvent for assertion
- `subgraph_suspend_passthrough_tests` (mod, cfg(test)) — Verifies SUSPENDED results returned verbatim with questions preserved
- `passes_through_suspended()` (fn, test) — Validates invariant that child SUSPENDED result is passed through unchanged

## File-level notes

- **Parameter naming inconsistency (line 88)**: `_observer` parameter is prefixed with underscore despite being used throughout the method (lines 155, 233, 241, 262, 292, 298). Should be `observer` without the underscore to match actual usage.
- **Redundant conditional (lines 162–166)**: After resume, both the `if SUSPENDED` branch and the fallthrough return the identical `Ok(result)` value. The condition is dead code; should simplify to a single `return Ok(result)`.
- **State filtering logic (lines 214–229)**: Child state map excludes `__colmena_*` keys via loop, then inserts the incremented `__colmena_subgraph_depth` afterward. Well-commented but slightly unintuitive; the post-loop insertion is necessary to survive the filtration.
- **Best-effort event emission (lines 233–243, 292–300)**: Boundary events silently ignore serialization failures (`if let Ok(raw) = ...`). Intentional best-effort design; could benefit from explicit doc comment at call site.
- **Graph source resolution**: Cleanly separates config-path (edge-based DAG node usage) vs. inputs-path (LLM tool usage via executor's fixed_config merge), with explicit config precedence.
- **Resume propagation**: Correctly traces child suspension across executor via `parent_session_id` + `parent_path` keys, enabling HITL inside nested subgraph tools.
- **Comprehensive test coverage**: Four test modules (config resolution, schema, depth guard, boundary events, suspend passthrough) cover all major code paths and invariants; 9 total test functions.
