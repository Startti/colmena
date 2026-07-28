# src/libs/colmena/src/dag_engine/infrastructure/pool_registry/mod.rs

**Layer:** infrastructure  **Purpose:** Re-exports the public API of the Postgres connection-pool registry system. Manages a single source of truth for all `PgPool` instances, keyed by normalized connection URL and reused across jobs and consumers.

## Symbols

- `config` (mod, private) — submodule defining pool configuration types and validation
- `error` (mod, private) — submodule defining registry-specific error types
- `metrics` (mod, private) — submodule defining pool and registry metrics
- `registry` (mod, private) — submodule defining the main `PgPoolRegistry` struct and logic
- `url_key` (mod, private) — submodule defining URL normalization and key management
- `ConfigError` (type, pub) — re-exported error type for pool configuration failures
- `PoolConfig` (type, pub) — re-exported configuration struct for connection pools
- `RegistryError` (type, pub) — re-exported error type for registry operations
- `PoolMetrics` (type, pub) — re-exported metrics for individual pools
- `RegistryMetrics` (type, pub) — re-exported metrics for the entire registry
- `PgPoolRegistry` (type, pub) — re-exported main registry struct for managing connection pools
- `UrlKey` (type, pub(crate)) — re-exported normalized URL key type used internally by the crate

## File-level notes

- This is a pure API re-export module with no implementation logic; all substantive code resides in submodules.
- Line 17 carries `#[allow(unused_imports)]` for `UrlKey`, suggesting the linter initially flagged it as unused. Since it is exported as `pub(crate)`, this suppression is valid if other crate-internal modules consume it; if not, it is a dead re-export.
- The module-level doc comment references a design spec at `docs/superpowers/specs/2026-04-20-connection-pool-management-design.md`.
