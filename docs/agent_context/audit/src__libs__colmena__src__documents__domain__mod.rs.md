# src/libs/colmena/src/documents/domain/mod.rs

**Layer:** domain  **Purpose:** Barrel/index module for the documents domain layer. Organizes and re-exports type definitions, port traits (trait boundaries), and error types from six domain submodules (artifact, error, ids, ir, patch, ports).

## Symbols

### Modules (declarations)
- `artifact` (mod, pub) — Submodule defining artifact metadata, summaries, versioning, and patch application outcomes
- `error` (mod, pub) — Submodule defining domain-specific error types (DocumentError, StorageError, etc.)
- `ids` (mod, pub) — Submodule defining value-object identifiers (ArtifactId, AssetId, SessionId, VersionId, ArtifactKind)
- `ir` (mod, pub) — Submodule defining intermediate representation abstractions for document formats (Excel, HTML, Word, common IR)
- `patch` (mod, pub) — Submodule defining patch operation abstractions (Patch, PatchOp, PatchSource)
- `ports` (mod, pub) — Submodule defining port trait abstractions for storage, rendering, validation, and ID generation

### Re-exported types from `artifact`
- `ArtifactMeta` (pub, type re-export) — Metadata container for artifacts (likely source, timestamps, or provenance)
- `ArtifactSummary` (pub, type re-export) — Summary representation of an artifact
- `AssignedIds` (pub, type re-export) — Container for IDs assigned to an artifact
- `OpOutcome` (pub, type re-export) — Result type for operations (likely enum of success/failure states)
- `PatchApplied` (pub, type re-export) — Record of a successfully applied patch
- `PatchSummary` (pub, type re-export) — Summary representation of a patch
- `VersionData` (pub, type re-export) — Data associated with a document version

### Re-exported types from `error`
- `AssetError` (pub, error enum re-export) — Error type for asset-related failures
- `ConflictDetail` (pub, type re-export) — Details about document or patch conflicts
- `DocumentError` (pub, error enum re-export) — Primary domain error type for document operations
- `IndexError` (pub, error enum re-export) — Error type for artifact indexing failures
- `RenderError` (pub, error enum re-export) — Error type for rendering IR to formats
- `StorageError` (pub, error enum re-export) — Error type for persistent storage operations

### Re-exported types from `ids`
- `ArtifactId` (pub, type re-export) — Identifier for artifacts
- `ArtifactKind` (pub, enum re-export) — Classification of artifact type
- `AssetId` (pub, type re-export) — Identifier for assets
- `SessionId` (pub, type re-export) — Identifier for sessions
- `VersionId` (pub, type re-export) — Identifier for versions

### Re-exported types from `ir`
- Implicitly re-exported via `pub mod ir` (no items re-exported at this level; consumers use `ir::<Type>`)

### Re-exported types from `patch`
- `Patch` (pub, type re-export) — Container/description of a patch operation
- `PatchOp` (pub, type re-export) — Individual patch operation (likely add/remove/modify/reorder)
- `PatchSource` (pub, enum re-export) — Origin or classification of a patch

### Re-exported traits from `ports`
- `ArtifactStore` (pub, trait re-export) — Port for artifact storage/retrieval (database or file adapter)
- `AssetStore` (pub, trait re-export) — Port for asset storage/retrieval
- `AssetSummary` (pub, type re-export) — Summary metadata for an asset
- `IRRenderer` (pub, trait re-export) — Port for rendering intermediate representation to final format
- `IRValidator` (pub, trait re-export) — Port for validating intermediate representation
- `IdGenerator` (pub, trait re-export) — Port for generating unique identifiers
- `SessionArtifactIndex` (pub, trait re-export) — Port for indexing artifacts by session

## File-level notes

- **Pattern:** Classic Rust barrel/index file (module organization + re-exports)
- **Completeness:** All submodules declared and key types re-exported; no apparent omissions
- **Documentation:** No module-level doc comment (`//!`) present; would benefit from brief summary of documents domain purpose
- **Dependencies:** Imports from six internal submodules only (zero external dependencies); clean domain isolation
- **Architecture:** Pure domain layer (zero infrastructure dependencies); ports defined for boundaries; all traits are abstractions only
