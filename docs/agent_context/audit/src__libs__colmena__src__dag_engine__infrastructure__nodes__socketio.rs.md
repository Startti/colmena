# socketio.rs

**Layer:** infrastructure  
**Purpose:** Socket.IO request node implementation. Emits events to Socket.IO servers with support for acknowledgment callbacks, wait-event response patterns, pre-event sequences on the same connection, and transport-level error capture for LLM guidance.

## Symbols

### Types & Constants
- `WaitSlots` (type alias) — `Arc<Mutex<HashMap>>` routing wait_event names to their active oneshot channel senders
- `MAX_TRANSPORT_ERRORS` (const, usize = 10) — cap on raw transport error messages collected per execution
- `TRANSPORT_ERROR_ADVICE` (const, &str) — LLM-facing guidance string for failures with transport errors
- `PreEventSpec` (struct, private) — one entry in `pre_events` array with event name, payload, optional wait_event, optional timeout override

### Main Node & ExecutableNode Implementation
- `SocketIoNode` (struct, pub, unit) — stateless node implementing `ExecutableNode` for Socket.IO connections

### Private Helper Methods (on SocketIoNode impl)
- `resolve_env_vars` (fn, private) — resolve `${ENV_VAR}` placeholders in strings; identical to HttpNode's resolver
- `resolve_env_vars_in_value` (fn, private) — recursively resolve env vars in all string values within a JSON Value (objects, arrays, scalars)
- `payload_to_value` (fn, private) — convert `rust_socketio::Payload` enum to `serde_json::Value` (Text → single or array, Binary → base64 wrapper)
- `payload_to_compact_string` (fn, private) — render Payload as single-line string for logs (unwraps single strings, JSON-serializes objects)
- `summarize_transport_errors` (fn, private) — collapse duplicate error messages preserving first-seen order (e.g. `["E", "E", "F", "E"]` → `["E (x3)", "F"]`)
- `attach_transport_context` (fn, private) — inject `transport_errors` + LLM `advice` fields into failure envelope; no-op if error list empty
- `get_str` (fn, private) — read string field from inputs (priority) or config; None if missing or non-string
- `get_u64` (fn, private) — read u64 field from inputs (priority) or config; None if missing or non-integer
- `parse_pre_events` (fn, private) — parse `pre_events` value into `Vec<PreEventSpec>`; accepts null/absent/empty array as OK, validates non-empty event strings per array item
- `emit_step` (async fn, private) — emit one Socket.IO event (pre or main) over existing connection; races ack callback vs. wait_event listener vs. exception channel vs. timeout; returns parsed response Value

### ExecutableNode Trait Methods (pub)
- `execute` (async fn) — main execution: resolve config + inputs (inputs > config), parse pre_events, build Socket.IO client with transport type/headers/cookies/handlers, emit pre_events sequentially, emit main event, disconnect cleanly, build output envelope with success/error/pre_responses/transport_errors
- `description` (fn) — returns node description string for registry
- `default_output` (fn) — returns default output port name ("response")
- `schema` (fn) — returns JSON schema documenting all config fields, input overrides, and output envelope structure

### Tests (cfg(test) module)
- `parse_pre_events_*` — 8 tests covering parse_pre_events: none, null, empty array, valid minimal/full, missing/empty event, non-array, non-object item
- `resolve_env_vars_in_value_recursive` — tests recursive ${VAR} replacement in nested objects/arrays
- `summarize_transport_errors_*` — 3 tests covering empty, single, and aggregation with order preservation
- `attach_transport_context_*` — 2 tests covering no-op (empty errors) and population of transport_errors/advice fields
- `payload_to_compact_string_*` — 2 tests covering single string unwrap and object serialization

## File-level notes
- **Architecture compliance**: Pure infrastructure layer; all external I/O (Socket.IO client, payload handling) confined here; no domain dependencies
- **Error handling strategy**: Boxed StdError at trait boundary; Result<String, String> for internal helpers
- **Logging**: Uses println! with emoji prefixes and structured formatting for debug/dev visibility; no tracing/log framework yet (consistent with alpha phase)
- **Configuration layering**: Fully implements inputs > config priority pattern; supports ${ENV_VAR} resolution in all string values
- **Transport-level resilience**: Captures transport errors in a bounded vector during execution window; gates handlers on execution-active flag to silence zombie background tasks after disconnect; deduplicates and summarizes errors for LLM inclusion on failure
- **Pre-events implementation**: Runs optional event sequence on same connection before main event; early abort on pre-event failure with structured pre_responses array; supports per-event wait_event and timeout overrides
- **Test coverage**: Unit tests for all helper functions (parsing, resolution, error handling, payload conversion); no end-to-end execute() test (integration testing via test graphs)
