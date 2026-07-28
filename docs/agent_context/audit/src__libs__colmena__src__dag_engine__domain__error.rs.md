# src/libs/colmena/src/dag_engine/domain/error.rs

**Layer:** domain  
**Purpose:** Defines domain-layer error types for the DAG engine using `thiserror`. Captures validation and execution failures across 7 distinct scenarios.

## Symbols

- `DagError` (enum, pub) — Domain error type for DAG validation and execution; derives Debug and Error via thiserror [FLAG: none]
  - `CycleDetected` (unit variant) — Circular dependency detected during topological sort of the DAG
  - `NodeTypeNotFound(String)` (newtype variant) — Raised when a node's `node_type` is not registered in NodeRegistryPort
  - `NodeIdNotFound(String)` (newtype variant) — Raised when a referenced node ID does not exist in the graph's node map
  - `NodeExecution(String)` (newtype variant) — Generic wrapper for execution errors returned by an ExecutableNode
  - `StateError(String)` (newtype variant) — Wrapper for state persistence or recovery errors (Postgres-level)
  - `InvalidNodeId { node_id: String, reason: &'static str }` (struct variant) — Raised when a node ID violates structural invariants (e.g., contains reserved `/` character for path qualifiers)
  - `InvalidToolSchema { node_id: String, tool_name: String, reason: String }` (struct variant) — Raised before execution when a node's tool schema is invalid (e.g., array field without items); enables early detection for clarity and agent-actionable errors in subgraph contexts

## File-level notes

- All error messages are in Spanish, consistent with project convention for domain documentation.
- Error messages use `#[error(...)]` attributes with structured context (field interpolation in InvalidNodeId and InvalidToolSchema).
- The enum derives only Debug and Error via `thiserror`; no custom impl blocks or trait implementations in this file.
- Comments in Spanish reference the originating use case (e.g., "Se produce cuando el `DagRunUseCase` detecta..."), providing clear lineage to infrastructure callers.
- Generic String payloads in NodeExecution and StateError are intentional abstractions; detail loss is acceptable at the domain layer.
- CycleDetected carries no contextual payload; detection happens upstream in DagRunUseCase's topological sort.
- This is a complete, well-maintained error definition with no dead code, stubs, or incomplete implementations.
