# src/libs/colmena/src/crdt_documents/storage/mod.rs

**Layer:** application  **Purpose:** Storage abstraction (port) and factory for persisting CRDT artifact state and metadata; supports local filesystem and GCS backends.

## Symbols

- `ArtifactMeta` (struct, pub) — Metadata for a CRDT artifact (id, name, timestamps, sheet count).
- `StorageError` (enum, pub) — Domain error type for storage operations (IO, serde, not found, backend).
- `ArtifactStorage` (trait, pub, async) — Port trait defining methods to list, load, save, and delete artifact state and metadata.
- `StorageConfig` (enum, pub) — Configuration enum for selecting storage backend (LocalFs with root path, or GCS with bucket and prefix).
- `StorageConfig::build` (fn, pub) — Factory method that constructs an `Arc<dyn ArtifactStorage>` from the selected backend configuration.
- `gcs` (mod, pub, feature-gated) — GCS storage implementation module (conditional on `feature = "gcs"`).
- `localfs` (mod, pub) — Local filesystem storage implementation module.
- `LocalFsStorage` (re-export, pub) — Public re-export of the local filesystem storage adapter from `localfs`.

## File-level notes

- Well-structured hexagonal architecture: `ArtifactStorage` trait is the port (domain), `StorageConfig::build()` is the adapter factory (application).
- All trait methods are async with `#[async_trait]`, enabling non-blocking I/O across backends.
- GCS backend is appropriately feature-gated; always-available local filesystem provides a zero-dependency fallback.
- No TODOs, unimplemented!(), or incomplete code detected.
- Error handling via `thiserror` is domain-level; no error context enrichment in `build()` (acceptable for simple factory).
