# src/libs/colmena/src/llm/domain/attachments/conversation_attachment.rs

**Layer:** domain  
**Purpose:** Defines domain types for attachments in conversations: `AttachmentSource` (where it came from and recovery strategy) and `ConversationAttachment` (metadata record with storage and usage tracking).

## Symbols

- `AttachmentSource` (enum, pub) — Tagged enum representing three attachment source origins: SignedUrl, Path, or Inline; drives expiry-recovery strategy.
- `AttachmentSource::SignedUrl` (variant) — Variant for attachments sourced from a signed URL string.
- `AttachmentSource::Path` (variant) — Variant for attachments sourced from a file path string.
- `AttachmentSource::Inline` (variant) — Variant for attachments sourced inline (no string value; bytes not retained after first upload).
- `AttachmentSource::kind_str` (method, pub) — Returns static string representation of the source kind for serialization/logging.
- `AttachmentSource::value` (method, pub) — Returns the URL or path string for SignedUrl/Path sources, None for Inline.
- `AttachmentSource::is_recoverable` (method, pub) — Returns true if the source can be re-uploaded on expiry (SignedUrl or Path); false for Inline.
- `ConversationAttachment` (struct, pub) — Domain value object holding attachment metadata: session/document/provider IDs, mime type, filename, size, label, description, source, timestamps, storage key, origin, and last-used tracking.
- `ConversationAttachment::catalog_line` (method, pub) — Formats attachment metadata for LLM display as: `"<doc_id>" — <label or filename> (<mime>, <size>)[. <description>]`
- `human_size` (function, private) — Helper to format byte counts as human-readable strings (B, KB, MB, GB with 1 decimal place).
- `tests::mk` (function, private) — Test helper to construct a ConversationAttachment with configurable label, description, and size.
- `tests::source_kind_str_matches_serialized_form` (test) — Verifies kind_str() output matches serde tag values for all three source variants.
- `tests::inline_source_is_not_recoverable` (test) — Verifies is_recoverable() returns false for Inline, true for SignedUrl and Path.
- `tests::catalog_line_uses_label_when_present` (test) — Verifies catalog_line() prefers label and formats size and mime type correctly.
- `tests::catalog_line_falls_back_to_filename_without_label` (test) — Verifies catalog_line() uses filename when label is None.
- `tests::catalog_line_appends_description_when_present` (test) — Verifies catalog_line() appends description with ". " separator when present and non-empty.
- `tests::unknown_size_renders_as_question_mark` (test) — Verifies catalog_line() displays "?" when size_bytes is None.
- `tests::conversation_attachment_holds_storage_key_origin_last_used_at` (test) — Verifies struct fields storage_key, origin, and last_used_at can be set and retrieved.

## File-level notes

- Clean domain layer with zero infrastructure dependencies; only uses chrono, serde, and ProviderKind (another domain type).
- Well-commented with Plan A/Plan B/Plan C references pointing to specs (attachment uniform resolution design).
- All public methods have clear single-responsibility: source classification, recovery eligibility, catalog rendering.
- Comprehensive test coverage (6 tests) for both AttachmentSource and ConversationAttachment behavior.
- No issues: no dead code, no unfinished stubs, no error-handling gaps, no unclear naming.
