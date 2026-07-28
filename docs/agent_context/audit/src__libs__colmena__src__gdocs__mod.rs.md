# src/libs/colmena/src/gdocs/mod.rs

**Layer:** application/infrastructure coordinator  
**Purpose:** Module root for Google Docs integration (Subsystem G) — organizes domain/application/infrastructure layers following hexagonal architecture.

## Symbols

- `application` (mod, pub) — high-level use cases (replace_text, insert, style, apply_edits, co_edit_guard, …)
- `domain` (mod, pub) — DocsClient port trait, value types, error definitions
- `infrastructure` (mod, pub) — REST adapter, OAuth/SA auth, config, postgres revision store, in-memory outline cache

## File-level notes

- Module declaration only — no implementations in this file. Clean separation of concerns; tool dispatchers are delegated to `crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::gdocs_tools`.
- Hexagonal structure is well-documented in module-level comment: ports in domain, business logic in application, external integrations in infrastructure.
- No symbols flagged — this is a clean, minimal coordinator.
