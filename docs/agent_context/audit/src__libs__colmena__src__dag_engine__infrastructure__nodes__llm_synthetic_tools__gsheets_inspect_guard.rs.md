# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_inspect_guard.rs

**Layer:** infrastructure  **Purpose:** Pure helper functions for the gsheets "inspect-before-python" guard. Provides deduplication logic, sheet binding filtering, and markdown preview truncation to force agents to see sheet structure before code execution.

## Symbols

- `sheet_key` (pub fn) — Creates a normalized deduplication key combining spreadsheet_id and lowercased sheet name (handles A1 quoting and case variance)
- `normalize_sheet_name` (fn, private) — Normalizes sheet names by trimming, stripping one layer of A1 quotes (single or double), and lowercasing for comparison
- `SheetBindingRef` (pub struct) — Represents a sheet binding reference with `var`, `spreadsheet_id`, `sheet`, and optional `range` fields
- `unseen_sheet_bindings` (pub fn) — Extracts sheet bindings from `gsheets_run_python` args that are not yet in the seen set; skips inline bindings (with `data`) and incomplete bindings
- `truncate_markdown_preview` (pub fn) — Truncates a markdown table to header + separator + `max_data_rows` data rows (returns unchanged if ≤2 lines)
- `columns_from_markdown_header` (pub fn) — Parses column names from a markdown table's first header line (splits by `|`, trims, filters empty)
- `tests` (mod) — Test module with 8 test cases covering sheet key normalization, binding filtering, markdown truncation, and column extraction

## File-level notes

- Well-scoped pure-function module with no I/O or statefulness; separation of concerns explicitly noted in module doc comment
- Comprehensive test coverage: all public functions and key edge cases tested (case/quote variants, mixed bindings, short tables, empty input)
- Error handling is defensive (graceful fallback for missing JSON keys via `.and_then()`, `.unwrap_or()`, pattern matching on Option)
- No external crate dependencies beyond `serde_json` (already present in project); minimal stdlib imports
- Module comment references design spec at `docs/superpowers/specs/2026-06-15-gsheets-inspect-guard-design.md`
- No dead code, no unfinished implementations, no todo!/unimplemented! stubs
