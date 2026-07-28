# src/libs/colmena/src/gsheets/domain/types.rs

**Layer:** domain  
**Purpose:** Define all value types (DTOs, enums, wrappers) used across the Google Sheets API surface. Provides serializable domain models for spreadsheet metadata, cell values, sharing permissions, and list operations.

## Symbols

- `SpreadsheetId` (struct, pub) — Newtype wrapper for Google's stable spreadsheet identifier (string)
- `impl Display for SpreadsheetId` — Display trait for rendering spreadsheet ID as string
- `SheetId` (struct, pub) — Newtype wrapper for Google's internal numeric sheet (tab) identifier (i64)
- `CellValue` (enum, pub) — Untagged enum representing cell contents: Null, Bool, Number (f64), or String
- `impl CellValue` (pub methods):
  - `from_json` — Converts serde_json::Value to CellValue (arrays/objects collapse to Null)
  - `to_json` — Converts CellValue back to serde_json::Value (inverse of from_json)
- `ValueRenderOption` (enum, pub) — Read directive: FormattedValue (locale string), UnformattedValue (raw), or Formula (formula text)
- `impl ValueRenderOption` — `as_api_str` returns wire-format string for Sheets API `valueRenderOption` query param
- `ReadOptions` (struct, pub) — Configuration for read calls: value_render mode and as_records flag (rectangular vs keyed output)
- `impl Default for ReadOptions` — Defaults to UnformattedValue and rectangular (as_records=false)
- `ReadResponse` (struct, pub) — Result envelope for read operations: sheet name, range, and JSON values (array or records)
- `SetRangeResponse` (struct, pub) — Result envelope for write operations: updated cell count and range affected
- `SheetMeta` (struct, pub) — Metadata for one tab: sheet_id, title, index, row/col counts
- `SpreadsheetMeta` (struct, pub) — Metadata for whole spreadsheet: id, title, URL, and vector of SheetMeta
- `ShareRole` (enum, pub) — Drive permission role enum: Reader, Commenter, or Writer
- `impl ShareRole` — `as_api_str` returns wire-format string for Drive Permissions API
- `PermissionEntry` (struct, pub) — One Drive permission: id, type (user/domain/anyone), role, optional email/display_name
- `PermissionList` (struct, pub) — Wrapper for vector of PermissionEntry
- `SpreadsheetListItem` (struct, pub) — One entry in list_spreadsheets response: id, name, URL, modified_time, owners
- `SpreadsheetListResult` (struct, pub) — Result envelope: vector of items and optional next_page_token (pagination)
- `SpreadsheetListFilter<'a>` (struct, pub) — Query filter builder (borrowed strings): query, parent_folder_id, modified_after, limit, page_token
- `mod tests` (cfg test) — Unit test module with three round-trip and edge-case tests

## File-level notes

- No infrastructure or external dependencies beyond serde/serde_json serialization
- All public types properly documented with inline comments explaining Google Sheets API mappings
- Symmetric design with gdocs subsystem (ShareRole, PermissionEntry, etc.)
- `CellValue::from_json` silently coerces unparseable JSON numbers to 0.0 (edge case in Number::as_f64 fallback on line 45) — may be intentional for cell coercion but worth noting
- Test coverage for CellValue round-trips (JSON round-trip, array/object-to-null, ValueRenderOption API strings)
- Clean hexagonal architecture: pure domain layer with no application/infrastructure coupling
