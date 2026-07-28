# src/libs/colmena/src/dag_engine/infrastructure/pool_registry/config.rs

**Layer:** infrastructure  
**Purpose:** Validated configuration for `PgPoolRegistry`, providing environment-variable parsing and range validation for PostgreSQL connection pool parameters.

## Symbols

- `ConfigError` (enum, pub) — Validation error type wrapping invalid env var names and reasons
- `ConfigError::Invalid` (variant, pub) — Indicates an env var or constraint validation failed with variable name and reason
- `PoolConfig` (struct, pub) — Configuration container for connection pool limits, timeouts, and lifetime settings
- `PoolConfig::max_entries` (field, pub) — Maximum number of distinct database URL pools to cache
- `PoolConfig::max_conn_per_url` (field, pub) — Maximum connections in a single URL's pool
- `PoolConfig::min_conn_per_url` (field, pub) — Minimum connections maintained in a single URL's pool
- `PoolConfig::idle_timeout` (field, pub) — Duration before closing an idle connection
- `PoolConfig::max_lifetime` (field, pub) — Maximum lifetime of any connection before forced closure
- `PoolConfig::acquire_timeout` (field, pub) — Timeout for acquiring a connection from the pool
- `PoolConfig::defaults()` (fn, pub) — Returns hard-coded default configuration (100 entries, 2/0 conn per URL, 30s/600s/10s timeouts)
- `PoolConfig::from_env()` (fn, pub) — Parses environment variables with defaults and validates all fields against strict ranges (1–10000 entries, 1–50 conn per URL, 10–3600s idle, 60–86400s lifetime, 1–60s acquire)
- `parse` (fn, private) — Inner helper in `from_env` that parses a typed env var or returns a default, with generic `FromStr` bound
- `tests` (mod, cfg test) — Unit tests validating defaults, env fallbacks, and constraint violations

## File-level notes

- No flagged issues detected. Code is defensive and well-structured.
- Validation ranges are comprehensive and match documented pool strategy.
- Test coverage includes defaults, env fallback behavior, and out-of-range rejection; all tests use `#[serial]` to prevent env-var leakage between runs.
- Inner `parse` helper is appropriately tightly scoped and generic.
