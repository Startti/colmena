# src/libs/colmena/src/documents/application/apply_patch.rs

**Layer:** application  
**Purpose:** Implements the ApplyPatchUseCase application service, which orchestrates patch application to document artifacts (Excel, Word, HTML), validating, rendering, and persisting new versions after operation execution.

## Symbols

- `ApplyPatchInput` (struct, public) — Input wrapper containing a Patch to apply
- `ApplyPatchOutput` (struct, public) — Output of apply patch: new version ID and summary
- `ApplyPatchUseCase` (struct, public) — Application service holding artifact store, renderers, validators, and ID generator for patch application
- `ApplyPatchUseCase::execute` (method, public async) — Main entry point: reads artifact, applies ops, validates, renders, persists new version across Excel/Word/HTML paths
- `op_outcome_entry` (fn, private) — Builds JSON object summarizing one operation (op_index, op tag, assigned_ids)
- `describe_op` (fn, private) — Large match on all PatchOp variants generating human-readable summaries for Excel, Word, and HTML operations
- `fmt_value` (fn, private) — Formats a JSON value for display, truncating long strings
- `truncate` (fn, private) — Truncates a string to max chars, appending "..."
- `tests::NoopR` (struct, test) — Mock IRRenderer returning empty bytes
- `tests::NoopR::render` (method, test async) — Mock render returning empty vec
- `tests::NoopR::target_extension` (method, test) — Returns "docx"
- `tests::NoopR::target_mime` (method, test) — Returns "x"
- `tests::NoopV` (struct, test) — Mock IRValidator
- `tests::NoopV::validate` (method, test) — Mock validation returning Ok
- `tests::apply_set_cell_creates_v2` (test) — Verifies Excel SetCell op advances version v1 → v2
- `tests::apply_with_stale_base_returns_conflict` (test) — Verifies VersionConflict error on base version mismatch
- `tests::apply_set_theme_on_html_advances_version` (test) — Verifies HTML SetTheme op advances version v1 → v2

## File-level notes

**Improvement flags:**

1. **Orchestration duplication (lines 51–237)** — Three near-identical workflows for Excel, Word, and HTML: parse IR → create applier → loop ops → validate → render → persist. All three paths could be refactored into a single polymorphic orchestrator (trait + per-type implementations or factory pattern) to reduce repetition and ease future artifact types.

2. **Large describe_op match (lines 258–553)** — 295-line match statement covering all Excel, Word, and HTML operation variants. Structurally repetitive (similar pattern per op: extract fields → format string → append optional IDs). Maintainable as-is, but a candidate for modularization (separate modules per artifact type, or trait-based dispatch) if operations grow further or if operation description logic needs to diverge.

3. **Unwrap on serde_json::to_value (lines 77, 82, 141, 146, 204, 209)** — Serialization of known types unwrapped without comment. Safe in practice (serialization failure on struct types is rare if Serialize is correct), but lacks documentation. Consider either a comment explaining safety, or defensive error handling (map_err to DocumentError).

4. **Op_outcome_entry fallibility (lines 247–249)** — Chains `.ok()` and `.and_then()` to extract op tag, defaulting to empty string silently. No error surfacing; operation type loss is silent. Not a bug (serialization failure is rare), but worth a comment explaining intent.

**No dead code or unfinished stubs detected.** All symbols are reachable; no TODO/FIXME comments; test coverage covers happy path (SetCell, stale base conflict) and HTML theme. Tests use noop mocks appropriately.
