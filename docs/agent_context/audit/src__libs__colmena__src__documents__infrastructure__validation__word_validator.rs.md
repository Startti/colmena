# src/libs/colmena/src/documents/infrastructure/validation/word_validator.rs

**Layer:** infrastructure  
**Purpose:** Validates Word document IR (intermediate representation) structures, checking for duplicate block/run/item/row IDs and validating heading levels (1–6).

## Symbols

- `WordValidator` (struct, pub) — Unit struct implementing the `IRValidator` trait for Word IR validation
- `IRValidator::validate` (impl method, pub) — Entry point: deserializes JSON to WordIR, checks block-ID uniqueness, then validates each block via recursive helpers
- `validate_block` (fn, private) — Pattern-matches block type (Heading/Paragraph/List/Table) and delegates validation of nested structures (heading levels, run IDs, list items, table rows)
- `check_run_ids` (fn, private) — Helper that ensures no duplicate run IDs within a given scope; used by heading/paragraph/list-item/table-cell validators
- `tests::empty_word_is_valid` (test, private) — Verifies an empty Word IR passes validation
- `tests::duplicate_block_ids_fail` (test, private) — Verifies duplicate block IDs are rejected with an error
- `tests::heading_level_out_of_range_fails` (test, private) — Verifies heading levels outside 1–6 are rejected
- `tests::same_run_id_in_different_blocks_ok` (test, private) — Verifies that run IDs may be reused across different blocks (scope is per-block)

## File-level notes

- Code is complete and well-tested; no unfinished work (no `todo!()`, `unimplemented!()`, stubs, or FIXMEs).
- All functions are used; no dead code.
- Error paths include clear path and reason fields for debugging.
- Duplicate-checking pattern (HashSet insert-then-test) appears three times (block IDs, list items, table rows) but is idiomatic and context-specific; extraction would not materially improve clarity.
- Test coverage includes happy path and error cases.
