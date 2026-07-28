# src/libs/colmena/src/documents/application/list_versions.rs

**Layer:** application  **Purpose:** Implements a use case for listing document versions with optional pagination. Fetches versions from an ArtifactStore, retrieves metadata for each, and returns ordered VersionEntry objects.

## Symbols

- `VersionEntry` (struct, pub) — Serializable value object representing a single version with id, timestamp, source, and summary
- `VersionEntry::version_id` (field, pub) — Identifier of the version
- `VersionEntry::applied_at` (field, pub) — Timestamp when the version was applied
- `VersionEntry::source` (field, pub) — String indicating the source/origin of the version (e.g., "agent")
- `VersionEntry::summary` (field, pub) — Vector of summary strings describing the version changes
- `ListVersionsUseCase` (struct, pub) — Use case handler that lists versions of a document
- `ListVersionsUseCase::store` (field, pub) — Arc-wrapped ArtifactStore trait dependency for fetching version data
- `ListVersionsUseCase::execute` (fn, pub async) — Main async use case method that returns paginated version list

## File-level notes

- Clean, focused implementation of a single use case; no coupling to infrastructure beyond the ArtifactStore trait port
- Reverses version list (`.rev()`) to return latest versions first, with optional limit for pagination
- Source extraction (lines 33–39) defaults to "agent" if not present in patch metadata; defensive but may mask missing data silently
- Error propagation via `?` is appropriate; DocumentError from domain is returned cleanly
- All symbols are used; no unused code or dead branches
- Serialization ready for REST API responses (derive Serialize on VersionEntry)
