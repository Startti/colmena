# src/libs/colmena/src/gsheets/application/mod.rs

**Layer:** application  
**Purpose:** Module root for Google Sheets application-layer use cases. Declares and re-exports the `format` submodule containing pure formatting logic and data types for Google Sheets API integration.

## Symbols

- `format` (mod, pub) — Submodule containing `FormatSpec` types and pure mapping logic for Google Sheets formatting requests.

## File-level notes

- This is a minimal module declaration file with no code logic at the `mod.rs` level; all application logic resides in `format.rs`.
- The `format` submodule is actively used: imported by `gsheets_integration_test.rs` and `gsheets_tools.rs` (infrastructure/nodes layer).
- Follows standard Rust module structure (no concerns).
