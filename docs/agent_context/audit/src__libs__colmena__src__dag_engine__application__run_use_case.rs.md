# src/libs/colmena/src/dag_engine/application/run_use_case.rs

**Layer:** application  
**Purpose:** Main orchestration use case for DAG graph execution, handling node lifecycle, state persistence, resume/suspend, liveness monitoring, and streaming event delivery. Bridges NodeRegistry (domain ports) to concrete execution via async streams.

## Symbols

- `DagRunUseCase` (struct, pub) — Main orchestrator holding registry, state repository, secure value service, and liveness settings
- `DagRunUseCase::new()` (fn, pub) — Constructor with registry and optional state repository
- `DagRunUseCase::with_secure_values_and_service()` (fn, pub) — Constructor injecting pre-built SecureValueService (official injection point per hex architecture)
- `DagRunUseCase::with_liveness()` (fn, pub) — Builder to override heartbeat and idle-timeout watchdog knobs
- `DagRunUseCase::check_limits()` (fn, private) — Static validator checking node global and caller-specific call limits before execution
- `DagRunUseCase::execute()` (fn, async, pub) — **[FLAG: unfinished — unimplemented!() body, deprecated in favor of execute_stream()]**
- `DagRunUseCase::strip_extra_info()` (fn, pub) — Recursively removes extra_info from JSON output tree, preserving colmena control markers
- `DagRunUseCase::execute_stream()` (fn, pub) — Main async streaming executor: session/agent/resume lifecycle, queue-based DAG traversal, node input building, secret injection, execution with observer, liveness heartbeat+idle-abort watchdog, cancellation handling, state persistence, subgraph event relay and nesting, usage telemetry
- `DagRunUseCase::compute_resuming_node_ids()` (fn, private) — Computes set of node IDs with `__colmena_status: "SUSPENDED"` from persisted outputs (guards against injecting resume_answer into non-resuming nodes)
- `DagRunUseCase::find_status_by_key()` (fn, private) — Recursive JSON search for a key and returns its string value (used to detect SUSPENDED markers)
- `DagRunUseCase::build_inputs_for()` (fn, private) — Constructs NodeInputs by resolving edges, source node outputs, default_output/default_input fields, and auto-flattening objects
- `ChannelObserver` (struct, private) — Simple observer forwarding events via tokio mpsc channel
- `ChannelObserver::on_event()` (impl ExecutionObserver, private) — Sends observer events to unbounded channel
- `node_event_advances_heartbeat()` (fn, private) — Determines if a NodeEvent should reset the heartbeat liveness clock (excludes pure accounting events like LlmUsage)
- `SubGraphExecutorPort::run_subgraph()` (async, impl) — Executes a child graph with isolated session, forwarding observer events to parent
- `SubGraphExecutorPort::resume_subgraph()` (async, impl) — Resumes a persisted child graph with answer, reconstructing state from DB
- `SubGraphExecutorPort::find_child_session_id_for_resume()` (async, impl) — Queries state repository for suspended child under a parent session
- Test module `resuming_node_ids_tests` (module, #[cfg(test)]) — Unit tests for compute_resuming_node_ids edge cases (empty, suspended-only filtering, nested suspensions, empty outputs)

## File-level notes

- **execute() unimplemented:** Intentional deprecation. Comment directs callers to execute_stream() and draining-wrapper pattern. Function body is `unimplemented!()` — should remain as-is to fail fast if called, but it is not a bug/TODO.
- **execute_stream() complexity:** Single 847-line function (lines 153–999). Handles session lifecycle (Branch 1/2/3 resume routing), queue-based DAG traversal, input building with edge resolution, secret injection (non-LLM only), suspension detection, hard cancellation (mid-node + between-nodes), two-clock liveness (heartbeat + idle watchdog), state persistence (SUSPENDED/CANCELLED/COMPLETED/FAILED), subgraph event relay with depth/path nesting, and usage telemetry. Necessarily complex due to coupled concerns; decomposition would require significant refactoring of state threading. No duplication detected.
- **Secret injection skipped for LLM nodes (line 419):** Intentional — LLM nodes handle secrets via a different path (described in secure_values_service docs).
- **All private helpers actively used:** check_limits, compute_resuming_node_ids, find_status_by_key, build_inputs_for all called within execute_stream or SubGraphExecutorPort methods.
- **Observer pattern:** ChannelObserver is lightweight forwarder; real filtering/processing happens downstream via DagExecutionEvent mapper and SubgraphChildEvent relay.
- **Spec cross-references:** Code heavily references specs in docs/superpowers/specs/ for session lifecycle (§4.1), suspend/resume answer routing (§3.1), and liveness clocks (SPEC_STREAM_MIDRUN_LIVENESS).
