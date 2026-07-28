# src/libs/colmena/src/documents/application/rollback.rs

**Layer:** application  **Purpose:** Implements the rollback use case for documents/artifacts. Allows reversal to a previous version by creating a new version with the same content as the target and recording the rollback in patch history.

## Symbols

- `RollbackInput` (struct, pub) — Input DTO specifying which artifact and version to roll back to
- `RollbackInput::artifact_id` (field, pub) — The artifact being rolled back
- `RollbackInput::to_version` (field, pub) — The target version to restore
- `RollbackOutput` (struct, pub) — Output DTO containing new and source version IDs (derives Debug)
- `RollbackOutput::new_version_id` (field, pub) — The newly created version after rollback
- `RollbackOutput::copied_from` (field, pub) — The original version that was cloned
- `RollbackUseCase` (struct, pub) — Use-case orchestrator coordinating the rollback workflow
- `RollbackUseCase::store` (field, pub) — Arc-wrapped ArtifactStore trait dependency
- `RollbackUseCase::execute` (async method, pub) — Entry point: reads metadata and target version, increments version ID, clones IR with updated version_id field, records PatchApplied with rollback operation, writes new VersionData to store, updates head pointer with CAS, updates metadata, returns new version IDs

## File-level notes

- Clean application-layer use case with proper dependency injection and error propagation
- All async store operations use `?` error handling consistently
- Compare-and-set on `set_head` correctly passes previous version for optimistic concurrency
- DTO structs are simple and well-scoped
- No unused code or incomplete stubs
