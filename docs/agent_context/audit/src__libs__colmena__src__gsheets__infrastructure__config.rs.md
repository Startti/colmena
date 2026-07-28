# src/libs/colmena/src/gsheets/infrastructure/config.rs

**Layer:** infrastructure  
**Purpose:** gsheets-specific configuration struct that reads OAuth scopes, request timeout, retry budget, and share_email from environment variables. Post-OAuth migration (2026-06-10), credentials live in the shared `google_oauth` subsystem.

## Symbols

- `DEFAULT_SCOPES` (pub const) — array of default OAuth scope URLs (spreadsheets, drive.file)
- `GSheetsConfig` (pub struct) — configuration container holding scopes, request_timeout (Duration), max_retries (u32), and share_email (String)
- `GSheetsConfig::from_env` (pub fn) — reads COLMENA_GOOGLE_SHARE_EMAIL and COLMENA_GSHEETS_SCOPES (comma-separated, auto-prefixed with googleapis.com URL) from environment; returns GSheetsConfig with defaults (30s timeout, 3 retries, empty share_email if unset)
- `tests::default_scopes_cover_sheets_and_drive_file` (test fn) — verifies DEFAULT_SCOPES contains both /spreadsheets and /drive.file scope URLs

## File-level notes

- All doc comments are clear and reference OAuth migration context (2026-06-10) and associated modules (google_oauth, google_workspace_prelude).
- `from_env()` intentionally succeeds always; consumers surface NotConfigured errors when OAuth env vars are missing (separation of concerns documented in method doc).
- Scope parsing supports both short names (prefixed automatically) and full URLs (passed through), enabling flexibility.
- Test coverage is minimal (single sanity check on DEFAULT_SCOPES), but config reading itself is tested implicitly via integration tests in gsheets nodes.
