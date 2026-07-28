# src/libs/colmena/src/dag_engine/infrastructure/nodes/input.rs

**Layer:** infrastructure  **Purpose:** Implements the Input node, an ExecutableNode that outputs static configuration data with template resolution support, accepting upstream injection overrides.

## Symbols

- `InputNode` (struct, pub) — marker struct implementing ExecutableNode for static input data emission
- `resolve_templates` (fn, private) — recursively resolves `{{key}}` and `{{key.nested}}` templates in JSON values via flat state lookup, traversing Objects and Arrays
- `ExecutableNode impl for InputNode` (impl) — trait implementation providing node lifecycle
- `execute` (async fn, pub) — resolves config data or passes through injected inputs, substituting `{{}}` templates and allowing injected values to override declared keys
- `description` (fn, pub) — returns descriptive text for the Input node
- `default_output` (fn, pub) — returns "output" as the default port name
- `schema` (fn, pub) — returns JSON schema defining the node type, config shape, and outputs

## File-level notes

- Clean, focused implementation with no dead code or unfinished stubs.
- Template resolution (lines 10–46) handles string, object, and array recursion; no escaping mechanism for literal `{{}}` sequences.
- Minor doc comment typo on line 50: "outpus" should be "outputs" — cosmetic, no impact on functionality.
- State injection via `__payload__` (line 60) short-circuits template resolution; passthrough mode (line 74–82) filters out internal keys (`__*` and `session_id`) before returning injected inputs.
- Injected inputs selectively override declared config keys, preserving nulls and empty values as "no override" signals (lines 86–97).
