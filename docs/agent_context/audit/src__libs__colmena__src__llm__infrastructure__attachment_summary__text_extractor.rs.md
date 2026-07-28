# src/libs/colmena/src/llm/infrastructure/attachment_summary/text_extractor.rs

**Layer:** infrastructure  
**Purpose:** Local text extraction from document bytes (PDF, plaintext, markdown, CSV, HTML) and multi-byte-safe UTF-8 character truncation. Feeds the summary generator with bounded input text.

## Symbols

- `ExtractError` (enum, pub) — Error type for text extraction; distinguishes parse failures from encoding errors
- `ExtractError::PdfParse(String)` (variant, pub) — PDF parsing failure with error message
- `ExtractError::InvalidUtf8(String)` (variant, pub) — Invalid UTF-8 sequence in text document with error message
- `extract_text(mime: &str, bytes: &[u8])` (fn, pub) — Dispatcher that parses MIME type and extracts text; returns `Ok(Some(...))` for extracted text, `Ok(None)` for recognized but non-text-extractable MIME (images, archives), `Err` for malformed input
- `extract_pdf(bytes: &[u8])` (fn, private) — Extract text from PDF bytes using `pdf_extract` crate; wraps parse errors into `ExtractError::PdfParse`
- `extract_plaintext(bytes: &[u8])` (fn, private) — Decode plaintext/markdown/CSV/HTML bytes as UTF-8; wraps decode errors into `ExtractError::InvalidUtf8`
- `truncate_chars(s: &str, max_chars: usize)` (fn, pub) — Truncate string to at most `max_chars` Unicode characters without splitting UTF-8 code points; uses `char_indices()` for safe byte-position lookup
- `tests` (mod, private) — 14 unit tests covering truncation (ASCII, emoji, edge cases), plaintext extraction (various MIME types), PDF extraction, error cases, MIME normalization with parameters and whitespace

## File-level notes

- **Comprehensive test coverage**: All public functions exercised with normal, edge, and error cases. PDF extraction test uses a real fixture (`hello.pdf`); UTF-8 truncation tested with multi-byte emoji.
- **MIME normalization**: Robust to whitespace, parameters (e.g. `text/plain; charset=utf-8`), and case-insensitivity via split-on-semicolon + trim + lowercase pattern.
- **Clean boundary semantics**: `extract_text` returns full extracted text; caller must use `truncate_chars` separately to enforce `summary_max_chars` limit (documented in docstring).
- **Error handling**: Appropriate use of `thiserror` macro for domain errors; parse/encoding errors wrapped with context.
- **No unused code**: All private functions called from `extract_text`; public functions (`extract_text`, `truncate_chars`) are entry points for the attachment_summary module.
