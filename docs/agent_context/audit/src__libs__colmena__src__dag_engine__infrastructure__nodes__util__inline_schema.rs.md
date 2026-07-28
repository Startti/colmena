# src/libs/colmena/src/dag_engine/infrastructure/nodes/util/inline_schema.rs

**Layer:** infrastructure  **Purpose:** Converts between compact inline JSON schema format (field-name-keyed) and standard JSON Schema, and validates values against inline schema definitions.

## Symbols

- `inline_to_json_schema` (pub fn) — converts inline-required schema format `{ field_name: { type, required?, description? } }` to standard JSON Schema `{ type: "object", properties: {...}, required: [...] }`
- `validate_against_inline_schema` (pub fn) — validates a JSON object against an inline schema, checking required fields presence and type correctness
- `type_label` (fn) — returns human-readable type name ("null", "boolean", "number", "string", "array", "object") for a JSON value
- `tests` (mod) — test module with 10 comprehensive tests covering conversion and validation paths

## File-level notes

- Both public functions have clear error messages and reject invalid inputs (non-objects, empty schemas, missing/invalid type fields)
- Supported types: "string", "number", "integer", "boolean", "array", "object" — validated in both directions
- Null handling: required fields reject null; optional fields accept null (lines 84, 92–93)
- Integer validation (line 102) uses `is_i64() || is_u64()` to handle both signed and unsigned ranges
- Tests verify single/multiple fields, required/optional distinction, description preservation, type mismatch errors, and null/missing field handling
- No TODOs, unimplemented calls, or dead code
