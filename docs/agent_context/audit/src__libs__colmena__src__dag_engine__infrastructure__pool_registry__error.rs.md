# src/libs/colmena/src/dag_engine/infrastructure/pool_registry/error.rs

**Layer:** infrastructure  **Purpose:** Defines error types for database pool registry operations, including pool creation failures and registry lifecycle errors.

## Symbols

- `RegistryError` (enum, pub) — Error type for pool registry with two variants: pool creation and registry closed states
- `RegistryError::PoolCreation` (variant, pub) — Pool creation failure with sqlx error message wrapped as String
- `RegistryError::Closed` (variant, pub) — Registry closed error (no additional data)
- `From<sqlx::Error> for RegistryError` (impl, pub) — Converts sqlx::Error into RegistryError::PoolCreation by capturing error message

## File-level notes

- Minimal, focused error module following Rust error-handling best practices
- Uses `#[derive(Debug, Error)]` with `thiserror` for ergonomic error formatting
- `From` impl is appropriate: captures sqlx error context for downstream logging/reporting
- No error variants for other pool registry failures (e.g., pool exhaustion, connection timeout); current scope is narrow and appropriate for this infrastructure module
- No dead code, complexity, or unfinished patterns detected
