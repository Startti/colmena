# src/libs/colmena/src/python_bindings/mod.rs

**Layer:** bindings  
**Purpose:** Exposes Colmena's LLM multi-provider abstraction and DAG engine execution to Python via PyO3, providing both synchronous and async APIs for LLM calls, streaming, DAG execution, and registry inspection.

## Symbols

- `crdt_documents` (mod, private) — Submodule declaring CRDT document bindings
- `LlmException` (exception, pub) — PyO3 exception type for LLM domain errors
- `From<LlmError> for PyErr` (impl, pub) — Converts domain LlmError to Python exception with stringified message
- `PyLlmStream` (struct, private) — Python-facing async iterator wrapping Arc<Mutex<LlmStream>>
- `PyLlmStream::__aiter__` (fn, private) — Returns self as async iterator protocol entry point
- `PyLlmStream::__anext__` (fn, private) — Pulls next stream chunk, returns string content or raises StopAsyncIteration when exhausted
- `LlmConfigOptions` (struct, pub) — Configuration value object (api_key, model, temperature, max_tokens, penalties); derives Clone, Default
- `LlmConfigOptions::new` (fn, pub) — Constructor returning default instance
- `ColmenaLlm` (struct, pub) — Main LLM client managing per-provider ServiceContainer map
- `ColmenaLlm::new` (fn, pub) — Initializes ConfigResolver and creates all provider containers via ServiceContainerFactory
- `ColmenaLlm::call` (fn, pub) — Synchronous LLM call: parses message dicts → creates LlmConfig → delegates to tokio runtime → returns string response
- `ColmenaLlm::stream` (fn, pub) — Async LLM streaming: parses messages → returns PyDagStream-like async iterator of text chunks
- `ColmenaLlm::health_check` (fn, pub) — Provider health check: executes LlmHealthCheck in tokio runtime, returns bool
- `ColmenaLlm::get_providers` (fn, pub) — Lists available provider names from container keys
- `DagPartStream` (type alias, private) — Pinned boxed Stream of SSE-mapped DAG parts (serde_json::Value)
- `PyDagStream` (struct, private) — Python-facing async iterator wrapping Arc<Mutex<DagPartStream>>
- `PyDagStream::__aiter__` (fn, private) — Returns self as async iterator protocol entry point
- `PyDagStream::__anext__` (fn, private) — Pulls next DAG part, pythonizes JSON to Python dict, or raises StopAsyncIteration
- `run_dag` (fn, pub) — Synchronous DAG execution from file path or dict: parses graph source → delegates to tokio runtime → returns JSON string result
- `stream_dag` (fn, pub) — Async DAG streaming: parses graph source → returns awaitable resolving to PyDagStream async iterator
- `validate_graph` (fn, pub) — Validates graph dict deserializes into engine's Graph type; smoke-test helper
- `Registry` (struct, pub) — Read-only wrapper around HashMapNodeRegistry; exposes inspection helpers for smoke tests
- `Registry::node_types` (fn, pub) — Returns sorted list of all registered node type names
- `Registry::toolkit_catalog` (fn, pub) — Returns sub-tool catalog for a toolkit node as list of {name, description, required} dicts
- `SmokeTaskMemory` (struct, private) — Stub DagTaskMemoryRepository implementation; all methods return empty/ok for in-memory registry tests
- `SmokeTaskMemory::add_task` (fn, private) — No-op task insertion
- `SmokeTaskMemory::update_task_result` (fn, private) — No-op task result update
- `SmokeTaskMemory::get_tasks_for_run` (fn, private) — Returns empty vec
- `SmokeTaskMemory::get_first_uncompleted_task` (fn, private) — Returns None
- `SmokeTaskMemory::delete_task` (fn, private) — No-op deletion
- `SmokeTaskMemory::clear_tasks_for_run` (fn, private) — No-op clear
- `SmokeTaskMemory::get_current_phase` (fn, private) — Returns None
- `SmokeTaskMemory::get_uncompleted_tasks_for_phase` (fn, private) — Returns empty vec
- `SmokeTaskMemory::save_phase_summary` (fn, private) — No-op save
- `SmokeTaskMemory::get_phase_summaries` (fn, private) — Returns empty vec
- `default_registry` (fn, pub) — Builds in-memory HashMapNodeRegistry with no live DB (PgPoolRegistry, ConversationRepositoryFactory, SqlPortFactory, SmokeTaskMemory)
- `serve_dag` (fn, pub) — Serves DAG JSON as HTTP server on host:port via dag_engine::api::serve_dag
- `colmena` (fn, pub) — PyModule init function; registers all classes (ColmenaLlm, LlmConfigOptions, Registry, exceptions) and functions (run_dag, stream_dag, serve_dag, validate_graph, default_registry)

## File-level notes

- **Message parsing duplication** (improvement): Lines 114–143 (`call`) and lines 187–216 (`stream`) implement identical message dict-to-LlmMessage conversion. Extract to a shared helper to reduce maintenance burden.
- **GraphSource enum duplication** (improvement): Lines 331–346 (`run_dag`) and lines 417–432 (`stream_dag`) define and parse the identical GraphSource enum. Extract to module-level to avoid repeat parsing logic and type duplication.
- **Missing doc comments** (improvement): Public API items (`ColmenaLlm`, `run_dag`, `LlmConfigOptions`, `Registry`) lack `///` documentation visible to Python callers and Rust doc consumers. Add docstrings, especially for method signatures and error conditions.
- **Async/await pattern**: Both `PyLlmStream` and `PyDagStream` follow the same `Arc<Mutex<...>>` + `future_into_py` pattern; idiomatic for PyO3 but worth noting as a design pattern.
- **No unfinished stubs**: No `todo!()`, `unimplemented!()`, FIXME, or TODO comments; all code is complete.
- **All symbols used**: Private structs (PyLlmStream, PyDagStream, SmokeTaskMemory) are exported or consumed within the module; no obvious dead code.
