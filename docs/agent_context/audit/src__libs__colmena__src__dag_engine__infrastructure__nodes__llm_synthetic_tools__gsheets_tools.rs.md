# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs

**Layer:** infrastructure  
**Purpose:** Exposes 11 Google Sheets operations (create, read, write, share, format) as synthetic LLM tools with JSON-shaped error envelopes, markdown table rendering, and attachment plumbing for XLSX import/export.

## Symbols

### Constants
- `TOOL_CREATE_SPREADSHEET` (pub const &str) — tool alias for creating a new empty spreadsheet
- `TOOL_CREATE_FROM_XLSX` (pub const &str) — tool alias for importing an XLSX file as a new spreadsheet
- `TOOL_EXPORT_XLSX` (pub const &str) — tool alias for exporting a spreadsheet as XLSX bytes
- `TOOL_LIST_SPREADSHEETS` (pub const &str) — tool alias for Drive discovery (Bundle 2A, 2026-06-11)
- `TOOL_SHARE` (pub const &str) — tool alias for granting Drive permissions (Bundle 2B, 2026-06-11)
- `TOOL_LIST_PERMISSIONS` (pub const &str) — tool alias for listing Drive permissions (Bundle 2B)
- `TOOL_UNSHARE` (pub const &str) — tool alias for revoking Drive permissions (Bundle 2B)
- `TOOL_LIST_SHEETS` (pub const &str) — tool alias for listing tabs within a spreadsheet
- `TOOL_ADD_SHEET` (pub const &str) — tool alias for creating a new tab
- `TOOL_DELETE_SHEET` (pub const &str) — tool alias for deleting a tab
- `TOOL_READ` (pub const &str) — tool alias for reading cell ranges as markdown or JSON
- `TOOL_SET_CELL` (pub const &str) — tool alias for setting a single cell value
- `TOOL_SET_RANGE` (pub const &str) — tool alias for bulk-setting a rectangular range
- `TOOL_FORMAT_RANGE` (pub const &str) — tool alias for applying formatting (bold, color, etc.)
- `MARKDOWN_PREVIEW_BUDGET_BYTES` (pub const usize) — byte cap (45 KB) for markdown read results

### Argument Structs (derive Debug, Deserialize, JsonSchema)
- `CreateSpreadsheetArgs` (pub struct) — `title: String`
- `CreateFromXlsxArgs` (pub struct) — `attachment_id: String, title: String`
- `ExportXlsxArgs` (pub struct) — `spreadsheet_id: String`
- `ListSpreadsheetsArgs` (pub struct) — `query, parent_folder_id, modified_after, limit, page_token` (all Optional)
- `ListSheetsArgs` (pub struct) — `spreadsheet_id: String`
- `AddSheetArgs` (pub struct) — `spreadsheet_id: String, name: String`
- `DeleteSheetArgs` (pub struct) — `spreadsheet_id: String, sheet: String` (alias "name")
- `ReadArgs` (pub struct) — `spreadsheet_id, sheet, range, value_render, as_records, format` (mixed required/optional)
- `SetCellArgs` (pub struct) — `spreadsheet_id, sheet, addr (alias "address"), value: serde_json::Value`
- `SetRangeArgs` (pub struct) — `spreadsheet_id, sheet, start_addr (alias "start"), values_2d (alias "values")`
- `FormatRangeArgs` (pub struct) — `spreadsheet_id: String, ops: Vec<FormatOp>`
- `FormatOp` (pub struct) — `sheet: String, range: String, format: FormatSpec`
- `ShareArgs` (pub struct) — `spreadsheet_id, email, role` (reader|commenter|writer)
- `ListPermissionsArgs` (pub struct) — `spreadsheet_id: String`
- `UnshareArgs` (pub struct) — `spreadsheet_id, permission_id`

### Helper Functions
- `error_to_json(e: SheetsError) -> serde_json::Value` (pub(crate) fn) — converts domain errors to JSON with stable shape; special-cases `PermissionDenied` with structured `share_email` + directive `hint`
- `permission_denied_payload(share_email: &str) -> serde_json::Value` (fn) — builds 403 error envelope; degrades gracefully if share email unset
- `build_client() -> Result<GoogleSheetsHttpClient, serde_json::Value>` (fn) — constructs HTTP client from `GSheetsConfig` env vars
- `parse_value_render(s: Option<&str>) -> ValueRenderOption` (fn) — parses FORMULA|FORMATTED_VALUE|UNFORMATTED_VALUE string to enum; defaults to UNFORMATTED_VALUE
- `md_cell(v: &serde_json::Value) -> String` (fn) — renders a single cell value for markdown table; escapes `|` and newlines; handles null, bool, number, string, arrays/objects
- `values_to_markdown(values: &serde_json::Value) -> String` (fn) — converts 2-D array to GitHub-flavored markdown table with header row, separator, padded ragged rows
- `bound_markdown_rows(md: &str, budget: usize) -> (String, usize)` (fn) — keeps header + separator + as many whole data rows as fit within byte budget; always preserves first data row; returns bounded markdown + row count
- `compute_dimensions(values: &serde_json::Value) -> serde_json::Value` (fn) — computes row/column extent from 2-D or record array; returns `{"rows": N, "columns": M}`

### Tool Definition Functions
- `tool_create_spreadsheet() -> ToolDefinition` (pub fn)
- `tool_create_from_xlsx() -> ToolDefinition` (pub fn)
- `tool_export_xlsx() -> ToolDefinition` (pub fn)
- `tool_list_spreadsheets() -> ToolDefinition` (pub fn, Bundle 2A)
- `tool_share() -> ToolDefinition` (pub fn, Bundle 2B)
- `tool_list_permissions() -> ToolDefinition` (pub fn, Bundle 2B)
- `tool_unshare() -> ToolDefinition` (pub fn, Bundle 2B)
- `tool_list_sheets() -> ToolDefinition` (pub fn)
- `tool_add_sheet() -> ToolDefinition` (pub fn)
- `tool_delete_sheet() -> ToolDefinition` (pub fn)
- `tool_read() -> ToolDefinition` (pub fn)
- `tool_set_cell() -> ToolDefinition` (pub fn)
- `tool_set_range() -> ToolDefinition` (pub fn)
- `tool_format_range() -> ToolDefinition` (pub fn)

All 14 definition functions follow the same pattern: call `super::build_synthetic_tool_with_summary::<ArgsType>()` with tool name, description, and summary from the text registry.

### Dispatcher Functions (async)
Each tool has a `dispatch_*_with_client` variant for testability (accepts `&dyn SheetsClient`) and a `dispatch_*` variant (builds client internally).

- `dispatch_list_sheets_with_client(args, client) -> Value` (pub async fn) — parses ListSheetsArgs, calls `client.list_sheets()`, wraps as `{ok: true, sheets: [...]}`
- `dispatch_list_sheets(args) -> Value` (pub async fn) — calls dispatch_list_sheets_with_client with built client
- `dispatch_list_spreadsheets_with_client(args, client) -> Value` (pub async fn, Bundle 2A) — Drive discovery; calls `client.list_spreadsheets()` with filter
- `dispatch_list_spreadsheets(args) -> Value` (pub async fn)
- `dispatch_share_with_client(args, client) -> Value` (pub async fn, Bundle 2B) — validates role (reader|commenter|writer), calls `client.share()`
- `dispatch_share(args) -> Value` (pub async fn)
- `dispatch_list_permissions_with_client(args, client) -> Value` (pub async fn, Bundle 2B) — calls `client.list_permissions()`
- `dispatch_list_permissions(args) -> Value` (pub async fn)
- `dispatch_unshare_with_client(args, client) -> Value` (pub async fn, Bundle 2B) — calls `client.delete_permission()`
- `dispatch_unshare(args) -> Value` (pub async fn)
- `dispatch_create_spreadsheet_with_client(args, client) -> Value` (pub async fn) — calls `client.create_spreadsheet()`, returns id + url + sheets metadata
- `dispatch_create_spreadsheet(args) -> Value` (pub async fn)
- `dispatch_add_sheet_with_client(args, client) -> Value` (pub async fn) — calls `client.add_sheet()`
- `dispatch_add_sheet(args) -> Value` (pub async fn)
- `dispatch_delete_sheet_with_client(args, client) -> Value` (pub async fn) — calls `client.delete_sheet()`
- `dispatch_delete_sheet(args) -> Value` (pub async fn)
- `dispatch_read_with_client(args, client) -> Value` (pub async fn) — auto-expands single A1 cell to A1:A1; renders markdown or JSON; truncates large sheets with `truncated`/`rows_shown`/`total_rows` metadata
- `dispatch_read(args) -> Value` (pub async fn)
- `dispatch_set_cell_with_client(args, client) -> Value` (pub async fn) — calls `client.set_cell()` with CellValue
- `dispatch_set_cell(args) -> Value` (pub async fn)
- `dispatch_set_range_with_client(args, client) -> Value` (pub async fn) — bulk-converts 2-D JSON to CellValue rows, calls `client.set_range()`
- `dispatch_set_range(args) -> Value` (pub async fn)
- `dispatch_format_range_with_client(args, client) -> Value` (pub async fn) — resolves sheet names to sheet IDs; validates ops; builds Sheets API batch requests; calls `client.batch_update()`
- `dispatch_format_range(args) -> Value` (pub async fn)
- `dispatch_create_from_xlsx_via_executor(executor, args) -> Value` (pub async fn) — Bundle 1, 2026-06-10; fetches attachment bytes, uploads to Drive with mime conversion
- `dispatch_export_xlsx_via_executor(executor, args) -> Value` (pub async fn) — exports spreadsheet as XLSX, registers bytes as attachment

### Test Support (module cfg test)
- `FakeClient` (struct) — minimal mock implementing `SheetsClient` trait; only `list_sheets` returns test data, all others return Internal("not used")
- `ReadClient` (struct) — mock with configurable `values` from `read_range()`; all other methods return Internal
- `mock_client_returning(values) -> ReadClient` (fn) — helper to construct ReadClient
- `FormatFake` (struct) — mock for format tests; implements `list_sheets` and `batch_update`, captures requests to Mutex; all other methods unimplemented!()

### Tests
- `dispatch_list_sheets_returns_ok_envelope` (tokio::test) — verifies ok + sheets JSON envelope
- `dispatch_invalid_args_returns_invalid_args_error` (tokio::test) — missing required field triggers invalid_args error
- `parse_value_render_defaults_to_unformatted` (test) — default and explicit FORMULA variants
- `error_to_json_includes_kind_and_message` (test) — error envelope structure
- `permission_denied_payload_emits_share_email_and_directive_hint` (test) — verifies 403 envelope contains email + Spanish directive hint
- `permission_denied_payload_in_degraded_mode_directs_to_operator` (test) — empty share_email falls back to operator reference
- `dispatch_read_defaults_to_markdown_with_dimensions` (tokio::test) — markdown rendering + dimensions metadata
- `dispatch_read_json_format_preserves_values` (tokio::test) — JSON format skips markdown, returns raw values
- `values_to_markdown_renders_table_with_header` (test)
- `values_to_markdown_pads_ragged_rows_and_renders_types` (test) — escaping `|` and newlines; null/bool/number rendering
- `values_to_markdown_empty_is_empty_string` (test)
- `bound_markdown_rows_keeps_header_and_rows_under_budget` (test)
- `bound_markdown_rows_always_keeps_at_least_one_data_row` (test) — even if oversized
- `bound_markdown_rows_header_only_table_unchanged` (test)
- `compute_dimensions_uses_max_width_including_header` (test)
- `format_range_resolves_sheet_and_builds_requests` (tokio::test) — end-to-end format dispatcher
- `format_range_unknown_sheet_errors` (tokio::test) — validation of sheet existence

## File-level notes

- **Subsystem E (2026-06-11)**: 9 core tools + 5 permission/discovery tools (Bundles 2A/2B).
- **UX design**: aliases (`address` ↔ `addr`, `start` ↔ `start_addr`, `values` ↔ `values_2d`) reduce friction; single A1 cells auto-expanded to A1:A1 ranges.
- **Error shapes**: all dispatchers return JSON with `error`/`message` keys or tool-specific `ok` + payload. `PermissionDenied` is special: returns structured `{error, share_email, hint}` with directive Spanish text so LLM reproduces it verbatim.
- **Markdown rendering**: `values_to_markdown()` produces GitHub-flavored tables; large results are row-bounded via `bound_markdown_rows()` to stay under 45 KB while preserving header + at least one data row. Truncation is annotated with `truncated`/`rows_shown`/`total_rows` so the agent sees real columns + sample.
- **Attachment plumbing**: `dispatch_create_from_xlsx_via_executor` and `dispatch_export_xlsx_via_executor` (Bundle 1, 2026-06-10) orchestrate the attachment lifecycle (fetch/register) via DagToolExecutor; live dispatchers without executor variants (create/list/add/delete/read/set/format) are wired in the router (E-T7).
- **Role normalization**: `dispatch_share` validates role string (case-insensitive) and maps to domain enum before calling client.
- **Dimension tracking**: every read result includes `{rows, columns}` metadata so the agent understands data extent without parsing markdown.
- **Test coverage**: comprehensive unit tests for error handling, markdown rendering (including escaping, ragged rows, truncation), dispatch envelopes, and format resolution. Mock clients follow trait boundaries; no real HTTP calls in tests.
- **Documentation**: all tool names, arg structs, and helpers are marked `pub` (exposed to router). Tool definitions call `text::tool_description()` and `text::tool_summary()` to fetch descriptions from the text registry (YAML/Markdown).
