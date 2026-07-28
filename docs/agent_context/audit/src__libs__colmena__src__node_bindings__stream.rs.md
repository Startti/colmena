# src/libs/colmena/src/node_bindings/stream.rs

**Layer:** bindings  **Purpose:** Provides napi-rs async-iterator handles for streaming LLM text output and DAG execution events to TypeScript consumers.

## Symbols

- `LlmStreamHandle` (struct, pub) — napi-bound wrapper over an LLM text stream (`Arc<Mutex<LlmStream>>`), accessed via `pull()` for async iteration
- `LlmStreamHandle::pull` (fn, pub async) — locks the stream, retrieves the next text chunk via `next().await`, and returns `Some(String)` or `None` when exhausted; converts stream errors to napi `GenericFailure` status
- `LlmStreamHandle::new` (fn, pub) — constructs a new handle by wrapping a raw `LlmStream` in an `Arc<Mutex<>>`
- `DagPartStream` (type alias, pub) — owned, pinned, boxed `futures::Stream<Item=Result<serde_json::Value, DagError>> + Send` representing SSE-mapped DAG execution events
- `DagStreamHandle` (struct, pub) — napi-bound wrapper over a `DagPartStream` (`Arc<Mutex<DagPartStream>>`), accessed via `pull()` for async iteration
- `DagStreamHandle::pull` (fn, pub async) — locks the stream, retrieves the next DAG event (as `Value`) via `next().await`, and returns `Some(Value)` or `None` when the graph finishes; converts stream errors to napi `GenericFailure` status
- `DagStreamHandle::new` (fn, pub) — constructs a new handle by wrapping a raw `DagPartStream` in an `Arc<Mutex<>>`

## File-level notes

- Both handle types follow identical structural patterns (mutex-wrapped stream, async `pull()` method, non-napi `new()` constructor) — this duplication is idiomatic for napi-rs bindings, providing type safety and clear semantics on the TypeScript side.
- Error handling is uniform: stream errors become `Status::GenericFailure` via `e.to_string()`.
- No intra-crate imports; depends only on external crates (`futures`, `napi`, `tokio`, `serde_json`).
- Both `LlmStreamHandle::new` and `DagStreamHandle::new` are used by `node_bindings/dag.rs` and `node_bindings/llm.rs` respectively (per module dependency map).
