# src/libs/colmena/src/dag_engine/application/sql_execution_service.rs

**Layer:** application  **Purpose:** Orchestrates the SQL execution pipeline (validate → critic → execute → feedback). Implements the application-layer use case depending only on domain ports (traits), not infrastructure adapters.

## Symbols

- `SqlExecutionService` (struct, pub) — Orchestrates full SQL execution pipeline; holds connection, validator, critic (optional), and registry ports as Arc-wrapped trait objects
- `SqlExecutionResult` (struct, pub) — Complete result envelope for SQL execution: output JSON, row count, truncation flag, validation warnings, and optimization hints
- `SqlExecutionResult::to_json` (impl, pub fn) — Serializes result to JSON format returned to the LLM, conditionally including warnings and hints if non-empty
- `SqlExecutionService::new` (impl, pub fn) — Constructor accepting connection, validator, optional critic, and registry ports; stores all as Arc-wrapped dependencies
- `SqlExecutionService::execute` (impl, pub async fn) — Main orchestration method: Stage 1 (static validation) → Stage 2 (optional LLM critic) → Stage 3 (execute query) → Stage 4 (feedback recording); returns SqlExecutionResult or SqlNodeError; also registers created functions after execution
- `extract_comment_from_stmts` (fn, private) — Pulls comment text from first `COMMENT ON ... IS '<text>'` statement using sqlparser AST (avoids quote-escaping bugs from regex)
- `ast_extract_tests` (mod, test) — Test module verifying comment extraction handles apostrophes correctly in comment bodies

## File-level notes

- Static validation and LLM critic failures are recorded as feedback via `registry.record_feedback()` but exceptions are intentionally silenced (`let _ =`) — feedback recording failures do not block the query result.
- Lines 140–141: Duplicate parsing acknowledged as acceptable for now; comment notes potential future optimization via shared parse cache (no action required, design is intentional).
- Function registration after execution (lines 142–158) captures created functions' names and schemas; parameters and return types are intentionally set to `None` pending a later enhancement phase.
- All four domain ports (`SqlConnectionPort`, `SqlValidatorPort`, `SqlCriticPort`, `FunctionRegistryPort`) are injected at construction, enabling full testability with mocks.
