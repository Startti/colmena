# src/libs/colmena/src/dag_engine/infrastructure/nodes/trigger.rs

**Layer:** infrastructure  **Purpose:** Implements TriggerWebhookNode, an ExecutableNode serving as an entry point for external webhook events that extracts and passes through request payloads.

## Symbols

- `TriggerWebhookNode` (struct, pub) — Unit struct implementing ExecutableNode trait for webhook trigger entry points
- `ExecutableNode::execute` (async fn) — Executes trigger by extracting payload from config (__payload__, test_payload, or inputs) and returning it as output
- `ExecutableNode::description` (fn) — Returns description identifying trigger as webhook entry point for external events
- `ExecutableNode::default_output` (fn) — Returns "output" as the default output field name
- `ExecutableNode::schema` (fn) — Returns JSON schema describing trigger webhook configuration and outputs

## File-level notes

- **Stale comment (line 20):** Comment states "Changed `StdError` to `Error`" but the actual return type is `Result<Value, Box<dyn StdError + Send + Sync>>`. Either the change was not completed or the comment is outdated/misleading. [FLAG: improvement — comment does not match code]
- **Non-standard schema format (lines 50–62):** The `schema()` method returns a simplified format where type descriptions are bare string literals (`"path": "string"`, `"test_payload": "any (optional...)"`) rather than structured JSON Schema objects with `type` and `description` fields. Documentation is embedded in string values instead of using proper schema structure. This deviates from standard JSON Schema conventions. [FLAG: improvement — inconsistent with schema best practices]
- **Unused parameters:** `_state` and `_observer` parameters are intentionally unused (prefixed with underscore) as part of the ExecutableNode trait signature; this is acceptable.
