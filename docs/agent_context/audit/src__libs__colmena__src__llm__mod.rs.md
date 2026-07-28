# src/libs/colmena/src/llm/mod.rs

**Layer:** shared  **Purpose:** Orchestration facade that re-exports the domain, application, and infrastructure layers of the LLM module, with intentional ambiguous glob handling for the attachments submodule conflict.

## Symbols

- `application` (mod, pub) — Public re-export of the LLM application layer (use cases)
- `domain` (mod, pub) — Public re-export of the LLM domain layer (traits, value objects, errors)
- `infrastructure` (mod, pub) — Public re-export of the LLM infrastructure layer (provider adapters, persistence)

## File-level notes

- **Ambiguous glob re-exports intentionally allowed**: Both `domain::attachments` and `infrastructure::attachments` exist (Plan A domain concept + infra adapter). The `#[allow(ambiguous_glob_reexports)]` attributes (lines 9, 11, 13) suppress compiler warnings because glob imports would be ambiguous; users must use fully-qualified paths to distinguish them. This is documented in the comment (lines 5–8) and is by design.
- **No logic in this file**: This is a pure re-export facade with no executable code, tests, or conditionals.
