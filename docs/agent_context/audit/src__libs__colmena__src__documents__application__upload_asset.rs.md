# src/libs/colmena/src/documents/application/upload_asset.rs

**Layer:** application  **Purpose:** Implements the UploadAssetUseCase — a use-case orchestrator that validates asset uploads (size, MIME type) and persists them through the AssetStore port abstraction.

## Symbols

- `UploadAssetInput` (struct, pub) — DTO: session_id, raw bytes, MIME type, and optional label for an asset to upload
- `UploadAssetOutput` (struct, pub) — DTO: returned asset_id and summary after successful upload
- `UploadAssetUseCase` (struct, pub) — Main orchestrator holding arc-wrapped AssetStore + IdGenerator, size limit, and allowed MIME set
- `execute` (method, pub async) — Validates file size and MIME type, generates asset ID, persists to store via port, fetches summary, returns output
- `allowed` (fn, test helper) — Creates a HashSet of allowed MIME types for tests
- `upload_happy_path` (test) — Verifies successful upload with valid PNG, correct ID allocation, and size metadata
- `rejects_too_large` (test) — Confirms TooLarge error when bytes exceed max_size_bytes
- `rejects_disallowed_mime` (test) — Confirms MimeNotAllowed error for PDF when only PNG/JPEG allowed

## File-level notes

- Clean application-layer design: validation logic separated into early-exit guards, then delegation to port.
- Error handling is idiomatic: `?` propagation on port calls, explicit Result returns for validation failures.
- Tests cover happy path and both validation error branches (size and MIME).
- No external dependencies beyond domain types and standard library; properly uses Arc for trait objects per hexagonal pattern.
- No todo!(), unimplemented!(), or stubbed logic detected.
