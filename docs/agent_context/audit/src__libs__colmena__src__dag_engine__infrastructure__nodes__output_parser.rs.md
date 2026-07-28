# src/libs/colmena/src/dag_engine/infrastructure/nodes/output_parser.rs

**Layer:** infrastructure  **Purpose:** Implements OutputParserNode, a DAG-executable wrapper that parses unstructured text into structured JSON matching an inline schema, delegating to the extraction engine.

## Symbols

- `DEFAULT_SYSTEM_MSG` (const) — Build-time-loaded system prompt for extraction from markdown file
- `OutputParserNode` (struct, public) — Zero-sized marker struct implementing ExecutableNode trait
- `OutputParserNode::resolve_env_var` (fn, private) — Resolves environment variable references in ${VAR_NAME} format or passes through literal strings
- `OutputParserNode::is_empty_input` (fn, private) — Checks if a JSON value is empty (null, empty string, empty array, or empty object)
- `ExecutableNode::execute` (async fn, trait impl) — Main execution: validates provider/api_key/model/schema/instructions config, builds LLM system message, invokes extraction engine with templated prompt
- `ExecutableNode::default_input` (fn, trait impl) — Returns "input" as the default input port name
- `ExecutableNode::description` (fn, trait impl) — Returns user-facing description of the node's purpose
- `ExecutableNode::schema` (fn, trait impl) — Returns hardcoded JSON schema documenting config fields, input port, and output schema
- `tests::make_inputs` (fn, private) — Helper to construct NodeInputs test fixture
- `tests::fails_when_input_is_null` (test) — Verifies error when input is null
- `tests::fails_when_input_is_empty_string` (test) — Verifies error when input is whitespace-only string
- `tests::fails_when_input_is_empty_array` (test) — Verifies error when input is empty array
- `tests::fails_when_input_is_empty_object` (test) — Verifies error when input is empty object
- `tests::fails_when_schema_is_invalid_inline` (test) — Verifies error when schema contains invalid type field

## File-level notes

- Well-structured infrastructure node with clear separation of config validation, input validation, and delegation to extraction engine.
- Test coverage focuses on negative cases (input validation, schema validation); positive/happy-path tests not visible in this file (likely covered at integration level).
- Temperature conversion at line 99–100 casts f64 to f32; type is correct for model parameters.
- All error paths use `?` propagation or explicit `ok_or`, ensuring no silent failures.
- Schema method returns hardcoded documentation; this is stable and acceptable for a stable node interface.
