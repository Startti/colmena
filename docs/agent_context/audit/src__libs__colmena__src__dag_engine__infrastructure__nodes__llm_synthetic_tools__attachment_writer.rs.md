# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/attachment_writer.rs

**Layer:** infrastructure  
**Purpose:** Serializes tabular data (records as JSON objects) to CSV or XLSX bytes and registers them as conversation attachments via an async registrar callback. Provides the sink-side counterpart to the fetcher pattern in `tabular_bindings.rs`.

## Symbols

### Type Aliases
- `AttachmentRegistrar<'a>` (type, pub) — Boxed async function type that accepts a filename and byte buffer, registers them as an attachment, and returns a `document_id` string; mirrors the fetcher shape for consistent builder patterns.

### Constants
- `MAX_ROWS` (const, private) — Quota: maximum 100,000 records per attachment; enforced before serialization.
- `MAX_BYTES` (const, private) — Quota: maximum 50 MB serialized size per attachment; enforced after serialization.

### Public Functions
- `serialize_records()` (pub fn) — Serializes a slice of JSON object records to CSV or XLSX bytes, supporting optional CSV delimiter configuration; entry point for format-agnostic serialization.
- `write_output_attachments()` (pub async fn) — Top-level handler that iterates a dict of `filename -> spec` entries, serializes each to bytes, enforces quotas, registers via callback, and returns structured success or fail-fast error JSON with metadata.

### Private Helper Functions
- `collect_columns()` (fn) — Extracts unique column names from records in first-seen order; preserves deterministic column ordering.
- `value_to_csv_field()` (fn) — Converts a JSON value to a CSV field string representation: null/None → empty, strings/numbers/bools as-is, objects/arrays as minified JSON.
- `serialize_csv()` (fn) — Writes records to CSV format using the `csv` crate with configurable delimiter; validates delimiter is a single byte.
- `serialize_xlsx()` (fn) — Writes records to XLSX format using `rust_xlsxwriter`, with type-aware cell methods (numbers as numeric, bools as boolean, objects/arrays as JSON strings).
- `format_from_name()` (fn) — Infers format from filename extension (.csv or .xlsx); case-insensitive, returns `None` for unrecognized extensions.
- `extract_spec()` (fn) — Parses a spec value (either bare array or `{df: [...], delimiter?: string}`) and returns records and optional delimiter; returns structured `Value::Object` errors on parse failure.

### Test Module
- `tests` (mod, private) — Contains 8 tests (5 async, 3 sync) covering serialization, quota enforcement, and E2E attachment registration.

#### Test Functions
- `serialize_csv_has_header_and_rows()` — Verifies CSV output contains header row and data rows.
- `serialize_xlsx_is_nonempty_zip()` — Validates XLSX magic bytes (PK zip header).
- `serialize_unknown_format_errors()` — Confirms unsupported formats return an error message.
- `serialize_csv_custom_delimiter()` — Tests custom delimiter configuration (e.g., semicolon).
- `serialize_csv_handles_null_bool_nested()` — Exercises null values, booleans, and nested objects in CSV serialization.
- `mock_registrar()` (fn) — Helper that returns a mock registrar closure for async tests; generates deterministic document_id from filename and byte length.
- `write_output_attachments_bare_array_csv()` — E2E test of CSV registration with bare array spec.
- `write_output_attachments_df_spec_xlsx()` — E2E test of XLSX registration with `{df, delimiter}` spec.
- `write_output_attachments_invalid_extension()` — Verifies error on unsupported file extension.
- `write_output_attachments_too_many_rows()` — Confirms quota enforcement at row count limit (100k+1 rows).
- `write_output_attachments_registration_failure()` — Verifies error propagation when registrar fails.

## File-level notes

- **Determinism:** Column ordering is deterministic (first-seen order via HashSet insertion tracking), ensuring stable output across runs.
- **Quota enforcement:** Dual checks (row count + byte size) are applied before registration to fail fast and provide structured error context.
- **Error handling:** All error paths return structured JSON (either from fail-fast checks or via `extract_spec` parse errors), with the first failure propagated immediately (fail-fast pattern).
- **Type handling:** Nested objects and arrays in CSV/XLSX are serialized as minified JSON strings; null/missing fields become empty cells.
- **Async boundary:** The registrar callback is `Send + Sync + 'a`, enabling both blocking and async implementations; caller supplies the concrete registration logic.
- **Spec flexibility:** Supports both bare array and `{df, delimiter?}` object syntax for input, inferred at runtime.
- **Test coverage:** Good coverage of serialization, error cases, and E2E flows; no tests for serde_json serialization failures (theoretically rare for already-deserialized Value types).
