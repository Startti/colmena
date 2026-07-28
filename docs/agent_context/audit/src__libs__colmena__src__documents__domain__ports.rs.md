# src/libs/colmena/src/documents/domain/ports.rs

**Layer:** domain  **Purpose:** Defines port interfaces (traits and value objects) for artifact storage, asset management, document rendering, validation, and ID generation — hexagonal architecture boundary between domain logic and infrastructure adapters.

## Symbols

- `ArtifactStore` (trait, pub) — async port for CRUD + versioning of artifacts; manages creation, version read/write/delete, head pointer moves, and metadata updates
- `IRRenderer` (trait, pub, async) — async port for rendering JSON intermediate representation to binary with static target MIME type and file extension
- `IRValidator` (trait, pub) — sync port for validating JSON intermediate representation against domain rules
- `IdGenerator` (trait, pub) — sync port for generating unique string IDs across 9 entity types (artifacts, sheets, tables, blocks, runs, rows, list items, slides, assets)
- `SessionArtifactIndex` (trait, pub, async) — async port for session ↔ artifact ID mapping with artifact summaries; enforces session isolation per spec §9
- `AssetSummary` (struct, pub) — data class holding asset metadata: id, session_id, MIME type, size_bytes, optional label, created_at timestamp
- `AssetStore` (trait, pub, async) — async port for binary asset CRUD: upload (with session, MIME, optional label), read, list by session, delete, and head (metadata)
- `asset_store_trait_is_dyn_compatible` (test, private) — compile-time check that AssetStore trait object can be instantiated
- `asset_summary_has_required_fields` (test, private) — smoke test constructing AssetSummary and verifying field access

## File-level notes

- All port traits use `#[async_trait]` and `Send + Sync` bounds — standard for async infrastructure adapters.
- Minimal inline documentation; one reference to spec §9 for SessionArtifactIndex (session isolation design).
- Test coverage is thin (only 2 tests, both minimal) but appropriate for a pure port/type-definition file — the real tests are in adapter implementations and integration layers.
- Imports are clean and necessary; no unused dependencies.
- No dead code, unfinished stubs, or duplication detected.
