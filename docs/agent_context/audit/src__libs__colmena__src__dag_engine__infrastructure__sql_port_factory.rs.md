# src/libs/colmena/src/dag_engine/infrastructure/sql_port_factory.rs

**Layer:** infrastructure  
**Purpose:** Factory for creating `PgPoolAdapter` instances that wrap shared registry pools, isolating per-adapter statement timeout and work_mem settings so multiple nodes hitting the same URL with different limits do not interfere.

## Symbols

- `SqlPortFactory` (struct, pub) — Holds an `Arc<PgPoolRegistry>` and vends `PgPoolAdapter` instances keyed by URL.
- `SqlPortFactory::new` (fn, pub) — Constructor accepting an `Arc<PgPoolRegistry>`.
- `SqlPortFactory::get_adapter` (async fn, pub) — Obtains or creates a pool for `url` from the registry, wraps it in a `PgPoolAdapter` with the given `statement_timeout_ms` and `work_mem_mb`, returns `Arc<PgPoolAdapter>` or `SqlNodeError::ConnectionError`.

## File-level notes

- Clean, focused factory with no dead code, unfinished stubs, or error handling gaps.
- Proper async/await usage and Arc wrapping for thread-safe sharing.
- Import hygiene: minimal, uses only necessary domain error and infrastructure adapters.
