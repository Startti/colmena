# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/sql_bulk_tools.rs

**Layer:** infrastructure  **Purpose:** Provides two synthetic LLM tools for bulk-loading CSV/XLSX attachments (up to 50 MB, 100K rows) into Postgres without buffering all rows in the LLM context. Includes pure parsing logic (T2), schema validation (T3), and dispatcher trampolines (T4) with 240+ lines of comprehensive tests.

## Symbols

### Constants
- `SQL_INSPECT_ATTACHMENT_TOOL_NAME` (pub const) — Tool name constant for attachment inspection
- `SQL_BULK_INSERT_TOOL_NAME` (pub const) — Tool name constant for bulk insert
- `MAX_ATTACHMENT_BYTES` (pub const) — 50 MB hardcoded size ceiling
- `MAX_BULK_INSERT_ROWS` (pub const) — 100K row hardcoded ceiling
- `MAX_INSPECT_SAMPLE_ROWS` (pub const) — 20 row sample maximum
- `DEFAULT_INSPECT_SAMPLE_ROWS` (pub const) — 5 default sample rows
- `DEFAULT_HEADER_ROW` (pub const) — 1-indexed header row default
- `DEFAULT_TABULAR_SUMMARY_SAMPLE_ROWS` (pub const) — 3 rows for auto-summary in catalog block

### Enums
- `InferredType` (pub enum) — Column type inference result (Integer, Numeric, Bool, Date, Timestamp, Text)
  - `as_str()` (pub fn) — String representation of inferred type
- `FileFormat` (pub enum) — Source format (Csv, Xlsx)
  - `from_mime()` (pub fn) — Map MIME type to format or None
  - `from_filename()` (pub fn) — Map filename extension to format or None
- `OnErrorPolicy` (pub enum, Default) — Bulk insert error handling (FailFast default, SkipRows, PartialCommit deferred to v1.1)

### Request/Response Structs
- `InspectArgs` (pub struct) — sql_inspect_attachment arguments (attachment_id, sample_rows, delimiter, sheet_name, header_row, target_table)
- `InspectResponse` (pub struct) — sql_inspect_attachment response (columns, inferred_types, sample, total_rows, format, delimiter, encoding, sheet_name, target_table_schema)
- `TargetTableSchema` (pub struct) — Destination table metadata (table, columns)
- `TargetTableColumn` (pub struct) — Single column from information_schema (name, data_type, is_nullable, column_default)
- `BulkInsertArgs` (pub struct) — sql_bulk_insert_from_attachment arguments (attachment_id, table, column_mapping, on_error, delimiter, header_row, sheet_name)
- `BulkInsertResponse` (pub struct) — sql_bulk_insert_from_attachment response (rows_inserted, rows_skipped, duration_ms, method, errors)
- `BulkInsertRowError` (pub struct) — Per-row failure detail (row_index, message)

### Type Aliases
- `AttachmentRecords` (pub type) — (Vec<String>, Vec<serde_json::Map>) for pandas DataFrame construction

### Parsing Functions (T2 — Pure)
- `build_tabular_summary()` (pub fn) — Build structured one-paragraph summary for CSV/XLSX (zero-LLM-token, fallback-safe)
- `format_tabular_summary()` (fn) — Render InspectResponse as compact summary text
- `parse_inspect_bytes()` (pub fn) — Parse attachment bytes to InspectResponse (wrapper)
- `parse_inspect_bytes_with_filename()` (pub fn) — Parse with filename fallback for MIME resolution
- `parse_attachment_to_records()` (pub fn) — Load full attachment rows as JSON records for python_script
- `parse_csv_to_records()` (fn) — CSV-specific record loading
- `parse_xlsx_to_records()` (fn) — XLSX-specific record loading
- `parse_csv_bytes()` (fn) — CSV parsing branch with sample + type inference
- `parse_xlsx_bytes()` (fn) — XLSX parsing branch with calamine workbook streaming
- `detect_csv_delimiter()` (fn) — Auto-detect delimiter by counting commas/semicolons/tabs
- `cell_to_string()` (fn) — Convert calamine Data cell to string (preserves precision)
- `infer_column_types()` (pub fn) — Infer types per column via sampled-value pattern matching (Bool → Integer → Numeric → Timestamp → Date → Text)
- `is_numeric()` (fn) — Regex-free numeric check (handles negatives and decimals)
- `is_date()` (fn) — Strict YYYY-MM-DD validation
- `is_timestamp()` (fn) — YYYY-MM-DD[T ]HH:MM check

### SQL Functions (T3 — Validation & DB)
- `split_qualified_table()` (pub fn) — Parse schema.table or default to public.table
- `validate_table_against_allowlist()` (pub fn) — Verify schema + table against permissions (alphanumeric + underscore only)
- `build_short_lived_pool()` (pub async fn) — Create single-connection ad-hoc PgPool
- `fetch_target_table_schema()` (pub async fn) — Query information_schema for destination columns
- `quote_ident()` (fn) — Double-quote identifier with " → "" escaping
- `execute_bulk_insert_csv_postgres_copy()` (pub async fn) — Main implementation: COPY FROM STDIN with deterministic column ordering (FailFast-only in v1)

### Tool Definition Functions
- `build_sql_inspect_attachment_tool_definition()` (pub fn) — Create ToolDefinition for inspect tool
- `build_sql_bulk_insert_tool_definition()` (pub fn) — Create ToolDefinition for bulk insert tool

### Dispatcher Functions (T4 — Bridge)
- `resolve_env_vars()` (pub fn) — Replace ${ENV_VAR} in connection strings
- `extract_connection_config()` (fn) — Extract connection_url + permissions.allowed_schemas from fixed_config
- `err_envelope()` (fn) — Wrap error as {"error": msg, "source": "execution"} tool response
- `dispatch_sql_inspect_attachment_via_executor()` (pub async fn) — Trampoline from DagToolExecutor → parse → response
- `dispatch_sql_bulk_insert_from_attachment_via_executor()` (pub async fn) — Trampoline from DagToolExecutor → COPY → response

### Tests
- 45+ unit tests covering:
  - Type inference (Bool, Integer, Numeric, Date, Timestamp, Text edge cases)
  - CSV parsing (delimiter detection, header_row offsets, empty cells)
  - XLSX parsing (sheet selection, boundary conditions)
  - Tabular summary (text/plain fallback, oversized payloads, long cells, empty sheets)
  - Validation (schema allowlist, identifier sanitization, quote escaping)
  - DB integration (#[ignore]-gated by TEST_DATABASE_URL): happy path, column renaming, constraint rollback, schema lookup

## File-level Notes

1. **Intentional deferred features:** XLSX bulk insert (line 1563-1565) and SkipRows/PartialCommit policies (lines 1196-1205) are explicitly rejected with user-facing error messages documenting v1.1 tracking. This is by design, not a bug.

2. **Symbol ordering:** `is_timestamp()` is defined at line 1609 but called from `infer_column_types()` at line 1022. Rust module compilation allows this, but it's organizationally unusual. No functional impact.

3. **Defensive identifier validation:** All schema/table identifiers are validated to `[A-Za-z0-9_]` only (line 1107), then quoted with `"` → `""` escaping (line 1181) before injection into SQL. Defense-in-depth against injection.

4. **Mime type fallback chain:** Storage adapters frequently strip original MIME types (returning `application/octet-stream`), so the code prefers the catalog's stored metadata (line 1479-1481, 1559-1561) over the storage adapter's report. This is documented and tested.

5. **Test coverage is comprehensive:** 45+ tests including pure parsing (no DB), validation, and DB integration. Edge cases like 0-row CSVs, oversized payloads, and constraint violations are covered. Integration tests use `#[ignore]` and `TEST_DATABASE_URL` env-var gating per project convention.

6. **No known issues:** No dead code, no unfinished stubs beyond the documented v1.1 deferrals, no obvious optimizations or safety gaps. Error handling is sound (Result types, string error messages, env-var resolution guards).
