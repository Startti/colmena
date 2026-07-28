# src/libs/colmena/src/node_bindings/dag.rs

**Layer:** bindings  
**Purpose:** Provides napi-rs Node.js/TypeScript bindings for DAG engine execution, streaming, and validation operations. Thin wrapper layer exposing Rust DAG APIs via `#[napi]` macros.

## Symbols

- `run_dag` (pub async fn) — Executes a DAG graph from a file path, returns JSON result with optional resume/inject/session parameters
- `run_dag_from_json` (pub async fn) — Executes a DAG graph from a JSON string instead of file path, supports in-memory graph objects
- `serve_dag` (pub async fn) — Starts an HTTP server serving a DAG graph with configurable host and port
- `validate_graph` (pub fn) — Validates a graph object as a valid Colmena graph, throws error if invalid
- `stream_dag` (pub async fn) — Streams a DAG file's execution as SSE-mapped events via a pull-based handle
- `stream_dag_from_json` (pub async fn) — Streams a DAG graph from a JSON string as SSE events via pull-based handle

## File-level notes

- Well-structured with two logical sections (DAG Engine Bindings, DAG Streaming Bindings) separated by comments
- All functions are thin wrappers delegating to `crate::dag_engine::api::*` counterparts; error handling is uniform via `.map_err(|e| Error::new(Status::GenericFailure, e.to_string()))`
- Consistent optional parameter unwrapping patterns (`unwrap_or(false)` for bools, `unwrap_or_else` for host/port)
- Doc comments present on all public functions explaining intent and usage context (file path vs. JSON, streaming vs. completion)
- No dead code, unfinished stubs, or obvious improvements detected
