# src/libs/colmena/src/documents/infrastructure/validation/html_validator.rs

**Layer:** infrastructure  
**Purpose:** Implements `IRValidator` trait for HtmlIR validation. Performs structural (cardinality, unique IDs, layouts), security (URL schemes, image/video sources, nesting depth), and consistency (asset references) checks using regex and recursive block traversal.

## Symbols

- `HtmlValidator` (struct, pub) — Empty marker struct implementing IRValidator for HtmlIR
- `impl IRValidator for HtmlValidator::validate` (fn, pub) — Orchestrates all validation checks: cardinality, unique IDs, layouts, security, asset consistency; deserializes JSON to HtmlIR and delegates to sub-validators
- `validate_cardinality` (fn, private) — Rejects IR if slides array is empty
- `validate_unique_ids` (fn, private) — Ensures slide IDs are globally unique and block IDs are unique across all slides
- `block_id` (fn, private) — Exhaustively extracts `id` field from any Block variant via pattern match
- `validate_layouts` (fn, private) — Checks Title/SectionDivider layouts have non-empty titles and heading levels are 1..=6
- `validate_assets_referenced_consistency` (fn, private) — Verifies every asset referenced in blocks is declared in assets_referenced array
- `collect_asset_refs` (fn, private) — Recursively collects asset IDs from Image/Video blocks and nested TwoColumns/ThreeColumns
- `ALLOWED_LINK_SCHEMES` (static, private) — Array of permitted URL schemes: http, https, mailto, tel
- `DATA_URL_IMAGE_RE` (static Lazy<Regex>, private) — Regex for base64-encoded data URLs with allowed image types (png, jpeg, jpg, gif, webp)
- `EXTERNAL_HTTP_RE` (static Lazy<Regex>, private) — Regex matching http/https scheme prefix
- `VIDEO_ID_RE` (static Lazy<Regex>, private) — Regex for valid video ID format [A-Za-z0-9_-]+
- `MAX_NESTING_DEPTH` (const u8, private) — Maximum allowed nesting depth for column blocks: 3
- `link_scheme_ok` (fn, private) — Checks if URL uses an allowed scheme (case-insensitive prefix match)
- `validate_security` (fn, private) — Iterates slides and delegates recursive block security validation
- `validate_blocks_security` (fn, private) — Recursively validates block content: text-run links, image/video sources, chart data, KPI grid dimensions, nested-layout depth
- `nesting_err` (fn, private) — Helper to construct DocumentError for MAX_NESTING_DEPTH violation
- `check_runs_links` (fn, private) — Validates all text-run links use allowed schemes
- `check_image_src` (fn, private) — Validates ImageSrc: asset refs (pass-through), external URLs (http/https only), data URLs (base64-encoded image types only)
- `check_video_src` (fn, private) — Validates VideoSrc: YouTube/Vimeo video IDs match [A-Za-z0-9_-]+, asset refs pass-through
- `check_chart` (fn, private) — Validates ChartSpec: at least one series, pie/doughnut require exactly 1 series, series data length matches x_axis categories

**Test symbols:**
- `tests::minimal_ir` (fn, private) — Constructs minimal valid HtmlIR JSON with one blank slide
- `tests::valid_minimal_ir_passes` (test) — Verifies minimal IR passes all validations
- `tests::empty_slides_rejected` (test) — Verifies empty slides array is rejected
- `tests::duplicate_slide_ids_rejected` (test) — Verifies duplicate slide IDs fail validation
- `tests::title_layout_without_title_rejected` (test) — Verifies Title layout requires non-empty title
- `tests::heading_level_7_rejected` (test) — Verifies heading level out of range 1..=6 is rejected
- `tests::duplicate_block_ids_across_slides_rejected` (test) — Verifies block ID uniqueness across all slides
- `tests::assets_referenced_mismatch_rejected` (test) — Verifies blocks cannot reference undeclared assets
- `tests::link_javascript_scheme_rejected` (test) — Verifies javascript: URLs are rejected
- `tests::image_external_ftp_rejected` (test) — Verifies FTP URLs rejected for external images
- `tests::image_data_url_svg_rejected_v1` (test) — Verifies SVG data URLs rejected (v1 limitation)
- `tests::chart_bar_mismatched_series_categories_rejected` (test) — Verifies series data length matches x_axis categories
- `tests::chart_pie_two_series_rejected` (test) — Verifies pie/doughnut charts require exactly 1 series
- `tests::nested_grids_depth_4_rejected` (test) — Verifies nesting depth > MAX_NESTING_DEPTH is rejected

## File-level notes

- **Architecture compliance:** Correctly placed in infrastructure — uses regex and JSON traversal (implementation detail), delegates domain contract to `IRValidator` trait and `DocumentError` error type
- **Error handling:** All validation failures return structured `DocumentError::IRValidationFailed` with path (JSON pointer) and human-readable reason
- **Completeness:** Block exhaustive match via `block_id()` will trigger compile error if new Block variants are added, ensuring validators stay in sync
- **Test coverage:** 10 test functions with comprehensive boundary cases (empty arrays, duplicates, invalid ranges, scheme restrictions, nesting depth, schema mismatches)
- **Security-first design:** Dedicated `validate_security()` pass with regex-backed URL scheme and video ID validation; separates security concerns from structural/consistency checks
- **Lazy statics:** Regexes initialized once via `once_cell::sync::Lazy` to avoid recompilation per check
