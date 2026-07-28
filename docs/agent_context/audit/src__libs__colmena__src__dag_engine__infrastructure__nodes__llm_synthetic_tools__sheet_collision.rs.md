# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/sheet_collision.rs

**Layer:** infrastructure  **Purpose:** Shared collision-policy parsing and error-envelope construction for sheet write dispatchers. Data-only module (no async, no I/O) that centralized policy logic and structured error responses for gsheets_run_python and crdt_doc_run_python.

## Symbols

- `CollisionPolicy` (enum, pub) — What the dispatcher does when a target tab already exists; serializable with three variants: Fail (default), AutoSuffix, Overwrite
- `CollisionPolicy::Fail` (variant, pub) — Default: dispatcher returns SheetExists error without writing; LLM must explicitly choose next action
- `CollisionPolicy::AutoSuffix` (variant, pub) — Legacy behavior: silently write as "Name (2)"
- `CollisionPolicy::Overwrite` (variant, pub) — Replace existing tab; operator accepts risk, no round-trip
- `parse_policy` (fn, pub) — Parse operator-supplied policy string into CollisionPolicy; wraps serde deserialization and provides clear error listing valid options
- `TabMeta` (struct, pub) — Metadata about an existing tab (row/column counts, column names, optional Drive modification timestamp)
- `TabMeta::n_rows` (field, pub) — Row count
- `TabMeta::n_cols` (field, pub) — Column count
- `TabMeta::columns` (field, pub) — List of column names as strings
- `TabMeta::last_modified` (field, pub) — Optional RFC 3339 timestamp from Drive API; None when source has no Drive concept or lookup failed
- `build_sheet_exists_error` (fn, pub) — Construct structured SheetExists error payload with current tab metadata, advice, and three valid next moves (rename, update_in_place, overwrite) with example code
- `tests` (mod, private) — Test module with five tests covering policy parsing, default, error messages, and error payload structure

## File-level notes

- Thin, focused module with single responsibility: stateless collision-policy serialization and error construction
- All public symbols are exported and directly used by sheet write dispatchers
- Test coverage is comprehensive (policy parsing, defaults, error messages, field presence/omission)
- No infrastructure dependencies; serde+serde_json only
