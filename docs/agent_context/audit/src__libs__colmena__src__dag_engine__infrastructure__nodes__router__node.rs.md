# src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs

**Layer:** infrastructure  
**Purpose:** Implements the router node (ExecutableNode), which declaratively branches input to one of N branches using either LLM-direct selection or extract+rules logic. Handles environment variable resolution, input validation, and optional subgraph execution.

## Symbols

- `RouterNode` (struct, pub) — Container for a router node holding an Arc<OnceLock<Arc<dyn SubGraphExecutorPort>>> for lazy executor binding.
- `Default::default` (impl) — Default trait impl for RouterNode; delegates to new().
- `RouterNode::new()` (fn, pub) — Constructor that initializes a RouterNode with an empty OnceLock.
- `RouterNode::resolve_env_var` (fn, private) — Resolves `${VAR_NAME}` syntax to environment variable values or returns error if not found.
- `RouterNode::is_empty_input` (fn, private) — Helper to classify a JSON value as empty (Null, whitespace-only String, empty Array/Object).
- `ExecutableNode::execute` (async fn) — Main execution method: parses+validates config, reads input, picks branch via LLM-direct or extract-and-route mode, optionally executes subgraph and emits `__decision` metadata plus per-branch ports.
- `ExecutableNode::default_input` (fn) — Returns "input" as the canonical input port name.
- `ExecutableNode::description` (fn) — Returns human-readable description of router node modes and behavior.
- `ExecutableNode::schema` (fn) — Returns simplified JSON schema documenting config fields, inputs, and output ports.
- `tests::cfg()` (fn) — Test helper that constructs a minimal router config with two branches.
- `tests::inputs()` (fn) — Test helper that wraps a JSON value as the "input" key in NodeInputs.
- `tests::fails_when_input_is_null` (async fn) — Test verifying router rejects null input with "missing input" error.
- `tests::fails_when_input_is_empty_string` (async fn) — Test verifying router rejects whitespace-only input as empty.
- `tests::fails_on_invalid_config_at_runtime` (async fn) — Test verifying router rejects invalid mode ("weird") at runtime via parse_and_validate.
- `tests::extract_and_route_requires_schema_at_runtime` (async fn) — Test verifying extract_and_route mode fails if schema is missing in config.
- `tests::rejects_subgraph_with_both_path_and_inline` (async fn) — Test verifying router rejects subgraph configs that declare both child_graph_path and child_graph_inline.

## File-level notes

- The `_state` parameter in execute() is intentionally unused (Rust convention for ExecutableNode implementations that do not modify state).
- Input validation (`is_empty_input`) happens early (lines 68–70) before routing logic, preventing silent null routing.
- Environment variable resolution supports the `${VAR_NAME}` syntax in api_key config (lines 32–40), enabling credential injection via env.
- Subgraph execution (lines 131–158) forwards standard Colmena wiring keys (`__colmena_session_id`, `__colmena_agent_session_id`, `__colmena_node_id_path`, `__colmena_resume_answer`) and propagates extracted values to the child graph, enabling stateful HITL resume across branches.
- Output structure: always emits `__decision` object (selected_branch, reason, extracted metadata), plus one output port per branch (null for non-selected, payload for selected). This allows downstream nodes to subscribe to specific branches or observe the routing decision separately.
- Error handling distinguishes config validation errors (parse_and_validate, re-runs per execute) from runtime errors (missing input, invalid provider, LLM/extraction failures), all wrapped in Box<dyn Error + Send + Sync> for async trait compatibility.
- Test suite covers 5 critical edge cases: null/empty input, invalid config, missing schema for extract_and_route, and subgraph validation. No mocking of LLM/extract backends — those are tested separately in `llm_direct` and `extract_and_route` modules.
