# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/markdown_to_docs_ops.rs

**Layer:** infrastructure  
**Purpose:** Pure markdown → Google Docs `batchUpdate` request emitter. Converts markdown strings to a sequence of Google Docs API requests, supporting headings, lists, tables, inline formatting, and blocks; tracks lossy conversions for unsupported elements (footnotes, images).

## Symbols

### Public API
- `InsertionPoint` (pub enum) — Specifies where to insert markdown content: concrete UTF-16 index with optional tab_id, or end-of-segment placeholder.
- `ConversionResult` (pub struct) — Output of markdown conversion: list of batchUpdate requests and lossy conversion records.
- `markdown_to_requests` (pub fn) — Main entry point; parses markdown with pulldown-cmark, processes events via Emitter, returns requests and lossy list.

### Private types
- `ParaKind` (private enum) — Paragraph kind state: Normal, Heading(1-6), Bullet, Numbered, CodeBlock, Quote.
- `StyleFlags` (private struct) — Inline text styling state: bold, italic, strikethrough, code (monospace), optional link URL.
- `StyleSpan` (private struct) — Styled text region within a paragraph: start offset (UTF-16), length (UTF-16), style flags.

### Emitter state machine
- `Emitter` (private struct) — Core state machine: cursor position, tab_id, accumulated requests/lossy records, current paragraph buffer, style stack, table builder.
- `Emitter::new` (impl fn, private) — Constructor; initializes Emitter from InsertionPoint.
- `Emitter::current_style` (impl fn, private) — Merges all active styles from the style stack into a single StyleFlags.
- `Emitter::append_text` (impl fn, private) — Appends text to current paragraph, tracking UTF-16 length, recording style spans, respecting suppress_depth and table mode.
- `Emitter::feed` (impl fn, private) — Processes one markdown Event; dispatches to start_tag, end_tag, or text handlers.
- `Emitter::start_tag` (impl fn, private) — Handles Tag start events (Paragraph, Heading, List, Strong, Link, Table, etc.); updates ParaKind and style stack.
- `Emitter::end_tag` (impl fn, private) — Handles TagEnd events; flushes paragraph when needed, pops styles, handles table/footnote/image closing.
- `Emitter::flush_paragraph_if_any` (impl fn, private) — Emits InsertText, UpdateParagraphStyle, optional CreateParagraphBullets, and per-span UpdateTextStyle requests; advances cursor.
- `Emitter::emit_table` (impl fn, private) — Emits InsertTable request and per-cell InsertText pseudo-requests (v1 simplified shape, dispatcher resolves real indices after snapshot).
- `Emitter::finish` (impl fn, private) — Finalizes: flushes any remaining paragraph, returns ConversionResult.

### Helpers (private)
- `build_text_style` (private fn) — Constructs textStyle JSON object and field-mask string from StyleFlags.
- `_suppress_unused` (private fn, decorated `#[allow(dead_code)]`) — Suppresses compiler warning for unused `CodeBlockKind` import; no runtime purpose.
- `loc` (private fn) — Builds Location JSON object: `{ index, tabId? }` (tabId omitted if None for golden-fixture parity).
- `rng` (private fn) — Builds Range JSON object: `{ startIndex, endIndex, tabId? }` (tabId omitted if None for golden-fixture parity).

### Tests (private mod)
- `fixtures_dir` (private fn) — Resolves path to `tests/gdocs_markdown_fixtures` directory.
- `run_fixture` (private fn) — Loads markdown fixture, converts it, compares against golden `.ops.json` or regenerates with `COLMENA_MD_FIXTURE_UPDATE=1`.
- `fixture_01_heading_and_paragraph` through `fixture_14_lossy_math` (14 test fns) — Fixture-based tests covering headings, emphasis, lists, code, tables, lossy conversions.
- `end_of_segment_shape_matches_index_zero` (test fn) — Verifies `EndOfSegment` and `Index { index: 0 }` emit identical requests.
- `tab_id_threads_through_all_emitted_locations` (test fn) — Validates that every location/range in requests carries tabId when provided.

## File-level notes

- **UTF-16 accounting:** Throughout the file, text lengths are tracked in UTF-16 code units (not bytes or chars) to match Google Docs' internal indexing. All offset calculations use `s.encode_utf16().count()`.
- **Forward-only cursor:** Emission is strictly forward; no write-backwards complexity because all inserts happen at the running cursor position.
- **Lossy conversions:** Footnote definitions and image references emit a `LossyConversion` record and suppress output (`suppress_depth` counter). Math (`$...$`) is not detected by pulldown-cmark and passes through as plain text with no lossy record (acceptable for v1).
- **Golden fixtures:** Test suite uses `.md` + `.ops.json` golden pairs in `tests/gdocs_markdown_fixtures/`. The `COLMENA_MD_FIXTURE_UPDATE=1` env var regenerates them; omit flag to assert parity.
- **Tab routing:** The optional `tab_id` field threads through every `location` and `range` JSON object; when absent, fields are omitted for backward compatibility with existing golden fixtures.
- **Table v1 simplification:** `emit_table` uses pseudo-indices for cell addressing; the dispatcher resolves to real Google Docs indices after a snapshot fetch. This keeps the module stateless and testable.
- **Code quality:** Well-structured, comprehensive test coverage (14 fixture tests + 2 property tests), clear comments on strategy and unsupported elements, no TODOs or unimplemented stubs.
