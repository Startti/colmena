# src/libs/colmena/src/dag_engine/infrastructure/nodes/router/extract_and_route.rs

**Layer:** infrastructure  
**Purpose:** Implements Mode B router logic—the LLM extracts a JSON object against a user-provided schema; declarative `when` rules evaluate branches in declaration order to pick the first match.

## Symbols

- `EXTRACTION_SYSTEM_MSG` (const, private) — loads the extraction system prompt from embedded text file (`text/prompts/extraction_system.md`)
- `pick_branch` (async fn, pub) — main entry point: orchestrates LLM-based JSON extraction, iterates branches to find first `when` rule match, returns `(branch_index, extracted_json)` or error with extracted payload for diagnostics

## File-level notes

- Delegates schema extraction to `extract_with_schema` utility (good separation of concerns).
- Error at line 30 assumes config validation has already run; if `inline_schema` is missing, the diagnostic is opaque and unhelpful for debugging schema configuration issues.
- Error at line 62-65 uses ad-hoc string formatting (`format!` → boxed error) rather than a proper error type; consistent with other infrastructure code but less structured than the codebase's error strategy elsewhere.
- No `todo!()`, `unimplemented!()`, or unfinished stubs.
- Logic is correct: branches loop correctly guards against missing `when` rules with `if let Some`, and returns first match as expected.
