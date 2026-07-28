# src/libs/colmena/src/storage/mod.rs

**Layer:** infrastructure  
**Purpose:** Organizes the storage subsystem for persisting media output (images, audio) produced by DAG nodes. Declares two submodules: a port trait in domain and two production/test adapters in infrastructure.

## Symbols

- `domain` (mod, pub) — Submodule defining the `OutputStorageRepository` port trait and its associated types for media persistence
- `infrastructure` (mod, pub) — Submodule implementing `LocalCacheStorageAdapter` (in-memory for CLI/tests) and `HttpCallbackStorageAdapter` (external API delegation for production)

## File-level notes

- File is a clean module root: 8 lines of documentation + 2 submodule declarations
- No symbolic complexity; organization is clear and idiomatic
- Documentation accurately describes the hexagonal architecture (port in domain, adapters in infrastructure)
