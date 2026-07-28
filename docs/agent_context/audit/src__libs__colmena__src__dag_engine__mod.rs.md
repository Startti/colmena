# src/libs/colmena/src/dag_engine/mod.rs

**Layer:** infrastructure  
**Purpose:** Module root for the DAG engine subsystem. Organizes and re-exports the public API (domain, application, infrastructure, engine, API, utilities).

## Symbols

- `api` (pub mod) — Public API bindings and utilities for the DAG engine  
- `application` (pub mod) — Application layer: orchestration use cases and DAG execution logic  
- `domain` (pub mod) — Domain layer: node traits, graph structures, and core abstractions  
- `engine` (pub mod) — Engine abstraction and core execution logic  
- `infrastructure` (pub mod) — Infrastructure layer: node implementations, adapters, and concrete services  
- `sse_mapper` (pub mod) — Server-Sent Events (SSE) formatting and message mapping utilities  
- `verbose` (pub mod) — Verbose/debug output utilities for DAG execution and logging  

## File-level notes

- This file is a minimal module root containing only re-exports. It organizes the hexagonal architecture layers (domain/application/infrastructure) plus utility modules under a single namespace.
- No implementations, logic, or conditional compilation directives present.
- All submodules are public, indicating full re-export of the subsystem's API surface.
