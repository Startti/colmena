# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/diff_writer.rs

**Layer:** infrastructure  **Purpose:** Pure diff algorithm for computing cell-level changes between sheet/CRDT states; validates key uniqueness, column schema, and row identity; produces structured error or change list for dispatcher translation.

## Symbols

- `MAX_DUP_EXAMPLES` (const, private) — Cap on duplicate-key examples surfaced to LLM for debugging (default 3)
- `CellChange` (struct, pub) — Single cell change: key_value, column name, old/new values
- `DiffResult` (struct, pub) — Successful diff: changes list, rows_changed/unchanged/skipped counts, touched columns
- `DiffError` (enum, pub) — Six error variants: KeyColumnMissingInTarget/Input, DuplicateKeyInTarget/Input, ColumnMismatch, StrictMatchFailed
- `DiffError::to_json()` (impl method, pub) — Converts error to JSON envelope with error code, context, and LLM-facing message
- `diff_records()` (fn, pub) — Main API: takes current/new records, key column, optional column whitelist, strict_match flag; returns Result<DiffResult, DiffError>
- `column_set()` (fn, private) — Collects union of all column names across records in stable first-seen order
- `duplicate_examples()` (fn, private) — Finds and returns up to N duplicate key values in records
- `key_to_string()` (fn, private) — Converts JSON value (used as row key) to string; returns None for null/array/object
- `values_equal()` (fn, private) — NaN-safe value equality: treats NaN==NaN as true, everything else uses serde_json PartialEq
- `tests` (mod, cfg-test) — 14 unit tests covering: cell diffing, restrict_columns filtering, unmatched keys, strict_match, duplicate detection, missing keys, column mismatches, empty inputs, null handling, NaN handling, JSON serialization

## File-level notes

- **Purity enforced:** No I/O, no async, no Sheets/CRDT specifics — callers translate cell changes into their respective APIs
- **Error handling:** All validation errors caught early with descriptive JSON envelopes for LLM consumption
- **NaN-safety:** `values_equal()` includes defensive NaN comparison since serde_json normally rejects NaN; test (line 521–533) validates serde_json behavior at runtime
- **Test coverage is comprehensive:** 14 tests covering happy path, all 6 error variants, edge cases (null keys, empty input, NaN), and column filtering
- **Column ordering:** Preserved via first-seen order in `column_set()`; comparable columns sorted alphabetically for determinism (line 240)
- **Strict match mode:** Optional validation flag rejects any new records with keys not present in current state (default false allows silent skip)
