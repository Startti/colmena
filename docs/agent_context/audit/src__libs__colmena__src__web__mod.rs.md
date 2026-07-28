# src/libs/colmena/src/web/mod.rs

**Layer:** infrastructure  **Purpose:** Module root for web toolkit infrastructure (tavily_client, api_explorer, browser nodes). Re-exports domain, application, and infrastructure submodules organized per hexagonal architecture.

## Symbols
- `application` (mod, pub) — application layer use cases for web toolkit nodes
- `domain` (mod, pub) — domain layer traits, value objects, and errors for web toolkit (SessionRegistry, WebDomainError)
- `infrastructure` (mod, pub) — infrastructure layer adapters and implementations for web toolkit nodes

## File-level notes
- Pure module re-export; all logic lives in submodules
- Module doc points to design spec: `docs/superpowers/specs/2026-04-23-web-nodes-unified-design.md`
- No symbols to flag; file is well-structured
