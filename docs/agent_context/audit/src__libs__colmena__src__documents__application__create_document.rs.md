# src/libs/colmena/src/documents/application/create_document.rs

**Layer:** application  
**Purpose:** Use case that orchestrates document creation: generates artifact IDs, initializes IR (intermediate representation) templates, validates and renders them, and persists the result through the artifact store.

## Symbols

- `CreateDocumentInput` (pub struct) — Input DTO defining the artifact kind, session context, optional label/retention, initial IR, and patch source
- `CreateDocumentOutput` (pub struct) — Output DTO returning the created artifact ID, version ID, label, and artifact metadata
- `CreateDocumentUseCase` (pub struct) — Main orchestrator holding ports (ArtifactStore, validators/renderers per kind, ID generator) and default retention limit
- `CreateDocumentUseCase::execute` (pub async fn) — Primary business logic: generates artifact ID and version, injects them into IR, validates and renders, then persists via store; returns output DTO
- `default_label` (fn, private) — Generates default label as "Untitled {Kind} YYYY-MM-DD HH:MM" using current UTC time
- `empty_ir` (fn, private) — Constructs empty JSON IR template matching artifact kind (Excel/Word/Html) with structural fields and schema version
- `tests` (mod, private) — Unit tests with mock renderers/validators; covers empty Excel and HTML artifact creation paths

## File-level notes

1. **Tripled ArtifactKind matching (lines 64–69, 88–92, 116–120)**: The same three-way match on `ArtifactKind` appears in three contexts—choosing validator/renderer, selecting rendered file extension, and building label suffix. Extension and label_suffix matches could be factored into helper functions to reduce duplication and maintenance burden.

2. **Redundant artifact_id/version_id insertion in empty_ir case (lines 58–62)**: When `initial_ir` is None, `empty_ir()` already includes `artifact_id` and `version_id` fields, which are then overwritten with identical values at lines 60–61. For the Some case (user-provided IR), this insertion is correct and necessary; for the None case, it is unnecessary work. Could optimize by only inserting when `initial_ir.is_some()`.

3. **Silent if-let for object mutation (line 59)**: If IR is not a JSON object, the artifact_id/version_id injection silently skips. Defensive code that works correctly given the validators gate IR structure, but could benefit from a comment explaining the assumption.

4. **Test coverage**: Both tests use `initial_ir: None`, so the path where a user-provided IR is defensively overwritten (lines 60–61) is not exercised. Minor gap.
