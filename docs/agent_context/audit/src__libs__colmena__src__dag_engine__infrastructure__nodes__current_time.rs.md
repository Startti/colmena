# src/libs/colmena/src/dag_engine/infrastructure/nodes/current_time.rs

**Layer:** infrastructure  
**Purpose:** Provides a side-effect-free `ExecutableNode` that returns the current UTC timestamp as ISO-8601. Intended for use as an LLM tool when the model needs wall-clock time.

## Symbols

- `CurrentTimeNode` (struct, pub) — Zero-sized marker struct implementing `ExecutableNode` for returning current UTC time as ISO-8601
- `execute` (async fn, in impl ExecutableNode) — Fetches the current UTC timestamp via `Utc::now()` and returns it in the standard `{ "output": ... }` format
- `default_output` (fn, in impl ExecutableNode) — Returns the default output port name "output"
- `schema` (fn, in impl ExecutableNode) — Returns the node schema: empty inputs, one string output, node type "current_time"
- `description` (fn, in impl ExecutableNode) — Returns user-facing description: "Return the current UTC timestamp as an ISO-8601 string. Takes no parameters."
- `returns_iso8601_utc_timestamp` (test fn) — Unit test verifying the node returns a valid ISO-8601 UTC timestamp with timezone suffix

## File-level notes

- Minimal, focused implementation with no infrastructure dependencies beyond chrono and serde_json
- Implements all required methods of the `ExecutableNode` trait correctly
- No unused code, todos, or error handling gaps
- Test is adequate: validates ISO-8601 format and timezone presence
