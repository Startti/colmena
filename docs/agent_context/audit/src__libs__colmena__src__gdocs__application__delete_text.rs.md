# src/libs/colmena/src/gdocs/application/delete_text.rs

**Layer:** application  **Purpose:** Implements content-addressed scoped deletion in Google Docs with regex matching, co-edit guarding, and preview generation—mirrors `replace_text` but emits only `deleteContentRange` requests.

## Symbols

- `DeleteTextInput` (pub struct) — Input parameters: find pattern, scope, case sensitivity, whole-word matching, occurrence filter, confirm-many flag
- `run` (pub async fn) — Main entry point: guards scope, finds all regex matches, filters by occurrence or requires confirmation, executes batch delete, invalidates cache, returns EditResult with ChangeRecords
- `lookup_para` (fn) — Finds a paragraph by number in snapshot, panicking with expectation guarantee message if not found
- `lookup_tab_id` (fn) — Resolves the tab ID containing a given paragraph number, returning Option
- `utf16_len` (fn) — Calculates UTF-16 encoded character count for string slice (required for Docs API byte offsets)
- `build_previews` (fn) — Generates MatchPreview entries (index, paragraph, truncated context) for all hits
- `take_slice_around` (fn) — Extracts and truncates a string slice respecting UTF-8 char boundaries, limiting to 80 chars
- `build_outline` (fn) — Constructs OutlineEntry list from document snapshot (paragraph number, tab ID, kind, text preview)
- `expect_get_sequence` (fn, #[cfg(test)]) — Test helper: queues DocumentSnapshots for MockDocsClient::get to return sequentially
- `make_batch_update_ok` (fn, #[cfg(test)]) — Test helper: configures MockDocsClient::batch_update to return success with given revision ID
- `default_input` (fn, #[cfg(test)]) — Creates DeleteTextInput test fixture with defaults (find "deleteme", case-insensitive, no occurrence filter)
- `delete_text_happy` (async test) — Verifies deletion of single occurrence produces one ChangeRecord
- `delete_text_not_found` (async test) — Confirms TextNotFound error when pattern doesn't match
- `delete_text_confirm_many_threshold` (async test) — Verifies ConfirmManyMatches error at 5+ hits without consent
- `delete_text_multi_match_deletes_all` (async test) — Confirms three matches (below threshold) are all deleted
- `delete_text_whole_word_filters` (async test) — Confirms word-boundary filtering rejects partial matches

## File-level notes

- Follows hexagonal architecture: domain error types, application orchestration, infrastructure (DocsClient trait) accessed via GuardContext
- Regex construction properly escapes find pattern and handles word-boundary wrapping; case sensitivity via builder flag
- Hit collection scans all tabs/paragraphs filtered by scope, builds (paragraph_n, byte_offset, length) tuples
- Occurrence filtering (1-indexed) and confirmation gate (≥5 matches, requires `confirm_many: true`) match `replace_text` logic
- Sort-by-reverse (paragraph desc, offset desc) before batch delete ensures range indices remain valid across deletions
- UTF-16 offset calculation critical for Docs API: converts byte positions to UTF-16 units via `lookup_para.start_index`
- Cache invalidation → fresh `get()` → revision storage ensures next co-edit guard uses authoritative revision ID (not batch_update's writeControl ID)
- Tests use mockall and TestRig pattern with snapshot sequences; all guard branches (TextNotFound, ConfirmManyMatches, happy path) covered
- No unfinished stubs, todos, or dead code detected
