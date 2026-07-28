# src/libs/colmena/src/gdocs/application/insert.rs

**Layer:** application  
**Purpose:** Implements five insert use-cases for Google Docs (insert_after_text, insert_before_text, insert_between, insert_image_after_text, insert_image_from_bytes). All are content-addressed: agent specifies anchor/heading text, dispatcher locates it, computes UTF-16 index, converts markdown via markdown_to_docs_ops, and POSTs one batchUpdate with co-edit safety.

## Symbols

### Input structs
- `InsertAfterTextInput` (struct, pub) — anchor + new_markdown + occurrence count for text-based insert
- `InsertBeforeTextInput` (struct, pub) — anchor + new_markdown + occurrence count for text-based insert
- `InsertBetweenInput` (struct, pub) — after_heading + before_heading (None→EOF) + new_markdown for section-based insert
- `InsertImageAfterTextInput` (struct, pub) — anchor + image_url + occurrence + optional width_pt/height_pt for image insert

### Public use-case functions
- `run_after_text` (async fn, pub) — insert markdown after (occurrence-th) anchor match; validates no tables, guards scope, locates anchor, converts markdown, applies batchUpdate, finalizes
- `run_before_text` (async fn, pub) — insert markdown before (occurrence-th) anchor match; same flow as run_after_text
- `run_between` (async fn, pub) — insert markdown between two section headings using BetweenHeadings scope; resolves paragraph_start from guard, applies insertion
- `run_insert_image_after_text` (async fn, pub) — insert inline image (public URL only) after anchor match; validates URL, locates anchor, builds insertInlineImage request, finalizes
- `run_insert_image_from_bytes` (async fn, pub) — upload raw image bytes to Drive, make public via lh3 URL, insert inline image, delete temp file with transactional cleanup on error

### Public/crate helper functions
- `image_ext_for_mime` (pub(crate) fn) — map image MIME type (image/png, image/jpeg, image/gif) to Drive filename extension; returns None for unsupported/non-image types
- `reject_table_markdown` (pub(crate) fn) — pre-scan markdown for CommonMark table separator syntax (|---|) and reject with v1 limitation message; uses static regex
- `apply_and_finalize` (pub(crate) async fn, 8 params) — apply batchUpdate requests to doc, invalidate cache, fetch fresh snapshot, store revision_id, return EditResult with outline + lossy conversions + soft warnings

### Private helper functions
- `short_token` (fn) → String — generate short hex token from SystemTime nanoseconds for temp Drive filenames; collision harmless
- `validate_image_url` (fn) → Result<(), DocsError> — validate image_url is public http(s), ≤2000 chars; rejects attachment IDs with v1.1 deferral message
- `build_insert_image_request` (fn) → serde_json::Value — construct single insertInlineImage batchUpdate request; includes optional objectSize (PT dimensions) when width_pt or height_pt provided
- `find_anchor` (fn) → Result<(u32, usize, usize, Option<TabId>), DocsError> — linear scan all tabs/paragraphs for (occurrence-th) match of anchor string; returns (paragraph_n, byte_off_in_paragraph, byte_len, tab_id)
- `lookup_para` (fn) → &ParagraphSnapshot — lookup paragraph by number; panics if not found (expects invariant)
- `lookup_para_or_eof` (fn) → &ParagraphSnapshot — lookup paragraph by number, or return last paragraph if n exceeds doc; panics if doc is empty (expects invariant)
- `utf16_len` (fn) → u32 — count UTF-16 code units in string (used for Google Docs index calculation)
- `build_outline` (fn) → Vec<OutlineEntry> — extract outline (paragraph n + tab_id + kind + 80-char text preview) from fresh snapshot

### Test modules
- `tests` (mod, cfg(test)) — 6 unit tests covering markdown table rejection, image URL validation, image request building, literal pipes acceptance
- `app_tests` (mod, cfg(test)) — 10 integration tests with mocked DocsClient covering: insert_after/before text happy path + occurrence selection, image insert happy path + URL rejection, anchor not found, table rejection, image-from-bytes upload-make-public-insert-cleanup flow with transactional cleanup on failure, soft warnings on cleanup failure, insert_between headings

## File-level notes

- **Well-structured integration with co-edit guard**: all entry points (`run_after_text`, `run_before_text`, `run_between`, `run_insert_image_after_text`) call `run_guard(ctx, doc_id, &scope)` first to obtain snapshot + soft_warnings + resolved scope.
- **v1 limitations clearly documented**: table markdown rejected (line 8-11, 317-321) with pointer to v1.1 `gdocs_insert_table` tool; public-URL-only image inserts (line 103-107, 341-344) with pointer to deferred attachment-source path in BACKLOG.
- **Transactional image-from-bytes**: upload → set_anyone_reader → construct URL → run_insert_image_after_text → delete temp file. On error after upload, temp file is deleted before returning error (line 193-194, 207-208). On cleanup failure, soft warning is appended (line 213-217).
- **Minor inefficiencies** (not blocking): regex compiled on every `reject_table_markdown` call (line 313-314); `find_anchor` does linear scan over all paragraphs to collect all occurrences then selects by index (line 404-429). Both acceptable for typical doc sizes but could be optimized for very large documents.
- **`#[allow(clippy::too_many_arguments)]`** on `apply_and_finalize` (line 454) — function has 8 params (ctx, doc_id, snap, requests, kind, paragraph, new_markdown, tab_id, lossy, soft_warnings). Deliberate design choice; could be wrapped in struct but readability trade-off is acceptable.
- **Test coverage comprehensive**: unit tests validate input sanitization (tables, URLs, emptiness, length); integration tests cover both happy paths and error cases (anchor not found, insertion failure triggering cleanup, cleanup failure triggering warnings, occurrence selection).
- **No unfinished code**: no todo!(), unreachable!(), unimplemented!(), or FIXME comments.
