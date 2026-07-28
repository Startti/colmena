# src/libs/colmena/src/dag_engine/api.rs

**Layer:** infrastructure  **Purpose:** HTTP API and streaming interfaces to the DAG engine; provides entry points for running, streaming, and serving DAGs via file paths, JSON strings, and webhook triggers with suspend/resume support.

## Symbols

### Public Functions
- `run_dag` (fn, pub) — Loads a DAG from file and executes it, returning final output as JSON value or error.
- `run_dag_from_str` (fn, pub async) — Parses a DAG from in-memory JSON string, validates, optionally injects payload into trigger nodes, executes (streaming or non-streaming), then shuts down engine; backs Python dict entry point.
- `stream_dag` (fn, pub async) — Loads a DAG from file and returns an owned stream of SSE-mapped execution events.
- `stream_dag_from_str` (fn, pub async) — Parses a DAG from in-memory JSON, validates, optionally injects payload, returns owned stream of SSE-mapped parts; engine Arc-wrapped and held until stream EOF.
- `serve_dag` (fn, pub async) — Registers trigger_webhook node paths as POST routes, binds a TCP listener, starts an Axum server with graceful Ctrl-C shutdown.

### Private Structs
- `AppState` (struct, private) — Shared state across Axum handlers: Arc<Graph> and Arc<ColmenaEngine>.
- `ResumePayload` (struct, private) — Deserialized JSON body for the /resume endpoint: session_id (String), answer (String), agent_session_id (Option<String>).

### Private Functions (HTTP Handlers)
- `handler_webhook` (fn, private async) — Receives POST requests at trigger routes; supports two modes: SSE streaming (with optional loop-restart on non-output, non-finished nodes) or standard JSON response (with loop restart via output injection); resolves agent_session_id from header or body.
- `handler_resume` (fn, private async) — Resumes a suspended DAG with human input (session_id + answer); supports SSE or JSON response modes; resolves agent_session_id from header or body.

### Private Closures (inline helpers in handlers)
- `find_status` (closure, inside `handler_webhook` SSE branch, line 449) — Recursively searches JSON object for a string-valued key; checks root then 1 level deep in child objects.
- `find_bool` (closure, inside `handler_webhook` SSE branch, line 463) — Recursively searches JSON object for a bool-valued key; checks root then 1 level deep.
- `find_field` (closure, inside `handler_webhook` JSON branch, line 551) — Recursively searches JSON object for any value at a key, with fallback paths through "output", "extra_info", "result" nested structures; more complex than find_status/find_bool.

## File-level notes

### Duplication — Payload Injection
The pattern of injecting a payload into `trigger_webhook`/`input`/`mock_input` node configs is repeated three times:
1. Lines 61–73 in `run_dag_from_str`
2. Lines 209–220 in `stream_dag_from_str`
3. Lines 374–381 in `handler_webhook`

Each checks `node.node_type`, initializes `node.config` if null, then sets `node.config["__payload__"] = payload.clone()`. Candidate for extraction to a helper function.

### Duplication — Loop-Control Logic
The `handler_webhook` function contains nearly identical loop-exit detection logic in two branches (SSE vs. JSON):
1. Lines 447–490 (SSE path): Uses `find_status`/`find_bool` closures to detect `__colmena_status == "SUSPENDED"`, `__colmena_loop_status == "FINISHED"`, or `__colmena_is_output_node == true`.
2. Lines 550–645 (JSON path): Uses `find_field` closure with identical checks plus additional output-node result extraction.

Both branches handle the same exit conditions but with different closure implementations. Candidate for extraction to a shared helper enum/struct and function.

### Duplication — Agent Session ID Resolution
The pattern of resolving `agent_session_id` from headers then body fallback is repeated:
1. Lines 344–355 in `handler_webhook`
2. Lines 697–702 in `handler_resume`

Both extract from `x-agent-session-id` header, fall back to body field. Candidate for helper function.

### Complexity — Nested JSON Search Closures
The closures `find_status`, `find_bool`, and `find_field` all perform recursive/nested JSON searches with multiple levels of `.and_then()` and for-loops. While not incorrect, the nesting (2–3 levels) makes the logic harder to follow and test independently. The `find_field` closure in particular (lines 551–597) searches multiple fallback paths (root, child objects, "output" key, "extra_info" key, "result" key).

### Stream Ownership Model — Implicit Drop
The `stream_dag` and `stream_dag_from_str` functions return a stream that holds Arc<ColmenaEngine>. The docstring states "The returned stream owns the engine and shuts it down when the graph finishes," but shutdown occurs implicitly via Arc's drop when the stream ends. No explicit cleanup callback or resource-guard is present. This is safe but relies on caller understanding the Arc lifetime.

### Payload Cloning
Payload values are cloned multiple times during injection (lines 70, 218, 379, 498, 661) before being set into node config. For large JSON payloads, this could accumulate memory overhead. An Arc<Value> approach could reduce copies.

### Known Limitation — Generic Trigger Handling
The comment at line 371 notes: "To solve the closure context issue, we'll iterate and inject to all trigger_webhooks in the graph. The previous code injected to 'trigger_node_id' passed as closure. However, axum handler here is generic."

This indicates the current approach (iterate all trigger_webhook nodes) is a workaround for limited Axum closure context. If a graph has multiple trigger_webhooks, all receive the same injected payload, which may not be the intended behavior. No guard or validation prevents this scenario.

### Error Handling Boundary
When parsing and validating the graph (lines 55–58, 203–206, 251–254), errors are wrapped in Box<dyn std::error::Error + Send + Sync> with a format string prefix "Invalid graph: {}". This provides minimal context about which field or node caused the failure. Improved error messages could aid debugging.
