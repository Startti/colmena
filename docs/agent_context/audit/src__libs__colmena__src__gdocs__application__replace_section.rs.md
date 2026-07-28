# src/libs/colmena/src/gdocs/application/replace_section.rs

**Layer:** application  
**Purpose:** Use-case functions `run_replace_section` and `run_append_markdown` that replace or append markdown content in Google Docs with co-edit guard checks and table markdown rejection.

## Symbols

- `ReplaceSectionInput` (pub struct) — Input bundle: heading to find and new markdown to insert
- `ReplaceSectionInput.heading` (pub field) — Heading string that marks the section start (e.g. "## Resumen")
- `ReplaceSectionInput.new_markdown` (pub field) — Markdown content to replace into the section
- `AppendMarkdownInput` (pub struct) — Input bundle: markdown to append and optional tab scope
- `AppendMarkdownInput.new_markdown` (pub field) — Markdown content to append at end-of-segment
- `AppendMarkdownInput.tab_id` (pub field) — Optional tab ID to scope append to a specific tab
- `run_replace_section` (pub async fn) — Replaces content between heading and next same-or-higher heading (or EOF) with new markdown; validates via co-edit guard and rejects table markdown
- `run_append_markdown` (pub async fn) — Appends markdown at end-of-segment (optionally scoped to tab); validates via co-edit guard and rejects table markdown
- `lookup_para_opt` (fn) — Helper to find a paragraph by its sequence number in document snapshot
- `build_outline` (fn) — Helper to construct outline entries from document snapshot tabs and paragraphs
- `app_tests` (mod) — Unit test module with mocked clients
- `expect_get_sequence` (fn) — Test helper to queue mock snapshots for sequential get() calls
- `make_batch_update_ok` (fn) — Test helper to make batch_update() return success with a given revision ID
- `replace_section_happy` (test) — Happy path: replace section between headings succeeds
- `replace_section_heading_not_found` (test) — Guard rejects missing heading with InvalidArgs error
- `replace_section_rejects_table` (test) — Table markdown rejected before guard is called
- `append_markdown_happy` (test) — Happy path: append markdown at end-of-doc succeeds
- `append_markdown_rejects_table` (test) — Table markdown rejected in append path

## File-level notes

- **Duplication**: Cache invalidation + fresh fetch + snapshot store pattern repeated identically at lines 88–93 and 159–164; could be extracted to a helper function to reduce maintenance surface.
- **Comments**: Lines 84–87 and 155–158 clearly explain the CRITICAL distinction between `snapshot.revision_id` (from documents.get) and `batch_update`'s writeControl ID for co-edit guard correctness.
- **Edge case handling**: `run_append_markdown` carefully handles empty docs and tab-specific appends with saturating arithmetic and `unwrap_or(1)` fallback (lines 131–136).
- **V1 limitation**: Table markdown rejection (line 8) is a documented v1 limitation shared with `insert.rs`; no active work item noted.
- **Test coverage**: Four path tests (happy, missing heading, table reject in replace; happy, table reject in append) plus mocked guard behavior. No integration tests against real service.
