# src/libs/colmena/src/gdocs/application/named_range.rs

**Layer:** application  
**Purpose:** Implements two use cases for Google Docs named ranges: `run_create` registers a stable, name-addressable span within a document; `run_replace` updates a named range's contents via `replaceNamedRangeContent` with co-edit guarding and revision tracking.

## Symbols

- `CreateNamedRangeInput` (struct, pub) — DTO for named range creation: accepts `name` and `scope` (v1 supports `Scope::Paragraph` only; other scopes rejected at dispatcher layer)
- `ReplaceNamedRangeInput` (struct, pub) — DTO for named range replacement: accepts `name` and `new_text`
- `run_create` (fn, pub async) — Use case: guards the document read, delegates range creation to client, returns `NamedRangeMeta` with stable ID and paragraph bounds
- `run_replace` (fn, pub async) — Use case: guards document, finds named range by name (fails if missing), batches `replaceNamedRangeContent`, invalidates cache, fetches fresh snapshot with new revision, stores revision tracking, builds outline, returns `EditResult` with change records and outline
- `build_outline` (fn, private) — Flattens document tabs and paragraphs into `OutlineEntry` vector with paragraph ID, tab ID, kind, and 80-char text preview
- `app_tests` (mod, cfg(test)) — Test module with 4 async tests covering happy path and error cases
  - `expect_get_sequence` (fn, private) — Test helper: enqueues snapshots for mock client.get() to return in sequence
  - `make_batch_update_ok` (fn, private) — Test helper: mocks batch_update to return success with given revision ID
  - `create_named_range_paragraph_scope_happy` (test, async) — Verifies `run_create` with paragraph scope succeeds and returns correct metadata
  - `create_named_range_passes_arbitrary_scope_to_client` (test, async) — Verifies `run_create` forwards scope to client without application-layer validation
  - `replace_named_range_happy` (test, async) — Verifies `run_replace` replaces text, updates revision, returns correct change record
  - `replace_named_range_not_found` (test, async) — Verifies `run_replace` returns `InvalidArgs` error when named range doesn't exist

## File-level notes

- No dead code, unfinished work, or `todo!()`/`unimplemented!()` stubs detected.
- Error handling is explicit and appropriate: `ok_or_else` with descriptive error messages, proper `?` propagation.
- Revision tracking strategy is documented clearly (lines 63–66 CRITICAL comment): uses `documents.get` revision ID (not `batch_update`'s writeControl id) for co-edit guard consistency.
- Tests are comprehensive and well-structured: happy paths, error cases, and scope-passthrough verification covered.
- Code quality is solid: focused scope, clean separation between DTOs and use cases, minimal helper functions with clear purpose.
