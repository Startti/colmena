# src/libs/colmena/src/lib.rs

**Layer:** shared  **Purpose:** Crate root module that organizes and re-exports the public API of Colmena library, including DAG engine, LLM abstractions, bindings for Python/Node.js, and domain subsystems (sheets, docs, storage, etc.).

## Symbols

### Modules (public)
- `crdt_documents` — CRDT-based document collaboration infrastructure (Google Docs integration)
- `dag_engine` — Core DAG orchestration engine with 25+ node types and execution runtime
- `documents` — Document abstraction and handling layer
- `gdocs` — Google Docs integration subsystem
- `google_oauth` — OAuth 2.0 integration for Google service authentication
- `gsheets` — Google Sheets integration subsystem
- `llm` — Multi-provider LLM abstraction (OpenAI, Anthropic, Gemini) with application layer
- `skills` — Skills registry and loading system for LLM tool context enrichment
- `storage` — Output storage adapters (local cache, HTTP callback, GCS)
- `text` — LLM-facing text registry (prompts, descriptions, summaries in YAML/Markdown)
- `web` — Web integration layer (HTTP utilities, Socket.IO)
- `shared` — Shared utilities (config resolution, service container, error types)

### Feature-gated modules (public)
- `node_bindings` (feature="node") — napi-rs bindings for TypeScript/Node.js
- `python_bindings` (feature="python") — PyO3 bindings for Python

### Macros (exported)
- `colmena_log!` (macro) — Conditional debug/verbose printing that reads `--verbose` CLI flag or `COLMENA_VERBOSE=1` env variable; no-op if not enabled

### Re-exports (public)
- `pub use llm::*;` — Re-exports all public symbols from `llm` module
- `pub use node_bindings::*;` (feature="node") — Re-exports all public symbols from `node_bindings` module
- `pub use python_bindings::*;` (feature="python") — Re-exports all public symbols from `python_bindings` module

## File-level notes

- **Structure:** Clean crate root following Rust conventions; minimal logic (only module organization and re-exports).
- **Visibility strategy:** Public modules are directly declared; feature-gated bindings conditionally included; `llm` is re-exported wholesale (`pub use *`) to expose domain/application from that subsystem to library consumers.
- **No architectural concerns:** All subsystems are at the right visibility level; no dead code or stubs detected.
