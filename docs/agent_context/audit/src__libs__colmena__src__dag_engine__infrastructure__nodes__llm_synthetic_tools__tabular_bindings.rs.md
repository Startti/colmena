# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/tabular_bindings.rs

**Layer:** infrastructure  
**Purpose:** Provides polymorphic tabular-binding types and concurrent resolver for the `data_run_python` synthetic LLM tool. Handles four data sources (attachment CSV/XLSX, Google Sheets, SQL SELECT, inline JSON), validates bindings, and resolves them to sandbox-ready inputs.

## Symbols

- `DataBinding` (pub struct) — Deserializable representation of one Python variable with exactly one of four optional data sources; supports UX aliases (`binding_name`, `name`) on `var` field.
- `BindingKind` (pub enum) — Four-variant discriminator for the structural source: Attachment, Gsheets, Sql, Inline.
- `SqlBindingCtx` (pub struct) — Bundle of SQL adapter, permissions, allowed schemas, and tenant user ID for SQL binding resolution.
- `AttachmentFetcher<'a>` (pub type alias) — Closure type for fetching attachment bytes by ID, returns `(bytes, mime_or_filename)`.
- `ResolvedBindings` (pub struct) — Output of concurrent resolution: sandbox inputs map, column metadata, SQL snapshots, and gsheets load snapshots for diff-driven write-back.
- `SQL_BINDING_MAX_ROWS` (const) — Cap of 100,001 rows for SQL binding fetch (one above documented limit to detect overflow).
- `SQL_BINDING_ROW_LIMIT` (const) — Documented hard limit of 100,000 rows per SQL binding.
- `classify_binding` (pub fn) — Classifies a binding by structural inspection of discriminator fields; returns BindingKind or ambiguity error.
- `validate_bindings` (pub fn) — Validates full binding list: non-empty, non-empty trimmed vars, no duplicates, each classifies successfully; returns structured JSON error envelope on failure.
- `resolve_bindings` (pub async fn) — Concurrent resolver via `join_all` that fetches all bindings in parallel (inline/attachment/gsheets/SQL dispatch), assembles sandbox inputs and metadata.
- `deserialize_bindings_flexible` (pub fn) — Custom serde deserializer accepting canonical array form or LLM-hallucinated dict form (key=var, value=binding object); rejects bare strings with clear error.
- `ResolvedOne` (type alias, private) — Internal tuple `(var, records, optional_sql_snapshot, optional_gsheets_snapshot_info)`.
- `binding_error` (fn, private) — Builds structured JSON error envelope with `error`, `binding`, and extra fields.
- `extract_columns` (fn, private) — Pulls column names from first record of a records array; mirrors `gsheets_run_python::extract_columns`.
- `normalize_inline_data` (fn, private) — Converts 2-D array (first row = header) to records via `rectangle_to_records`; keeps array-of-objects as-is.
- `detect_plain_select_star_table` (fn, private) — Best-effort AST-based detection of plain `SELECT * FROM <table>` (no WHERE/JOIN/GROUP BY/HAVING) for snapshot retention; returns qualified table name or None.
- `resolve_one` (async fn, private) — Core dispatcher resolving a single binding to `(var, records, snapshot, gsheets_info)` by BindingKind; includes SQL SELECT-only gate, permission validation, row cap enforcement.
- `NeverCalledAdapter` (struct, test-only) — Mock `SqlConnectionPort` that panics on `execute_query`, proving SELECT-only gate rejects bad bindings before DB access.
- `b` (fn, test-only) — Helper deserializing DataBinding from serde_json::Value.
- Multiple test functions (lines 671–896) — Verify classification, validation, concurrent resolution, inline normalization, attachment fetching, SQL SELECT gate, gsheets/SQL context guards, and error propagation.

## File-level notes

- **Async design:** All bindings resolve concurrently via `join_all`, supporting multi-source workloads in parallel.
- **Error handling:** Every path produces structured JSON error envelopes with `"binding": <var>` so LLM/dispatcher can pinpoint failures.
- **Security:** SQL bindings enforce SELECT-only via AST classification before validator/execute_query; CTE-wrapped DELETE regression test verifies gate.
- **Snapshot retention:** SQL `SELECT * FROM <table>` and Gsheets bindings retain records for diff-driven write-back (update_by_position, UPDATE).
- **Flexibility:** Deserializer tolerates LLM hallucinations (dict form with var in key) without schema change; rejects ambiguous bare strings.
- **Column tracking:** Both `inputs` and `loaded_columns` populated; `_loaded_columns` injected into sandbox for LLM reference.
- **No external crate imports for core logic:** Uses only domain traits (SqlConnectionPort, SheetsClient) and standard library; sql_ast parsing delegated to dedicated module.
- **Test coverage:** 11 async tests covering happy paths, error gates, concurrent dispatch, and attachment/SQL/gsheets/inline sources.
