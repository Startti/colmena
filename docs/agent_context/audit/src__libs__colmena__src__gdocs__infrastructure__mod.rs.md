# src/libs/colmena/src/gdocs/infrastructure/mod.rs

**Layer:** infrastructure  
**Purpose:** Module aggregator for Google Docs infrastructure adapters—exports submodules for authentication, HTTP client, configuration, document-structure caching, and revision persistence.

## Symbols

- `auth` (mod, pub) — Submodule exporting Google Docs authentication strategies and credential handling
- `config` (mod, pub) — Submodule exporting Google Docs infrastructure configuration types
- `http_client` (mod, pub) — Submodule exporting the Google Docs HTTP REST client adapter
- `outline_cache` (mod, pub) — Submodule exporting document outline and structure caching infrastructure
- `revision_store` (mod, pub) — Submodule exporting document revision persistence and tracking

## File-level notes

- Pure module re-export aggregator; no implementations or logic present
- Delegates all infrastructure concern to well-organized submodules
- No dependencies, no public types/functions defined at this level
- Follows standard Rust module visibility pattern for infrastructure layer organization
