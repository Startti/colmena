# src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs

**Layer:** infrastructure  **Purpose:** Implements HTML rendering of an IR document using maud macros for type-safe HTML generation, asset bundling via include_str!, and base64-inlined images.

## Symbols

**Constants**
- `THEME_EXECUTIVE` (const, private) — CSS theme for executive/formal presentation style
- `THEME_MINIMAL` (const, private) — CSS theme for minimal/clean presentation style
- `THEME_VIBRANT` (const, private) — CSS theme for vibrant/colorful presentation style
- `THEME_DARK` (const, private) — CSS theme for dark/high-contrast presentation style
- `CHART_JS` (const, private) — Bundled Chart.js library for chart rendering
- `SLIDES_RUNTIME` (const, private) — Bundled JavaScript runtime for slide deck interactivity

**Structs**
- `HtmlRenderer` (struct, pub) — Main renderer struct holding an AssetStore port

**Trait implementations**
- `impl HtmlRenderer` — Constructor and utilities
  - `new(asset_store)` (pub fn) — Creates a new HtmlRenderer with an AssetStore dependency
- `impl IRRenderer for HtmlRenderer` (async_trait) — Renders HtmlIR to HTML bytes
  - `render(&self, ir)` (async fn) — Converts HtmlIR to self-contained HTML with inlined assets
  - `target_extension(&self)` (fn) — Returns "html" file extension
  - `target_mime(&self)` (fn) — Returns "text/html; charset=utf-8" MIME type

**Helper functions (private)**
- `load_theme(theme)` — Maps Theme enum to bundled CSS string
- `locale_lang(locale)` — Maps Locale enum to language code ("en"/"es")
- `doc_has_chart(ir)` — Checks if any slide in the IR contains a chart block
- `slide_has_chart(s)` — Checks if a slide contains a chart block
- `block_is_or_contains_chart(b)` — Recursively checks if a block is a chart or contains charts in column layouts [FLAG: improvement — duplicated with collect_chart_inits]
- `collect_asset_ids(ir)` — Walks IR tree and collects all referenced asset IDs
- `render_body(ir, assets)` — Renders document body div with layout class and all slides
- `render_slide(slide, assets, ir)` — Renders single slide as section element with title/subtitle and blocks
- `render_footer(ir)` — Renders footer div with custom text and optional page numbers
- `page_numbers_label(locale, total)` — Generates localized page number label text
- `render_runs(runs)` — Renders array of text runs as styled inline elements
- `render_run(r)` — Renders single text run (handles bold, italic, code, links)
- `render_block(b, assets, ir)` — Main block renderer; large match statement covering 15+ block types (Heading, Paragraph, List, Blockquote, Code, Table, Chart, KpiCard, KpiGrid, Image, TwoColumns, ThreeColumns, Comparison, Callout, Divider, Video, AutoToc)
- `render_table_cell(c)` — Renders table cell content (Text, Number, or Runs)
- `render_chart_init_scripts(ir)` — Generates Chart.js initialization JavaScript code for all charts
- `collect_chart_inits(blocks, out)` — Recursively collects Chart.js init scripts from blocks [FLAG: improvement — duplicated tree walk with block_is_or_contains_chart]
- `chart_init_for(block_id, c)` — Generates JavaScript Chart.js constructor for a single chart

**Test module**
- `render_blocking(ir)` (fn, private) — Test helper to synchronously render IR using Tokio runtime
- `minimal_ir()` (fn, private) — Test fixture providing a minimal valid HtmlIR JSON structure
- Tests (12 functions) — Comprehensive happy-path coverage: DOCTYPE presence, theme CSS inclusion, report/slides mode behavior, chart script injection, headings, links, HTML escaping, tables, KPI grids, videos, charts, and auto-TOC generation

## File-level notes

1. **Missing error handling for missing assets** (lines 378, 479): `unwrap_or_default()` silently returns empty string when asset lookup fails in `render_block()` (Image block) and `render_block()` (Video block). If an asset ID is collected but the resolution failed, this produces a broken `<img src="">` or `<iframe src="">` instead of logging or returning an error. Should at least produce a placeholder URL or warning.

2. **Duplicated recursive block-traversal pattern** (lines 53–80 vs 530–553): Both `block_is_or_contains_chart()` and `collect_chart_inits()` implement identical TwoColumns/ThreeColumns recursion logic. The tree-walk structure could be extracted into a single higher-order function or iterator to avoid duplication.

3. **Large match statement in `render_block()`** (lines 263–507): The 220+ line pattern match covering 15 block types is comprehensive and well-structured, but suggests potential for splitting into a separate render module or trait if the codebase continues to grow.

4. **Test coverage is solid**: 12 unit tests cover rendering output, asset handling, layout modes, styling, escaping, and special block types. No obvious gaps.
