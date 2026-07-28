# src/libs/colmena/src/dag_engine/infrastructure/persistence/mod.rs

**Layer:** infrastructure  **Purpose:** Module aggregator re-exporting PostgreSQL-backed DAG state and secure value repositories.

## Symbols

- `postgres_dag_state_repository` (mod, pub) — declares submodule for PostgreSQL DAG state persistence implementation
- `postgres_secure_value_repository` (mod, pub) — declares submodule for PostgreSQL secure value persistence implementation
- `PostgresDagStateRepository` (use/type, pub) — re-exports the PostgreSQL DAG state repository type for application-layer access
- `PostgresSecureValueRepository` (use/type, pub) — re-exports the PostgreSQL secure value repository type for application-layer access

## File-level notes

- Straightforward infrastructure persistence module aggregator; no logic, only declarations and re-exports
- Both submodules appear actively used by the application layer (judged by intentional re-exports)
- No inline implementations or trait definitions; delegates all behavior to submodules
