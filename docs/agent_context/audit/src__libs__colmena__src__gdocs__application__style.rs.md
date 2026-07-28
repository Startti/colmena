# src/libs/colmena/src/gdocs/application/style.rs

**Layer:** application  
**Purpose:** Implements the `style_text` use case — finds a substring in a document scope via regex and applies character-level (`updateTextStyle`) and paragraph-level (`updateParagraphStyle`) Docs API style updates through the co-edit guard.

## Symbols

- `StyleTextInput` (struct, pub) — Input struct for style_text use case: find pattern, StylePatch, Scope, case_sensitive, whole_word, occurrence index, anchor text, dry_run flag
- `run` (fn, pub, async) — Main orchestration: co-edit guard → regex match → build Docs API text/paragraph style requests → batch_update → cache invalidation → EditResult with changes and outline
- `build_style_payload` (fn, private) — Converts StylePatch into (textStyle JSON, paragraphStyle JSON, textFields CSV, paragraphFields CSV) for Docs API batch update
- `rgb_to_color` (fn, private) — Converts RgbColor domain type to Docs API color JSON structure
- `lookup_para` (fn, private) — Looks up ParagraphSnapshot by paragraph number; panics on not found [FLAG: improvement — should return Result<..., DocsError> instead of .expect()]
- `lookup_tab_id` (fn, private) — Looks up the TabId for a given paragraph number via flat_map over tabs and paragraphs
- `utf16_len` (fn, private) — Calculates UTF-16 code unit count of a string (needed for Docs API index conversion)
- `build_outline` (fn, private) — Collects all paragraphs across tabs into OutlineEntry vector with paragraph number, tab_id, kind, and 80-char text preview
- `app_tests` (mod, cfg(test)) — Test module with 4 async integration tests (bold, heading_level, link, dry_run, text_not_found)

## File-level notes

- **Error handling boundary:** `lookup_para` uses `.expect("paragraph exists")` on line 233. Since this is called from the public `run` function (a user-facing use case), a panic here is inappropriate — should be `Result<&ParagraphSnapshot, DocsError>` to let `run` return a structured error instead.
- **UTF-16 index handling:** Correctly uses `utf16_len` to convert byte offsets to UTF-16 code unit offsets for Docs API (lines 92–95), which is critical for multi-byte character accuracy.
- **Paragraph-level style scope:** Line 107 comment correctly notes that paragraph-level style applies to the entire containing paragraph, not just the matched substring.
- **Revision ID persistence:** Lines 141–150 include clear explanation of why `snap.revision_id` (documents.get format) is stored instead of batch_update's writeControl id — ensures next guard's revisionId comparison succeeds.
- **Test coverage:** 4 tests cover happy paths (bold, heading_level, link), dry_run skip, and text_not_found error; all use TestRig and mock expectations.
