# src/libs/colmena/src/documents/application/read_document.rs

**Layer:** application  
**Purpose:** Implements the `ReadDocumentUseCase` to fetch documents/artifacts from the store with optional version selection, extracting metadata into a typed output DTO.

## Symbols

- `ReadDocumentInput` (struct, pub) — Input DTO holding artifact_id and optional version for document retrieval
- `artifact_id` (field, pub) — The artifact identifier to read
- `version` (field, pub) — Optional specific version ID to retrieve; if None, reads current
- `ReadDocumentOutput` (struct, pub) — Output DTO containing the document IR as JSON and its version ID
- `ir` (field, pub) — The document intermediate representation as serde_json::Value
- `version` (field, pub) — The version ID of the returned document
- `ReadDocumentUseCase` (struct, pub) — Use case orchestrator wrapping the ArtifactStore port dependency
- `store` (field, pub) — Arc-wrapped trait object for the ArtifactStore port (hexagonal pattern)
- `execute` (async fn, pub) — Executes the read workflow: branches on version presence, reads from store, extracts version_id field into VersionId, returns typed output [FLAG: improvement — version_id extraction silently defaults to empty string if missing; no validation]

## File-level notes

- **Silent version_id defaulting**: Line 33 calls `unwrap_or_default()` when the `version_id` field is missing from the JSON IR returned by the store. This creates a `VersionId` with an empty string, which may mask data issues (malformed store response or incomplete document). Either the store contract should guarantee version_id presence, or this should return a structured `DocumentError` if the field is absent.
- No error handling at the version extraction boundary; the flow assumes the JSON structure from the store is always well-formed.
- Clean hexagonal pattern: `execute()` method depends on the `ArtifactStore` trait, not a concrete implementation.

