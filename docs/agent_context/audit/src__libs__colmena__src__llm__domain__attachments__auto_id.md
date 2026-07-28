# src/libs/colmena/src/llm/domain/attachments/auto_id.rs

**Layer:** domain  
**Purpose:** Deterministically generate stable attachment IDs from file metadata (filename, mime_type, size, source). Produces `att_<hex16>` identifiers that remain consistent across invocations for identical inputs.

## Symbols

- `generate_attachment_id` (pub fn) — Computes a stable attachment ID by SHA256-hashing filename, mime_type, size, and source-specific discriminator (URL for SignedUrl, path for Path, bytes digest for Inline); returns `att_<16-char-hex>` format
- `tests` (mod) — Test module

### Test functions (private, in `tests` mod)
- `same_inputs_produce_same_id` (fn) — Verifies identical inputs yield identical IDs and confirm format/length (4 + 16 chars)
- `different_urls_produce_different_ids` (fn) — Verifies different SignedUrl sources produce different IDs
- `different_filenames_produce_different_ids` (fn) — Verifies different filenames produce different IDs
- `inline_uses_bytes_digest` (fn) — Verifies different bytes digests for Inline sources produce different IDs

## File-level notes

- Clean, focused module with single responsibility
- Comprehensive test coverage covering determinism, differentiation by source type, and differentiation by content
- Well-documented public function with clear explanation of discriminator strategy per `AttachmentSource` variant
- Handles missing `size_bytes` gracefully (hashes `"?"` as placeholder)
- Takes upstream-computed `inline_bytes_digest` to avoid copying large buffers into the function
- Deterministic hash uses only first 8 bytes of SHA256 digest (128 bits → 16 hex chars), sufficient for collision-free stable IDs
