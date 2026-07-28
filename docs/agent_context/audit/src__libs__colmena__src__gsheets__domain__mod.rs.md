# src/libs/colmena/src/gsheets/domain/mod.rs

**Layer:** domain  
**Purpose:** Domain layer entry point for the Google Sheets subsystem, aggregating the SheetsClient port trait, SheetsError exception type, and all value types with zero infrastructure dependencies.

## Symbols

- `mod errors` (module) — submodule declaring the SheetsError enum
- `mod traits` (module) — submodule declaring the SheetsClient port trait
- `mod types` (module) — submodule declaring all value types
- `pub use errors::SheetsError` (re-export) — re-exports the SheetsError domain error type
- `pub use traits::SheetsClient` (re-export) — re-exports the SheetsClient hexagonal port
- `pub use types::*` (re-export) — wildcard re-export of all value types from the types module

## File-level notes

- This is a clean module facade performing standard domain aggregation with no business logic.
- All three submodules (errors, traits, types) are well-defined and actively used via the re-exports.
- No dead code, unfinished work, or obvious improvements needed.
- The wildcard re-export `pub use types::*` is a conventional and appropriate pattern for a domain layer entry point; it exposes the full type catalog to users while keeping submodule organization intact.
