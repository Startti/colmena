# src/libs/colmena/src/documents/infrastructure/mod.rs

**Layer:** infrastructure  
**Purpose:** Module aggregator and barrel export for the documents infrastructure layer, exposing submodules (ids, render, storage, validation) and re-exporting the primary ID generator types.

## Symbols

- `ids` (mod, pub) — Submodule containing ID generation strategies (CountingIdGenerator, UlidIdGenerator)
- `render` (mod, pub) — Submodule containing document rendering logic
- `storage` (mod, pub) — Submodule containing storage abstractions and implementations
- `validation` (mod, pub) — Submodule containing document validation logic
- `CountingIdGenerator` (type, pub use) — Re-exported from ids module; simple incrementing ID generator
- `UlidIdGenerator` (type, pub use) — Re-exported from ids module; ULID-based ID generator

## File-level notes

- File is a pure module aggregator with no code implementation.
- Public re-exports of ID generators indicate these are the primary consumer-facing types from the infrastructure layer.
- Follows Rust conventions for barrel modules (declaring submodules and re-exporting key types).
- No dependencies, error handling, or business logic in this file.
