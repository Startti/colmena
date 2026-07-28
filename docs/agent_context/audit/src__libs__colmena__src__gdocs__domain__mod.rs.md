# src/libs/colmena/src/gdocs/domain/mod.rs

**Layer:** domain  
**Purpose:** Root module for the Google Docs domain layer (ports, value types, and error types); provides a clean public API by re-exporting from three internal submodules: errors, traits, and types.

## Symbols

- `mod errors` — submodule containing domain error types (DocsError and related variants)
- `mod traits` — submodule containing the DocsClient port trait defining the contract for Docs API interaction
- `mod types` — submodule containing value objects and data structures for Docs domain concepts
- `pub use errors::DocsError` — re-export of the primary domain error type for public consumption
- `pub use traits::DocsClient` — re-export of the DocsClient port trait for public consumption
- `pub use types::*` — bulk re-export of all public types from the types submodule

## File-level notes

- Clean, minimal module root following hexagonal architecture pattern — zero domain logic, only aggregation
- All re-exports are intentional and properly scoped; no import shadowing or confusion
- Consistent with other domain roots in the codebase (e.g. llm/domain/mod.rs)
- No infrastructure dependencies (comment verified at line 1)
