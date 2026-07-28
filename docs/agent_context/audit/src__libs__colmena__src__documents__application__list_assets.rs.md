# src/libs/colmena/src/documents/application/list_assets.rs

**Layer:** application  
**Purpose:** Thin wrapper use case that delegates session-scoped asset listing to the AssetStore port abstraction.

## Symbols

- `ListAssetsUseCase` (struct, pub) — Container holding an Arc to AssetStore; provides execute method for listing session assets. [FLAG: dead_candidate — pass-through wrapper with no added logic; check external callers via module dependency map]
- `ListAssetsUseCase::store` (field, pub) — Arc reference to the AssetStore port implementation.
- `ListAssetsUseCase::execute` (fn, async, pub) — Delegates to `store.list_by_session(session_id)` with zero transformation.
- `tests` (mod, cfg-test) — Inline test module.
- `tests::returns_summaries_for_session` (fn, async, test) — Verifies asset upload and list round-trip using LocalFsAssetStore.

## File-level notes

- Module comment explicitly labels this as a "thin wrapper," suggesting intentionality, but the structure lacks any business logic, validation, or enrichment beyond delegation.
- Only internal user is its own test; no other callers visible in this file. External call sites should be verified via `docs/agent_context/module_dependency_map.md` to determine if this use case is truly used or if callers access `AssetStore` directly.
- Test is straightforward and covers the happy path (single upload per session).
