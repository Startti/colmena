# src/libs/colmena/src/gsheets/domain/errors.rs

**Layer:** domain  **Purpose:** Defines the error enum for Google Sheets domain operations; public so dispatchers can map to JSON tool results.

## Symbols

- `SheetsError` (enum, pub) — Top-level error type for Google Sheets operations with 9 variants covering auth, resource access, validation, and network failures
- `SheetsError::NotConfigured` (variant) — No credentials configured (GOOGLE_APPLICATION_CREDENTIALS missing, ADC fallback failed); carries hint string for operator
- `SheetsError::AuthFailed` (variant) — Token acquisition failed during auth flow (network, malformed JSON, etc.)
- `SheetsError::SpreadsheetNotFound` (variant) — Spreadsheet ID doesn't resolve in Google Drive; includes ID for agent to surface to user
- `SheetsError::SheetNotFound` (variant) — Sheet (tab) name unknown within an existing spreadsheet
- `SheetsError::InvalidRange` (variant) — A1 range syntax invalid (e.g. "Foo" instead of "Sheet1!A1:B2")
- `SheetsError::PermissionDenied` (variant) — 403 from Google; carries service account email so agent can tell user to share spreadsheet
- `SheetsError::RateLimit` (variant) — 429 rate-limited response with retry-after seconds
- `SheetsError::Http` (variant) — Network/5xx/timeout errors; free-form message
- `SheetsError::Internal` (variant) — Unexpected internal failure (shouldn't happen in well-tested code)

## File-level notes

- Minimal, focused domain error enum with no infrastructure dependencies
- All variants comprehensively documented with semantic meaning and helpful context for agent/operator handling
- Uses `thiserror::Error` derive for clean Display/Error trait implementation
- Each variant is designed to serialize to JSON tool results with actionable information (hints, retry-after, email hints)
- No unused symbols, unfinished code, or obvious improvements; well-designed error contract for the domain
