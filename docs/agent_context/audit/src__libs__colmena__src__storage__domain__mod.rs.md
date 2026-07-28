# src/libs/colmena/src/storage/domain/mod.rs

**Layer:** domain  
**Purpose:** Barrel module exposing the storage domain ports (OutputStorageRepository trait, value objects StoreRequest/StoredBytes/StoredOutput/StoredStream) and error types, with zero infrastructure dependencies.

## Symbols

- `output_storage_repository` (mod, pub) — submodule containing OutputStorageRepository port and value objects
- `storage_error` (mod, pub) — submodule containing StorageError enum
- `OutputStorageRepository` (trait, pub) — port for persistent storage backend abstraction
- `StoreRequest` (struct, pub) — value object representing a storage request
- `StoredBytes` (type, pub) — value object representing stored bytes
- `StoredOutput` (struct, pub) — value object representing stored output with metadata
- `StoredStream` (type, pub) — value object representing a stored stream (e.g., streaming output)
- `StorageError` (enum, pub) — error type for all storage-layer failures
- `MockOutputStorageRepository` (type, pub, cfg(test)) — test mock implementation of OutputStorageRepository

## File-level notes

- Clean hexagonal barrel module with no infrastructure dependencies
- All symbols are either trait ports or value objects appropriate for domain layer
- Test mock is properly gated behind `#[cfg(test)]`
- No dead code, unfinished implementations, or obvious improvements detected
