# src/libs/colmena/src/dag_engine/infrastructure/nodes/router/mod.rs

**Layer:** infrastructure  
**Purpose:** Organizes the router node implementation (declarative branching with LLM-direct and extract+rules modes). Re-exports public API and submodule structure for router configuration, LLM-direct routing, extract-and-route routing, and DSL-based conditional branching.

## Symbols

- `config` (pub mod) — submodule containing router configuration types and schemas
- `extract_and_route` (pub mod) — submodule implementing extract-and-route branching logic (LLM extracts data, rules determine output)
- `llm_direct` (pub mod) — submodule implementing LLM-direct branching (LLM directly selects output branch)
- `when_dsl` (pub mod) — submodule implementing declarative when-DSL for condition-based branching
- `node` (private mod) — submodule containing the core RouterNode implementation
- `RouterNode` (pub use from node) — re-exported public type representing the executable router node

## File-level notes

- Minimal file; purely organizational. No logic, no tests, no direct dependencies.
- Design spec reference (2026-05-31) is in the comment; implementation details are in submodules.
- Follows standard Rust mod.rs pattern for module hierarchy — clean separation of concerns across four routing strategies.
