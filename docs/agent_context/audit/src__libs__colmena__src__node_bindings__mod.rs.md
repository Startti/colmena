# src/libs/colmena/src/node_bindings/mod.rs

**Layer:** bindings  
**Purpose:** napi-rs TypeScript/Node.js binding aggregator that re-exports all napi-decorated items from submodules (dag, documents, llm, registry, stream) into a single public surface.

## Symbols

- `dag` (mod, private) — collects DAG-related napi bindings for execution engine operations
- `documents` (mod, private) — collects document/attachment-related napi bindings
- `llm` (mod, private) — collects LLM provider and call-related napi bindings
- `registry` (mod, private) — collects node registry and configuration napi bindings
- `stream` (mod, public) — re-exports stream event handling and SSE utilities publicly
- `pub use dag::*` (re-export) — flattens all public items from dag module into root namespace
- `pub use documents::*` (re-export) — flattens all public items from documents module into root namespace
- `pub use llm::*` (re-export) — flattens all public items from llm module into root namespace
- `pub use registry::*` (re-export) — flattens all public items from registry module into root namespace
- `pub use stream::*` (re-export) — flattens all public items from stream module into root namespace

## File-level notes

- Clean, idiomatic module aggregator with no implementation or complexity
- All submodules are private except `stream`, but all are re-exported via `pub use *`, making the visibility distinction immaterial from the consumer's perspective
- The file serves as the entry point for the entire napi binding layer, scoped to what napi-rs collects via `#[napi]` macro decorators
- No dead code, stubs, or improvement opportunities detected
