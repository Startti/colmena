# src/libs/colmena/src/dag_engine/main.rs

**Layer:** infrastructure  **Purpose:** CLI entry point for the DAG engine. Routes four subcommands: run (execute graph and stream events), serve (HTTP server mode), crdt-yws (standalone CRDT server), crdt-yws-graph (CRDT server + graph execution against shared runtime), and crdt-agent (one-shot CRDT mutation peer).

## Symbols

- `Cli` (struct, public) — Clap parser struct containing the subcommand selector
- `Cli::command` (field, public) — The selected CLI subcommand variant
- `CrdtAgentMode` (enum, public) — Subcommand variants for CRDT agent mutation modes
- `CrdtAgentMode::Ws` (variant, public) — Connect to crdt-yws via WebSocket and apply a set_cell mutation
- `CrdtAgentMode::Inproc` (variant, public) — POST to HTTP in-proc CRDT endpoint (sanity-check alternative to WS)
- `Commands` (enum, public) — Top-level CLI subcommand enum
- `Commands::Run` (variant, public) — Execute a graph JSON file locally with optional resume/answer/session parameters
- `Commands::Serve` (variant, public) — Serve a graph over HTTP API (wraps api::serve_dag)
- `Commands::CrdtYws` (variant, public) — Run standalone CRDT WebSocket server with localfs artifact storage
- `Commands::CrdtYwsGraph` (variant, public) — Run CRDT server + execute a graph against shared process-wide runtime singleton
- `Commands::CrdtAgent` (variant, public) — One-shot agent peer to mutate a CRDT artifact
- `main` (fn, async, private) — Tokio async entry point; initializes pyo3, parses CLI via clap, dispatches to subcommand handlers

## File-level notes

- **pyo3 initialization (line 125)**: `Python::initialize()` is called unconditionally because `python_script` node depends on pyo3 runtime, even though the `python` feature flag only controls PyO3 *bindings* (not the engine's own Python VM). Comment correctly explains this distinction.
- **Verbose mode (lines 142–145, 215–218)**: Correctly honors both CLI `--verbose` flag and `COLMENA_VERBOSE=1` env var with logical OR.
- **Engine lifecycle (Run and CrdtYwsGraph)**: Engine is always shut down after graph execution (lines 203, 393), even on error, via explicit `engine.shutdown().await`. Correct.
- **Storage path diagnostics (CrdtYws, CrdtYwsGraph)**: Path canonicalization and absolute-path printing prevents silent artifact reuse failures when server is restarted from different cwd (lines 249–252, 312–315). Same intent duplicated but acceptable in CLI context.
- **CRDT runtime singleton (CrdtYwsGraph, line 293)**: Process-wide singleton installed via `process_runtime::set_global()` so both WS server and llm_call dispatcher share one runtime instance. Enables transparent tool-driven mutations visible to browser peers.
- **Seed artifact (CrdtYwsGraph, lines 297–305)**: Optional pre-creation of an artifact so the graph finds it on first tool call; validates ArtifactId format.
- **Server pause before graph (CrdtYwsGraph, lines 334–339)**: Optional `--wait-before-graph` delay allows operator to open browser before agent mutations fire.
- **Signal handling (CrdtYwsGraph, line 403)**: `tokio::signal::ctrl_c()` blocks until Ctrl+C so operator can inspect browser; graceful shutdown on runtime (line 406).
- **Error handling**: Uses `anyhow::Error` with `?` propagation for CLI-appropriate error reporting; individual errors are caught and formatted as SSE "error" frames where applicable (lines 183–189, 371–382).
- **Stream handling**: Both Run and CrdtYwsGraph use `SseMapper` to transform engine events into SSE frames; streams are pinned and polled until exhaustion or error.
