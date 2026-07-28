# src/libs/colmena/src/dag_engine/infrastructure/node_schema_merge.rs

**Layer:** infrastructure  **Purpose:** Pure merge of caller-supplied arguments (LLM tool args or `for_each` row data) into a `node_schema`, extracted from `DagToolExecutor::execute_inner` to share identical merge-and-resolve semantics between tool calling and batch execution.

## Symbols

- `merge_args_into_schema` (fn, pub(crate)) — Merges LLM/row arguments into a parsed node_schema: seeds fixed values, places each arg via param_to_container (nested or top-level), rejects fixed-field overrides, and resolves ${VAR} templates; returns merged result or error. [FLAG: improvement — collision warning printed to stderr instead of collected and returned to caller]

- `tests` (mod, cfg(test)) — Test module for merge logic.

- `places_fixed_and_row_args` (test) — Verifies fixed fields and row-supplied arguments are both placed in output.

- `row_arg_cannot_override_fixed` (test) — Confirms row args attempting to override fixed fields are silently ignored and the fixed value is retained.

## File-level notes

- The function is a direct extraction from `DagToolExecutor::execute_inner` (PATH 0 per comment), reused by `for_each` to apply identical merge semantics to batch rows. No architectural coupling issues.
- Line 33–36: Dot-notation splitting (e.g., `"config.key"` → `config`/`key`) is used to place nested parameters. Intentional design, working as intended.
- Line 52–54: Collision warning (fixed-field override attempt) is logged via `eprintln!()` and the conflicting arg is discarded. This is a side effect in otherwise pure code; better practice would return warnings alongside the result or use structured logging.
- Extensive `.clone()` usage (lines 24, 43–44, 47, 64–65) reflects the complexity of merging `Value::Object` trees, not a code smell given serde_json constraints.
- No `todo!()`, `unimplemented!()`, or `unreachable!()` stubs present. No dead or unfinished code.
