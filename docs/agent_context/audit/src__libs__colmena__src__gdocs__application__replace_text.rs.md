# src/libs/colmena/src/gdocs/application/replace_text.rs

**Layer:** application  
**Purpose:** Content-addressed scoped find/replace use case for Google Docs. Resolves UTF-16 positions, runs co-edit guards, and emits batchUpdate requests for text replacements with support for case sensitivity, whole-word matching, occurrence targeting, paragraph scoping, and anchor disambiguation.

## Symbols

- `ReplaceTextInput` (struct, pub) — Input parameters for replace operation: find/replace strings, scope, case sensitivity, whole-word flag, dry-run mode, confirm-many flag, occurrence ordinal, and optional anchor for disambiguation
- `run` (fn, pub async) — Main entry point: validates via co-edit guard, collects regex matches within scope, applies occurrence/anchor/count filters, optionally dry-runs, emits write-backwards batchUpdate, and refreshes cache + revision state
- `lookup_para` (fn, private) — Helper that finds a paragraph by number across all tabs; panics if not found (expects caller to have filtered by scope)
- `lookup_tab_id` (fn, private) — Helper that finds the tab ID containing a given paragraph number; returns None for default tab
- `build_previews` (fn, private) — Builds match preview strings (~30 chars before, ~50 after each hit) for error display; respects char boundaries
- `build_outline` (fn, private) — Constructs document outline from all paragraphs with kind and 80-char text preview; used after successful write
- `utf16_len` (fn, private) — Counts UTF-16 code units in a string slice; used for Google Docs API index calculations
- `take_chars_in_byte_range` (fn, private) — Extracts substring via byte range; silently returns empty string if range is invalid
- `take_slice_around` (fn, private) — Safely slices a string around byte boundaries with explicit char-boundary advancement; takes first 80 chars of result
- `app_tests` (mod, test) — Test module with 10 test cases covering happy path, error conditions (TextNotFound, ConfirmManyMatches), occurrence/anchor/scope filtering, case sensitivity, whole-word matching, and dry-run mode

## File-level notes

- All regex matches come from `regex` crate; escaping via `regex::escape()` prevents injection
- UTF-16 offset calculations are critical for Google Docs API (`start_index` + UTF-16 length prefix + match length); verified by tests
- Writes are always backwards (highest paragraph/offset first) to preserve indices during multi-delete + multi-insert
- Revision ID handling distinguishes between `documents.get` opaque tokens (~111 chars) and `batchUpdate` writeControl IDs (~25 chars); stores snapshot ID for correct guard comparison on next call (line 183 comment explains critical invariant)
- Co-edit guard runs before any writes; `soft_warnings` for out-of-scope human changes passed through to result
- Error surface: `InvalidArgs` (regex build), `TextNotFound` (no matches), `ConfirmManyMatches` (≥5 matches without confirm flag)
- No explicit TODOs, panic points are defended by precondition assertions with comments
- Test helpers `expect_get_sequence`, `make_batch_update_ok`, `default_input`, test document factory `snap()` support 10 scenarios
