# src/libs/colmena/src/dag_engine/application/mod.rs

**Layer:** application  **Purpose:** Hub module organizing the application layer of the DAG engine, declaring public submodules for DAG execution, services, and use cases.

## Symbols

- `list_tool_executor` (mod, pub) — Module for executing tools in a list/batch context (deterministic iteration)
- `liveness` (mod, pub) — Module for tracking execution liveness, state, and frame forwarding
- `ports` (mod, pub) — Module defining application-level ports (interfaces/traits)
- `run_use_case` (mod, pub) — Module implementing the main DAG run orchestration use case
- `secure_value_service` (mod, pub) — Module for handling secure value encryption and decryption
- `sql_execution_service` (mod, pub) — Module for SQL query execution and validation
- `SecureValueService` (type, pub) — Re-exported service type from secure_value_service for secure value operations

## File-level notes

- This is a pure module hub (re-export file) with no implementations; all logic lives in submodules
- Single re-export (`SecureValueService`) is the only public item beyond submodule declarations
- No redundant re-exports; all submodules are actively used in the DAG engine architecture
