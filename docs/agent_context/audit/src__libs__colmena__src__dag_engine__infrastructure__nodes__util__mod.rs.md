# src/libs/colmena/src/dag_engine/infrastructure/nodes/util/mod.rs

**Layer:** infrastructure  
**Purpose:** Module declaration re-exporting three utility submodules providing attachment ID generation, structured LLM output extraction with schema validation, and inline schema conversion/validation for DAG node infrastructure.

## Symbols

### Module declarations (from mod.rs)
- `attachment_id` (mod, public) — Re-exports attachment ID generation utilities for multimedia node producers
- `extract_with_schema` (mod, public) — Re-exports structured LLM extraction helpers for output parsing and routing nodes
- `inline_schema` (mod, public) — Re-exports inline schema conversion and validation utilities

### Attachment ID generation (from attachment_id.rs)
- `build_document_id()` (fn, public) — Builds collision-resistant `document_id` from filename, MIME type, and producer prefix; uses 8-char UUID v4 suffix for true uniqueness across rapid same-filename calls
- `short_uuid8()` (fn, private) — Generates first 8 hex chars of UUID v4; guarantees ~2^32 collision-resistant values for artifact producer IDs
- `file_ext()` (fn, public) — Maps MIME type (image/audio) to short extension string; covers PNG/JPEG/WebP/GIF/WAV/MP3/M4A/OGG/FLAC/L16 with "bin" fallback
- `sanitize()` (fn, public) — Normalizes string to lowercase alphanumeric + underscore/dash; trims leading/trailing underscores for clean stems

### Structured LLM output extraction (from extract_with_schema.rs)
- `ExtractInput<'a>` (struct, public) — Input configuration for LLM extraction: provider_kind, api_key, model, system_message, user_text, inline_schema, temperature, observer
- `extract_with_schema<'a>()` (fn, public, async) — Single-turn LLM call with system+user messages; strips markdown code fences; parses JSON; validates against inline schema; reports usage to observer
- `parse_and_validate()` (fn, public) — Parses text as JSON (handles ```json and ``` fences); validates against inline schema; skips validation for empty schema (legacy output_parser.rs behavior)
- `EmptyToolExecutor` (struct, private) — Stub tool executor that always returns "No tools available" error; used to prevent tool calls during extraction-only LLM invocation

### Inline schema conversion & validation (from inline_schema.rs)
- `inline_to_json_schema()` (fn, public) — Converts inline-required format `{ field: { type, required?, description? } }` to standard JSON Schema `{ type: "object", properties: {...}, required: [...] }`
- `validate_against_inline_schema()` (fn, public) — Validates JSON object against inline schema; checks required fields (non-null), type match per declared field; returns human-readable error on first violation
- `type_label()` (fn, private) — Helper to return string representation of JSON value type (null/boolean/number/string/array/object)

## File-level notes

- **Module structure:** Pure declaration file; all implementation lives in three submodules. No direct code in mod.rs itself.
- **Design coherence:** Three focused utility modules supporting complementary concerns: artifact identity (attachment_id), inference output handling (extract_with_schema), and schema data validation (inline_schema).
- **Usage patterns:**
  - `attachment_id` — Used by image_generation, image_edit, tts nodes (multimedia producers)
  - `extract_with_schema` — Used by output_parser, extraction, router/llm_direct, router/extract_and_route
  - `inline_schema` — Used by extract_with_schema (inline), router/extract_and_route
- **Intentional design choices:**
  - `_storage_key` parameter in `build_document_id()` is intentionally unused (prefixed with underscore); replaced with UUID v4 in 2026-06-07 fix to guarantee uniqueness across same-filename, same-storage-key rapid calls (regression test verifies)
  - `EmptyToolExecutor` is intentional stub for tool-less extraction flow; not unfinished
  - Private helpers (`short_uuid8`, `type_label`) have appropriate visibility
- **No flags:** All symbols are actively used, properly scoped, and well-documented.
