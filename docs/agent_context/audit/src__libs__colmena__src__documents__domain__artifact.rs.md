# src/libs/colmena/src/documents/domain/artifact.rs

**Layer:** domain  
**Purpose:** Defines value objects for artifact metadata, versioning, patch application, and operational outcomes. Models the core domain entities for CRDT document artifacts with retention policies, version tracking, and structured change summaries.

## Symbols

- `ArtifactMeta` (struct, pub) — Metadata envelope for an artifact: ID, kind, creation/update timestamps, current version, retention limit, pinning flag, schema version, session ID, label, and optional tags.
- `ArtifactMeta::initial` (method, pub) — Constructor creating a new ArtifactMeta with current timestamp, initial version ID, pin_initial=true, empty tags, and caller-supplied ID/kind/session/label/retention.
- `PatchApplied` (struct, pub) — Record of a patch application: the patch JSON value, timestamp, resulting version ID, and a PatchSummary describing changes.
- `PatchSummary` (struct, pub) — Change summary for a patch: optional natural-language descriptions (Vec<String>) and optional structured summaries (Vec<serde_json::Value>).
- `OpOutcome` (struct, pub) — Wrapper struct containing AssignedIds; used to report the result of an operation (which IDs were created/modified).
- `AssignedIds` (struct, pub) — Catalog of entity IDs created/assigned during an operation: optional block, runs, list_items, rows, table, sheet, and slide IDs.
- `AssignedIds::is_empty` (method, pub) — Predicate returning true if all fields (block, runs, list_items, rows, table, sheet, slide) are empty/None; used as serde skip_serializing_if gate.
- `VersionData` (struct, pub) — Full version payload: IR as serde_json::Value, rendered binary bytes, rendered file extension, applied patch metadata, and associated blobs (vector of filename/bytes tuples).  [FLAG: dead_candidate — defined but no in-file usage; likely used in application/infrastructure layers]
- `ArtifactSummary` (struct, pub) — Lightweight summary for artifact listing/retrieval: ID, session ID, kind, optional label, current version, and updated-at timestamp.
- `tests::initial_meta_sets_v1` (test, private) — Verifies ArtifactMeta::initial correctly sets current_version to initial, pin_initial to true, and retention_limit to the supplied value.
- `tests::meta_roundtrip_json` (test, private) — Verifies ArtifactMeta round-trips through JSON serialization/deserialization with kind preserved.

## File-level notes

- All top-level structs are Serialize/Deserialize for persistence/transport.
- `VersionData` is the only non-derived struct (fields are public but no impl methods), suggesting it may be a passive data carrier.
- `PatchSummary` and `OpOutcome` are Default-able; `AssignedIds` implements the is_empty predicate for serde conditional serialization.
- Imports IDs and constants from sibling modules (ids, ir); no external infrastructure dependencies.
- Test coverage is minimal (2 tests) but focuses on correctness of construction and serialization.
- No error handling, validation, or boundary logic in this layer—appropriate for a domain value-object file.
