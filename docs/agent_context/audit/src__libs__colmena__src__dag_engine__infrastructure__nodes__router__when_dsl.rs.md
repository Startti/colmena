# src/libs/colmena/src/dag_engine/infrastructure/nodes/router/when_dsl.rs

**Layer:** infrastructure  
**Purpose:** Implements a "when" DSL parser and evaluator for conditional routing in the router node. Parses JSON rule definitions and evaluates them against extracted data to determine conditional flow.

## Symbols

- `WhenRule` (enum, pub) — AST representation of a when condition; variants for equals, not_equals, in, contains, gt, lt, gte, lte, matches, exists, and logical combinators (all, any, not)
- `WhenRule::parse` (fn, pub) — Parses a JSON value into a WhenRule; validates field references against an inline schema; handles nested all/any/not combinators and single-field operators
- `WhenRule::evaluate` (fn, pub) — Evaluates a parsed WhenRule against extracted JSON data; returns bool indicating rule satisfaction
- `resolve` (fn, private) — Helper to resolve dotted-path field names (e.g., "user.tier") in a JSON object tree; returns Option<Value>
- `tests::schema` (fn, test helper) — Creates a sample JSON schema for test cases
- `tests::parse` (fn, test helper) — Wrapper around WhenRule::parse with schema context for tests
- `tests::equals_string` (test) — Verifies string equality operator matching and non-matching cases
- `tests::equals_is_type_strict` (test) — Verifies equals rejects type mismatches (5 ≠ "5")
- `tests::not_equals` (test) — Verifies not_equals operator and missing-field handling (missing = not equal)
- `tests::in_operator` (test) — Verifies membership in a set of values
- `tests::contains_string_substring` (test) — Verifies substring search in string values
- `tests::gt_lt_gte_lte` (test) — Verifies numeric comparison operators
- `tests::matches_regex` (test) — Verifies regex pattern matching against string fields
- `tests::exists_true` (test) — Verifies field existence check (non-null values only)
- `tests::all_combinator` (test) — Verifies logical AND over multiple rules
- `tests::any_combinator` (test) — Verifies logical OR over multiple rules
- `tests::not_combinator` (test) — Verifies logical NOT inversion
- `tests::dotted_field_path` (test) — Verifies nested field resolution via dot notation (e.g., "user.tier")
- `tests::rejects_unknown_field_at_parse_time` (test) — Verifies parse-time validation against schema
- `tests::rejects_when_with_no_operator` (test) — Verifies error when rule has field but no operator
- `tests::rejects_invalid_regex` (test) — Verifies error on malformed regex patterns
- `tests::nested_combinators` (test) — Verifies complex nested combinations of all/any

## File-level notes

- No flags: code is well-structured and complete with comprehensive test coverage.
- All operators have clear error messages and type validation.
- The `resolve` function handles dotted-path traversal safely, returning None on any break in the chain.
- The design intentionally rejects `exists: false` (lines 128–133); users must use `not: { ..., exists: true }` instead, which is enforced with a clear error message.
- Parser validates top-level field names against inline_schema early (lines 60–66), catching schema mismatches at parse time.
- All 13 test cases provide good coverage of single operators, combinators, edge cases (missing fields, type strictness), and error paths.
