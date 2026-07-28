# src/libs/colmena/src/gsheets/mod.rs

**Layer:** infrastructure  
**Purpose:** Module root for Google Sheets integration (Subsystem E). Organizes the gsheets subsystem into domain (SheetsClient trait and value types), application (use cases), and infrastructure (REST adapter and auth).

## Symbols

- `application` (mod, public) — Google Sheets application layer (use cases, orchestration)
- `domain` (mod, public) — Google Sheets domain layer (SheetsClient port, value objects, errors)
- `infrastructure` (mod, public) — Google Sheets infrastructure layer (REST client adapter, authentication, config)

## File-level notes

- This is a pure module-organization file with no executable code; it re-exports three submodules that implement hexagonal architecture for Google Sheets.
- The crate-level doc comment correctly points tool dispatchers to their location: `dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_tools`.
- No complexity or antipatterns; clean architectural segregation.
