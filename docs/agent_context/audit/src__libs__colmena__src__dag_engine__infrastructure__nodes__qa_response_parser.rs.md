# src/libs/colmena/src/dag_engine/infrastructure/nodes/qa_response_parser.rs

**Layer:** infrastructure  
**Purpose:** Shared parser for the canonical ID-keyed Q/A resume-answer format (`Q[id]: ...\nA[id]: ...`) used by all suspend-flavored nodes. Handles order-independent, multi-line-preserving parsing with strict ID validation and comprehensive error reporting.

## Symbols

- `QaParseError` (enum, pub) — Error type with 6 variants: `InvalidIdSyntax`, `UnknownId`, `DuplicateId`, `MissingId`, `EmptyAnswer`, `OrphanQuestion`
- `InvalidIdSyntax` (variant) — ID contains invalid characters or length
- `UnknownId` (variant) — Answer ID not in expected set
- `DuplicateId` (variant) — Answer ID appears more than once
- `MissingId` (variant) — No answer provided for an expected ID
- `EmptyAnswer` (variant) — Answer text is whitespace-only
- `OrphanQuestion` (variant) — Question exists but no corresponding answer
- `impl Display for QaParseError` — Human-readable error messages for all variants
- `impl Error for QaParseError` — Standard Error trait implementation
- `ID_MAX_LEN` (const) — Maximum ID length (64 bytes)
- `is_valid_id_char(c: char) -> bool` (fn, private) — Predicate: true if char in `[A-Za-z0-9_-]`
- `validate_id(id: &str) -> Result<(), QaParseError>` (fn, private) — Full ID validation: length, charset, returns `InvalidIdSyntax` on failure
- `is_valid_qa_id(id: &str) -> bool` (pub fn) — Public API: boolean check for canonical Q/A ID format
- `parse_prefix_at(answer: &str, offset: usize) -> Option<(char, String, usize)>` (fn, private) — Parse single `Q[id]:` or `A[id]:` at line-start offset; returns kind, id string, and byte position after colon
- `parse_qa_response(answer: &str, expected_ids: &[&str]) -> Result<HashMap<String, String>, QaParseError>` (pub fn) — Main parser: reads `answer` line-by-line, extracts Q/A pairs with multi-line body support, validates against expected IDs, returns map of id→answer text or error
- `tests::parses_single_id_pair` (test) — Verifies single Q/A pair
- `tests::parses_multiple_ids_in_declared_order` (test) — Verifies order-independence with declaration order
- `tests::parses_multiple_ids_in_reversed_order` (test) — Verifies order-independence with reversed order
- `tests::preserves_internal_newlines_in_answer` (test) — Verifies multi-line answer bodies are preserved intact
- `tests::tolerates_no_space_after_colon` (test) — Verifies optional space after `:`
- `tests::does_not_validate_question_text_matches` (test) — Verifies Q text is ignored
- `tests::errors_on_invalid_id_syntax` (test) — Verifies rejection of space in ID
- `tests::errors_on_unknown_id` (test) — Verifies rejection of ID not in expected set
- `tests::errors_on_duplicate_id` (test) — Verifies rejection of repeated A[id]
- `tests::errors_on_missing_id` (test) — Verifies all expected IDs must have answers
- `tests::errors_on_empty_answer` (test) — Verifies non-empty answer requirement
- `tests::errors_on_orphan_q_without_a` (test) — Verifies Q without matching A is rejected
- `tests::validate_id_accepts_valid_chars` (test) — Verifies valid ID charset
- `tests::validate_id_rejects_invalid` (test) — Verifies rejection of invalid IDs
- `tests::is_valid_qa_id_accepts_valid` (test) — Verifies public API on valid IDs
- `tests::is_valid_qa_id_rejects_invalid` (test) — Verifies public API on invalid IDs
- `tests::parse_prefix_at_finds_q_prefix` (test) — Verifies Q[id]: prefix detection
- `tests::parse_prefix_at_finds_a_prefix` (test) — Verifies A[id]: prefix detection
- `tests::parse_prefix_at_returns_none_for_non_prefix` (test) — Verifies non-prefix rejection
- `tests::parse_prefix_at_aborts_on_newline_in_brackets` (test) — Verifies safety: newline in ID terminates bracket scan

## File-level notes

- **Comprehensive test coverage**: 21 tests covering normal paths, error cases, boundary conditions (empty IDs, max-length IDs, newlines in bodies, etc.), and edge cases (no space after colon, orphan questions).
- **Clear domain separation**: Private helpers (`is_valid_id_char`, `validate_id`, `parse_prefix_at`) support public API (`is_valid_qa_id`, `parse_qa_response`).
- **Robust line-based parsing**: Correctly handles multi-line answer bodies by scanning forward to next `Q[` or `A[` prefix; preserves internal newlines while trimming trailing newlines.
- **Strict validation**: ID charset restricted to `[A-Za-z0-9_-]{1,64}`; empty answers rejected; all expected IDs must be answered; orphan questions (Q without A) detected.
- **No external dependencies**: Pure stdlib (HashMap, HashSet, fmt, std::error::Error).
