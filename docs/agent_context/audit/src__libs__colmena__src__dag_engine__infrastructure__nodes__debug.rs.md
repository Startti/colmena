# src/libs/colmena/src/dag_engine/infrastructure/nodes/debug.rs

**Layer:** infrastructure  **Purpose:** Provides debugging and mock input nodes for DAG execution — LogNode prints inputs to console for inspection, MockInputNode emits its config as output data.

## Symbols

- `LogNode` (struct, pub) — A simple node that prints its inputs to console and passes them through for inspection
- `impl ExecutableNode for LogNode` (impl) — ExecutableNode trait implementation for LogNode
  - `execute` (async fn) — Logs inputs to console; flexibly accepts input/result/output keys or auto-flattens all inputs if no standard key found
  - `description` (fn) — Returns node description for debugging documentation
  - `default_input` (fn) — Returns default input field name "input"
  - `default_output` (fn) — Returns default output field name "output"
  - `schema` (fn) — Returns JSON schema metadata for the log node type
- `MockInputNode` (struct, pub) — Emits its config as the root output data; used for testing and mocking input sources
- `impl ExecutableNode for MockInputNode` (impl) — ExecutableNode trait implementation for MockInputNode
  - `execute` (async fn) — Returns the node's config unchanged as output
  - `default_output` (fn) — Returns None since output is raw config, not a named field
  - `schema` (fn) — Returns JSON schema metadata for the mock input node type

## File-level notes

- Both nodes have unused trait parameters (_config, _state, _observer, _inputs) — intentionally marked with underscore prefix per Rust convention
- LogNode's auto-flattening behavior (lines 27–34) handles flexible input routing; implicit contract documented only in code comment, not in schema
- MockInputNode carries "¡NO CAMBIAR!" (Do Not Change) comment on line 61–62 with no explanation of why it is critical or what depends on its exact behavior; this is a maintenance risk if assumptions are implicit
