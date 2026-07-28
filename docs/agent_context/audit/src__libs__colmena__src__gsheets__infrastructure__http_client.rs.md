# src/libs/colmena/src/gsheets/infrastructure/http_client.rs

**Layer:** infrastructure  
**Purpose:** HTTP/REST adapter implementing the `SheetsClient` trait against Google's Sheets API v4 and Drive API v3. Handles OAuth bearer authentication, exponential backoff retry logic (429/5xx/401 refresh), and response parsing into domain value objects.

## Symbols

### Struct & Impl
- `GoogleSheetsHttpClient` (struct, pub) — HTTP client wrapper with OAuth token management, retry configuration, and API base URL overrides for testing.

### Impl GoogleSheetsHttpClient Methods

**Production & Test Constructors:**
- `from_config(cfg: &GSheetsConfig) -> Result<Self, SheetsError>` — Creates instance from config, reading OAuth credentials from env and validating all required vars before boot.
- `for_tests(sheets_base, drive_base, drive_upload_base) -> Self` — Test-only constructor pointing at wiremock server with static token and 50ms retry delays.
- `token_test_seed(&self, token) -> async` — Test helper to seed sticky bearer token on internal TokenProvider.

**Private HTTP Verb Helpers:**
- `get_json(&self, url) -> async Result<Value>` — Bearer-auth GET with retry on 429/5xx and 401-refresh-then-retry.
- `put_json(&self, url, body) -> async Result<Value>` — Bearer-auth PUT with retry on 429/5xx and 401-refresh-then-retry; handles BAD_REQUEST specially for range/sheet-not-found cases.
- `post_json(&self, url, body) -> async Result<Value>` — Bearer-auth POST with retry on 429/5xx and 401-refresh-then-retry.
- `parse_meta(value: &Value, default_id) -> Result<SpreadsheetMeta>` — Extracts id/title/url/sheets array from Sheets API metadata response; supports optional fallback spreadsheetId for Drive responses.

### SheetsClient Trait Implementation (async)
- `read_range(&self, id, sheet, range, opts) -> Result<ReadResponse>` — Fetches grid data via `spreadsheets.get` with `includeGridData=true`, parses value blocks per render option (formatted/unformatted/formula), forward-fills merged cells via `forward_fill_merges`, returns 2D array or records format.
- `list_sheets(&self, id) -> Result<Vec<SheetMeta>>` — Calls `spreadsheets.get` and extracts metadata (id/title/index/row_count/col_count) for each sheet.
- `set_cell(&self, id, sheet, addr, value) -> Result<()>` — Writes single cell via `values/{range}` PUT with USER_ENTERED option.
- `set_range(&self, id, sheet, start_addr, values_2d) -> Result<SetRangeResponse>` — Writes 2D block via `values/{range}` PUT, returns updated cell count and final range.
- `create_spreadsheet(&self, title) -> Result<SpreadsheetMeta>` — POSTs new spreadsheet to Sheets API, parses response into metadata.
- `create_from_xlsx(&self, title, bytes) -> Result<SpreadsheetMeta>` — Uploads XLSX file to Drive via multipart upload, then fetches metadata from Sheets API using returned file ID.
- `export_xlsx(&self, id) -> Result<Vec<u8>>` — Downloads spreadsheet as XLSX bytes via Drive `/export` endpoint.
- `share(&self, id, email, role) -> Result<()>` — Creates permission on Drive file via `permissions` POST.
- `list_permissions(&self, id) -> Result<PermissionList>` — Fetches all permissions (users/groups/roles) from Drive file.
- `delete_permission(&self, id, permission_id) -> Result<()>` — Removes permission via Drive `permissions/{id}` DELETE with retry logic (401/429/5xx).
- `list_spreadsheets(&self, filter) -> Result<SpreadsheetListResult>` — Queries Drive with `q` filter (name/parent/modifiedTime) and pagination, returns file metadata.
- `get_modified_time(&self, id) -> Result<Option<String>>` — Fetches `modifiedTime` timestamp from Drive file metadata.
- `add_sheet(&self, id, name) -> Result<SheetMeta>` — Calls `batchUpdate` with `addSheet` request, returns new sheet metadata.
- `delete_sheet(&self, id, name_or_sheet_id) -> Result<()>` — Resolves sheet name to numeric ID if needed (via `list_sheets`), then calls `batchUpdate` with `deleteSheet`.
- `batch_update_cells(&self, id, sheet, updates) -> Result<SetRangeResponse>` — Batches multiple cell updates via `values:batchUpdate` request, returns total cells updated.
- `batch_update(&self, id, requests) -> Result<()>` — Low-level `batchUpdate` for arbitrary Sheets API requests (formatting, merges, etc.).

### Module-level Functions
- `rectangle_to_records(rect: &Value) -> Value` (pub(crate)) — Converts [[header_row], [data_row1], ...] into [{header: v1, ...}, ...]; skips empty-header columns, pads missing cells as Null.
- `quote_sheet_for_range(sheet: &str) -> String` (private) — Wraps sheet name in single quotes if non-alphanumeric; escapes internal quotes by doubling per Google range syntax.
- `extended_value_to_scalar(ev: &Value) -> Value` (private) — Extracts scalar from Google ExtendedValue one-of (numberValue | stringValue | boolValue | formulaValue | errorValue).
- `cell_scalar(cell: &Value, render: ValueRenderOption) -> Value` (private) — Maps grid CellData to scalar JSON, honoring render option (FormattedValue uses string; UnformattedValue uses effective; Formula prefers formula text).
- `parse_grid_block(body: &Value, render: ValueRenderOption) -> (Vec<Vec<Value>>, usize, usize)` (private) — Parses first sheet's first GridData block into rectangular 2D array; pads rows to uniform width; returns (grid, row_offset, col_offset).
- `parse_merges(body: &Value) -> Vec<MergeRect>` (private) — Extracts merge rectangles from `sheets[0].merges` array.

### Tests
- `mod tests` (cfg(test)) — 13+ unit tests via wiremock mocking API responses, covering: read/write/create/delete/share/export/list flows, error cases (404/403/401/429), merge forward-fill, records conversion, sheet quoting.
- `setup_mock() -> (MockServer, GoogleSheetsHttpClient)` — Test fixture.
- Individual test functions exercising all major paths.

## File-level notes

- **Retry logic duplication**: The three HTTP verb helpers (`get_json`, `put_json`, `post_json`) and `delete_permission`'s inline DELETE loop share nearly identical retry + status-handling logic (~160+ lines of duplicated code across 4 locations). The retry formula `retry_base_delay * (1 << attempt)` appears 8 times. This is not a bug but a refactoring opportunity — could extract a single `retry_json_request(http_method, url, body?)` helper.
- **Approach B merge-aware reads**: Comments at line 333-336 reference spec `2026-06-14-gsheets-expand-merges-design.md`; the read path uses `spreadsheets.get` with `includeGridData=true` in a single round-trip, avoiding stale cache issues vs. a separate merges fetch.
- **Comment at line 649**: Acknowledges lack of `delete_json` helper; DELETE retry is duplicated inline.
- **Range validation on BAD_REQUEST**: `put_json` (line 176-179) parses response text to distinguish "Unable to parse range" (maps to SheetNotFound) from other 400 errors (maps to InvalidRange). Clean error mapping.
- **OAuth credential validation at startup**: `from_config` calls `OAuthCredentials::from_env()` which collects all missing vars upfront (line 51-52), surfacing one clear error per boot rather than discovering secrets piecemeal.
