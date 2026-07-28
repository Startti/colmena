# src/libs/colmena/src/documents/application/delete_asset.rs

**Layer:** application  **Purpose:** Implements `DeleteAssetUseCase` for safe asset deletion with reference checking; prevents deletion if asset is still referenced by any artifact in the session.

## Symbols

- `DeleteAssetInput` (struct, pub) — Input value object with session_id and asset_id for deletion request
  - `session_id` (field) — Session identifier
  - `asset_id` (field) — Asset identifier to delete
- `DeleteAssetUseCase` (struct, pub) — Use case orchestrator that checks artifact references before deleting asset
  - `assets` (field) — Arc-wrapped AssetStore port dependency
  - `artifacts` (field) — Arc-wrapped ArtifactStore port dependency  
  - `index` (field) — Optional Arc-wrapped SessionArtifactIndex port for reference checking
  - `execute` (fn, pub async) — Main entry point that queries index for all session artifacts, checks ir.assets_referenced array for blocking references, returns StillReferenced error or deletes asset
- `StubArtifactStore` (struct) — Minimal in-memory test mock of ArtifactStore holding ir data by string key
  - `with` (fn) — Constructor populating store with single artifact ir and id
  - `ArtifactStore impl block` — Trait implementation with mostly no-op/stub methods
    - `create_artifact` (async fn) — Returns Ok(())
    - `write_version` (async fn) — Returns Ok(())
    - `read_current` (async fn) — Retrieves ir from HashMap by id, wraps in VersionData with stub rendered/patch fields
    - `read_version` (async fn) — Unimplemented stub [FLAG: unfinished — unimplemented!() in test mock; acceptable for stub but note usage pattern]
    - `list_versions` (async fn) — Returns empty vec
    - `set_head` (async fn) — Returns Ok(())
    - `delete_version` (async fn) — Returns Ok(())
    - `read_meta` (async fn) — Unimplemented stub [FLAG: unfinished — unimplemented!() in test mock; acceptable for stub]
    - `update_meta` (async fn) — Returns Ok(())
    - `delete_artifact` (async fn) — Returns Ok(())
- `StubIndex` (struct) — Minimal in-memory test mock of SessionArtifactIndex holding Vec of ArtifactSummary
  - `SessionArtifactIndex impl block` — Trait implementation
    - `register` (async fn) — Returns Ok(())
    - `list_by_session` (async fn) — Returns cloned summaries from stored vec
    - `lookup` (async fn) — Returns Ok(None)
    - `update_head` (async fn) — Returns Ok(())
    - `unregister` (async fn) — Returns Ok(())
- `summary` (fn) — Helper test utility creating ArtifactSummary with given id, hardcoded session s1, kind Html, initial version
- `delete_blocked_when_referenced` (test, async) — Verifies StillReferenced error when artifact has asset in assets_referenced array
- `delete_succeeds_when_not_referenced` (test, async) — Verifies deletion succeeds when artifact assets_referenced is empty
- `delete_skips_check_when_no_index` (test, async) — Verifies deletion proceeds without checking when index is None, even if artifact would block

## File-level notes

- Follows hexagonal architecture: use case depends on three ports (AssetStore, ArtifactStore, SessionArtifactIndex) with no direct infrastructure coupling.
- Reference checking logic (lines 34–41): iterates artifact ir.assets_referenced array and collects blocking artifact IDs; semantics rely on string equality with asset_id.
- Unimplemented trait methods in test stubs are acceptable; those methods aren't exercised by the test suite.
- Error handling is explicit: storage errors wrapped with context strings; reference check returns structured StillReferenced error with blocking artifact list.
- Test coverage is comprehensive: three scenarios cover blocked deletion, successful deletion, and graceful degradation when index is unavailable.
