# src/libs/colmena/src/dag_engine/infrastructure/nodes/output.rs

**Layer:** infrastructure  
**Purpose:** Implements OutputNode, a terminal DAG node that echoes its input as the graph result, wrapping it in a structured envelope with metadata.

## Symbols

- `OutputNode` (struct, pub) — Marker struct implementing ExecutableNode for graph terminal outputs
- `impl ExecutableNode for OutputNode` (impl, pub) — Trait implementation for the output node executor
- `execute` (async fn, pub) — Accepts input value, wraps it in `{"result": <input>, "extra_info": {"__colmena_is_output_node": true}}`
- `default_input` (fn, pub) — Declares default input port as "input"
- `default_output` (fn, pub) — Declares default output port as "result"
- `schema` (fn, pub) — Returns empty JSON schema (node accepts no configuration fields)

## File-level notes

- No configuration schema — this node is stateless and accepts no behavioral parameters
- Metadata flag `__colmena_is_output_node` marks outputs distinctly for downstream consumers
- All trait methods are minimal implementations with no optional behavior
- Code is straightforward with no error paths beyond Result wrapper
