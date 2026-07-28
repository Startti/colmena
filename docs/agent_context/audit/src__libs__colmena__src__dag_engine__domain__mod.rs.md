# src/libs/colmena/src/dag_engine/domain/mod.rs

**Layer:** domain  
**Purpose:** Module index for the hexagonal-architecture domain layer; organizes and publicly exposes core domain concepts (graph, node, errors, events, repositories, permissions) without infrastructure coupling.

## Symbols

- `mod error` (public module) — Domain error types and definitions
- `mod events` (public module) — Domain event types emitted during DAG execution
- `mod graph` (public module) — Graph domain model and structures
- `mod node` (public module) — ExecutableNode trait and node abstractions
- `mod observer` (public module) — Observer pattern for domain events and lifecycle hooks
- `mod secure_value_repository` (public module) — Trait defining secure value repository port
- `SecureValueRepository` (public type re-export) — Public export of the SecureValueRepository trait for domain callers
- `mod initializable_node` (public module) — Node initialization trait and helpers
- `mod sql_errors` (public module) — SQL-specific error types in the domain
- `mod sql_permissions` (public module) — SQL permission models (allowed schemas, operations)
- `mod sql_ports` (public module) — SQL repository port traits (connection, validation)

## File-level notes

- Pure module organization file with no logic or implementations
- All modules are public, making the entire domain layer API surface explicit
- Comment (line 1, Spanish) indicates intentional public exposure of `graph` and `node` modules
- No dependencies, no imports beyond `pub mod`/`pub use` declarations
- No candidates for cleanup or refactoring
