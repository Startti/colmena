# src/libs/colmena/src/dag_engine/infrastructure/nodes/util/attachment_id.rs

**Layer:** infrastructure  **Purpose:** Provides stable, collision-resistant `document_id` derivation for multimedia artifacts (image_generation, image_edit, tts nodes). Ensures consistent ID formatting across providers and prevents silent overwrites when multiple providers generate same-filename artifacts.

## Symbols

- `build_document_id` (pub fn) — Builds human-friendly, collision-resistant document_id from filename, MIME type, and prefix; uses 8-hex UUID suffix for guaranteed uniqueness across same-filename/same-storage-key calls
- `short_uuid8` (fn, private) — Generates first 8 hex chars of UUID v4; provides ~4.3×10⁹ collision-free values for single-agent artifact volumes
- `file_ext` (pub fn) — Maps MIME types to short extensions (image/png→png, audio/wav→wav, etc.); unknown types fall back to "bin"
- `sanitize` (pub fn) — Sanitizes strings to `[A-Za-z0-9_-]`, lowercased; trims leading/trailing underscores
- `tests` (mod, cfg(test)) — Comprehensive unit tests covering: extension stripping + UUID suffix, empty-stem fallback, collision prevention (same filename, same key, rapid generation), sanitization, MIME coverage

## File-level notes

- **Parameter `_storage_key` intentionally unused** (prefixed with underscore to suppress lint): the 2026-06-07 fix replaced storage-key-based suffix derivation (which caused collisions when keys ended with file extensions) with UUID v4-based suffix. Kept in signature for API/documentation stability. Regression test at line 141-172 documents the pre-fix collision scenario and verifies the new UUID approach.
- **Test coverage is exemplary**: collision regression test (line 141-172) explains the historical bug; rapid-call test (line 175-186) empirically validates 1000 rapid calls with identical args produce distinct IDs; MIME/sanitization tests exhaustive.
- **No error paths or fallible operations**: `file_ext` returns `&'static str` with sensible default; `sanitize` always succeeds; UUID generation is infallible.
- **Module module-level doc comment** (line 1-4) clearly states Plan A context and three producer nodes.
