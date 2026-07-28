# src/libs/colmena/src/gsheets/infrastructure/mod.rs

**Layer:** infrastructure  **Purpose:** Module index organizing the gsheets infrastructure layer, re-exporting auth, config, HTTP client, and merge-fill adapters.

## Symbols

- `auth` (mod, pub) — Re-exports authentication module for Google Sheets API authorization
- `config` (mod, pub) — Re-exports configuration module for gsheets infrastructure settings
- `http_client` (mod, pub) — Re-exports HTTP client module for REST API communication with Google Sheets
- `merge_fill` (mod, pub) — Re-exports merge-fill utility module for cell range operations

## File-level notes

- This is a minimal module definition file (6 lines) with no code logic.
- All re-exported submodules are infrastructure adapters (ports, HTTP client, config, utilities).
- No direct usage constraints visible at this scope; actual coupling determined by callers in application/domain layers.
