# src/libs/colmena/src/dag_engine/infrastructure/nodes/util/extract_with_schema.rs

**Layer:** infrastructure  **Purpose:** Utility for calling an LLM to extract structured JSON output with schema validation, markdown code-fence stripping, and integration with the agent service.

## Symbols

- `ExtractInput<'a>` (struct, pub) — Holds LLM provider configuration, credentials, model, system/user messages, inline schema, temperature, and observer for a structured extraction call
- `extract_with_schema` (fn, pub, async) — Executes a single-turn LLM call with system+user messages, strips markdown code fences from response, parses JSON, validates against schema, and returns parsed result
- `EmptyToolExecutor` (struct, private) — Stub struct satisfying `ToolExecutor` trait; used to fill `AgentRunParams` when no tools are available
- `EmptyToolExecutor::execute` (method, private, async) — Returns `ToolExecutionFailed` error (no tools available)
- `EmptyToolExecutor::available_tools` (method, private, async) — Returns empty tools vector
- `parse_and_validate` (fn, pub) — Strips markdown code fences (`\`\`\`json`, `\`\`\``), parses JSON string, and validates parsed object against inline schema; skips validation if schema is empty object
- `tests::parse_and_validate_strips_json_fence` (test, private) — Verifies markdown JSON fence removal
- `tests::parse_and_validate_strips_plain_fence` (test, private) — Verifies plain markdown fence removal
- `tests::parse_and_validate_accepts_unwrapped_json` (test, private) — Verifies acceptance of JSON without code fences
- `tests::parse_and_validate_fails_on_invalid_json` (test, private) — Verifies error on malformed JSON
- `tests::parse_and_validate_fails_on_schema_mismatch` (test, private) — Verifies schema type validation catches mismatches
- `tests::parse_and_validate_fails_on_missing_required_field` (test, private) — Verifies schema required-field validation
- `tests::parse_and_validate_skips_validation_for_empty_schema` (test, private) — Verifies empty-schema bypass allows any JSON

## File-level notes

- Clean error propagation via `?` operator; all fallible operations handled
- `EmptyToolExecutor` is a minimal stub satisfying the API contract; max_turns=1 ensures it is never invoked
- `parse_and_validate` skips schema validation when schema is empty object (`{}`), preserving legacy behavior for callers like `extraction.rs`
- All six test cases cover happy path, markdown variants, JSON parse errors, schema validation errors, and empty-schema bypass
- Uses in-memory conversation repository (ephemeral) and UUID-generated session/node IDs for isolation
- Observer integration reports LLM usage (prompt/completion/thinking/cache tokens) if observer provided
