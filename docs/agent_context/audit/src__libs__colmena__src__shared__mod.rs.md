# src/libs/colmena/src/shared/mod.rs

**Layer:** shared  
**Purpose:** Re-export module that provides public access to all shared infrastructure components.

## Symbols

- `infrastructure` (mod, pub) — submodule containing shared infrastructure utilities and adapters
- `pub use infrastructure::*;` (re-export) — re-exports all public items from the infrastructure submodule to the shared module's public API

## File-level notes

- This is a minimal re-export module. Its only function is to make infrastructure's public items accessible as part of the shared module's public surface.
- Contains no implementation code, only module organization.
- Follows standard Rust re-export pattern for organizing submodule interfaces.
