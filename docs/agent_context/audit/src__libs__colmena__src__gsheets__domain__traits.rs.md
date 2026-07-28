# src/libs/colmena/src/gsheets/domain/traits.rs

**Layer:** domain  
**Purpose:** Defines the `SheetsClient` port trait (hexagonal architecture) that abstracts all Google Sheets operations. Any infrastructure implementation must satisfy this interface to support spreadsheet creation, reading, writing, permissions, and sharing.

## Symbols

- `SheetsClient` (trait, pub) — Async port trait (Send + Sync) for Google Sheets operations; all 16 methods must be implemented by infrastructure adapters
- `create_spreadsheet` (async fn, pub) — Creates a new spreadsheet with a title; returns spreadsheet metadata
- `create_from_xlsx` (async fn, pub) — Creates a spreadsheet from XLSX binary data
- `export_xlsx` (async fn, pub) — Exports a spreadsheet to XLSX byte vector
- `share` (async fn, pub) — Grants access to a spreadsheet (Bundle 2B, 2026-06-11); uses Drive `permissions.create`
- `list_permissions` (async fn, pub) — Lists all Drive permissions on a spreadsheet (Bundle 2B, 2026-06-11)
- `delete_permission` (async fn, pub) — Revokes a permission from a spreadsheet (Bundle 2B, 2026-06-11); uses Drive `permissions.delete`
- `list_spreadsheets` (async fn, pub) — Discovers spreadsheets visible to the OAuth user via Drive API; symmetric to `DocsClient::list_documents`
- `get_modified_time` (async fn, pub) — Gets RFC 3339 last-modified timestamp for collision detection in `SheetExists` envelope; best-effort with graceful degradation
- `list_sheets` (async fn, pub) — Lists all sheets within a spreadsheet
- `add_sheet` (async fn, pub) — Adds a new sheet to a spreadsheet; returns sheet metadata
- `delete_sheet` (async fn, pub) — Deletes a sheet by name or stringified numeric sheet ID; implementations resolve both formats
- `read_range` (async fn, pub) — Reads a range of cells with options (ReadOptions); returns ReadResponse
- `set_cell` (async fn, pub) — Sets a single cell value at A1-style address
- `set_range` (async fn, pub) — Sets a 2D rectangular range of cell values; returns SetRangeResponse
- `batch_update_cells` (async fn, pub) — Applies N cell-level writes in one HTTPS round-trip via `values.batchUpdate`; each tuple is (A1-address, value)
- `batch_update` (async fn, pub) — Applies N `spreadsheets.batchUpdate` requests (formatting, borders, dimensions); distinct from `batch_update_cells`

## File-level notes

- Well-structured domain port with 16 cohesive methods covering spreadsheet CRUD, access control, Drive discovery, and formatting
- All methods are actively used by infrastructure implementations and LLM tools
- Comments clearly document purpose, Drive API endpoints, and feature bundles (e.g., "Bundle 2B (2026-06-11)")
- No dead code, unfinished work, or obvious refactoring opportunities detected
