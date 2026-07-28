# src/libs/colmena/src/dag_engine/domain/sql_errors.rs

**Layer:** domain  
**Purpose:** Defines error types for the SQL node validation and execution pipeline, covering validator blocks, critic rejection, connection failures, execution errors, and configuration issues.

## Symbols

- `SqlNodeError` (enum, pub) — Error type for SQL node pipeline with five variants representing different failure modes
- `SqlNodeError::Blocked` (variant, pub) — Query blocked by static validator rules; contains rule name and message
- `SqlNodeError::CriticRejected` (variant, pub) — Query rejected by LLM critic for security concerns; contains reason string
- `SqlNodeError::ConnectionError` (variant, pub) — PostgreSQL connection or pool creation failure; wraps error message
- `SqlNodeError::ExecutionError` (variant, pub) — Query execution failure at PostgreSQL level; wraps error message
- `SqlNodeError::ConfigError` (variant, pub) — Invalid permission configuration; wraps error message
- `fmt::Display impl` (impl, pub) — Implements human-readable Display formatting for all SqlNodeError variants
- `std::error::Error impl` (impl, pub) — Makes SqlNodeError implement the standard Error trait

## File-level notes

- Clean, minimal error type with no complexity or gaps. All variants are well-distinguished and clearly documented.
- No external dependencies; uses only `std::fmt` and standard error traits.
- Display implementation exhaustively covers all five variants with clear, user-friendly messages.
- No dead code, unfinished stubs, or obvious improvements identified.
