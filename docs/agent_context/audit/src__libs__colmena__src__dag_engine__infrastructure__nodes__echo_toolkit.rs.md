# src/libs/colmena/src/dag_engine/infrastructure/nodes/echo_toolkit.rs

**Layer:** infrastructure  
**Purpose:** Internal test toolkit node with two sub-tools (echo and double) for unit testing the DAG engine's `ExecutableNode` and `ToolkitNode` trait contracts. Not registered in default registry; construct directly in tests or register manually.

## Symbols

- `EchoToolkitNode` (struct, pub) — Zero-sized marker struct implementing both ExecutableNode and ToolkitNode traits for testing.
- `impl ExecutableNode for EchoToolkitNode` — Trait implementation providing async execution, schema, and description methods.
  - `execute` (async fn, pub) — Dispatches to "echo" (returns input string) or "double" (returns number × 2) sub-tools; errors on missing `__sub_tool` or required fields.
  - `schema` (fn, pub) — Returns JSON schema stub with empty inputs and single "any" output.
  - `description` (fn, pub) — Returns "Echo toolkit stub — internal test use only."
- `impl ToolkitNode for EchoToolkitNode` — Trait implementation defining the sub-tool catalog.
  - `sub_tool_catalog` (fn, pub) — Returns vec of two `SubToolDefinition`s: "echo" (string → string) and "double" (number → number).
- `tests` (module, cfg(test)) — Test module with 6 unit tests covering sub-tool dispatch, catalog structure, and error cases.
  - `dispatches_on_sub_tool_echo` (async test fn) — Verifies "echo" sub-tool echoes input string.
  - `dispatches_on_sub_tool_double` (async test fn) — Verifies "double" sub-tool returns 2× input number.
  - `catalog_has_two_entries` (async test fn) — Verifies catalog contains exactly 2 entries (echo, double).
  - `missing_sub_tool_returns_error` (async test fn) — Verifies execute errors when `__sub_tool` is missing.
  - `unknown_sub_tool_returns_error` (async test fn) — Verifies execute errors for unrecognized sub-tool name.
  - `echo_missing_message_returns_error` (async test fn) — Verifies "echo" sub-tool errors on missing "message" input.
  - `double_missing_n_returns_error` (async test fn) — Verifies "double" sub-tool errors on missing "n" input.

## File-level notes

- Clear module-level documentation explicitly marks this as internal test-only and notes it is not in the default registry.
- All error paths use simple string errors (`format!(...).into()`), appropriate for a test fixture.
- The `schema()` method returns a minimalist stub schema (`{ "inputs": {}, "outputs": { "output": "any" } }`) — not production-grade but sufficient for testing the trait contract.
- Sub-tools are hardcoded; no configuration ingestion from `_config` parameter.
- Test coverage is comprehensive (dispatch paths, error cases, catalog structure).
