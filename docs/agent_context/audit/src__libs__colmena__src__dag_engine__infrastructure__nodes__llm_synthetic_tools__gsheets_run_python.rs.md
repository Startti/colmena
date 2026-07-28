# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs

**Layer:** infrastructure  
**Purpose:** Implements the `gsheets_run_python` LLM tool that executes sandboxed Python code over Google Sheets data, keeping rows out of the LLM context by fetching sheet bindings in parallel and injecting them directly into the sandbox.

## Symbols

### Constants
- `TOOL_GSHEETS_RUN_PYTHON` (const, pub) — tool name identifier for registry
- `GSHEETS_PY_PRELUDE` (const) — Python prelude (pandas/numpy/scipy imports) injected before user code
- `GSHEETS_PY_POSTLUDE` (const) — Python postlude (wraps globals into output/output_sheets envelope)
- `OUTPUT_BYTE_CAP` (const) — response output truncation limit (10 KB)
- `STDOUT_BYTE_CAP` (const) — response stdout truncation limit (10 KB)
- `ERROR_BYTE_CAP` (const) — response error truncation limit (10 KB)
- `CODE_TIMEOUT_SECS` (const) — sandbox execution timeout (30 seconds)

### Structs
- `GsheetsRunPythonArgs` (struct, pub) — tool arguments: bindings, code, write_to_spreadsheet, output_sheets, on_existing_sheet
- `GsheetsBinding` (struct, pub) — single binding: var, spreadsheet_id, sheet, range, data (supports sheet or inline data source)
- `BindingsVisitor` (struct, private) — Visitor impl for custom serde deserializer supporting canonical array and LLM-hallucinated dict forms

### Functions (non-test)
- `deserialize_bindings_flexible()` (fn, private) — custom serde Visitor that tolerates both array form `[{var,ss_id,sheet}]` and dict form `{var: {ss_id, sheet}}` for LLM flexibility
  - `BindingsVisitor::visit_seq()` — deserialize canonical array form
  - `BindingsVisitor::visit_map()` — deserialize LLM-hallucinated dict form with key→var merge
- `tool_gsheets_run_python()` (fn, pub) — builds ToolDefinition with schema and summary for lazy tool loading
- `dispatch_gsheets_run_python()` (fn, pub async) — production entry point: creates GoogleSheetsHttpClient from env (GOOGLE_APPLICATION_CREDENTIALS/ADC) and dispatches
- `dispatch_gsheets_run_python_with_client()` (fn, pub async) — core dispatcher: validates args, fetches sheet bindings in parallel, injects into sandbox, handles write_to_spreadsheet, enforces caps, returns structured response
- `wrap_user_code()` (fn, private) — wraps user code with prelude and postlude
- `extract_columns()` (fn, private) — pulls column names from first record of Google response
- `truncate()` (fn, private) — truncates string to byte cap with UTF-8 boundary safety
- `truncate_json()` (fn, private) — truncates JSON value to byte cap with UTF-8 safety and returns (truncated_value, was_truncated) flag

### Tests (28 total)
- `mock_with_two_sheets()` — wiremock setup: two mock sheets (Products, Sales) with Approach B (includeGridData)
- `dispatch_runs_pandas_over_two_bindings_in_parallel()` — parallel sheet fetch with len() checks on output
- `python_keyerror_returns_loaded_columns_for_self_correction()` — KeyError → error + loaded_columns in response for LLM self-correction
- `empty_bindings_returns_invalid_args()` — reject empty bindings array
- `duplicate_binding_var_is_rejected()` — reject duplicate var names
- `binding_accepts_binding_name_alias()` — `binding_name` alias for `var`
- `binding_accepts_name_alias()` — `name` alias for `var`
- `binding_canonical_var_still_works()` — canonical `var` field still accepted
- `args_accept_output_sheets_as_tool_arg_silently()` — tool arg `output_sheets` parsed without error (warning issued by dispatcher)
- `tool_def_has_summary()` — verifies lazy_tool_loading summary present and within bounds
- `bindings_canonical_array_form_works()` — canonical `[{var,ss_id,sheet}]` array parses
- `bindings_dict_form_accepted_with_objects()` — LLM dict form `{var: {ss_id,sheet}}` parses (E-T22b)
- `bindings_dict_with_string_value_rejected()` — dict with bare string value (ambiguous sheet) rejected with clear error
- `bindings_dict_form_works_with_binding_name_alias()` — dict form + `sheet_name` alias combined
- `multi_sheet_write_back_three_tabs()` — script assigns `output_sheets={name: DataFrame}`, write_to_spreadsheet set → 3 add_sheet + 3 set_range calls, `wrote_sheets` array populated
- `multi_sheet_collision_retries_with_suffix()` — add_sheet 400 "already exists" → retry with " (2)" suffix, `name` + `resolved_name` in response
- `update_in_place_writes_only_changed_cells()` — `output_sheets={'name': {mode: update_in_place, df, key}}` → diff-write via batchUpdate
- `replace_mode_default_fail_returns_sheet_exists()` — collision default `fail` → structured `SheetExists` error with `current_state`, `advice`, `valid_next_moves`
- `test_client_empty()` — minimal mock client for inline-data validation tests
- `binding_inline_data_parses()` — inline `data: [{}]` binding parses
- `dispatch_rejects_binding_with_both_sources()` — reject binding with both spreadsheet_id + data
- `dispatch_rejects_binding_with_no_source()` — reject binding with neither source
- `dispatch_inline_data_binding_reaches_sandbox()` — inline data (array of dicts) injected into sandbox without sheet fetch
- `dispatch_inline_2d_array_is_converted_to_records()` — 2-D array `[[header], [row]]` converted to records via `rectangle_to_records()`
- `header_range_spans_past_column_z()` — `a1_addr()` correctly spans past column Z (A1:AD1 for 30-column sheet)
- `fail_envelope_has_wide_columns_and_last_modified()` — `SheetExists` envelope surfaces all 30 column names + `last_modified` from Drive
- `auto_suffix_policy_preserves_old_behavior()` — `on_existing_sheet: auto_suffix` collision writes with " (2)" suffix silently

## File-level notes

- **Architecture**: Fully compliant with hexagonal infrastructure layer — all external integrations (SheetsClient) are trait-based; domain errors use `SheetsClient` trait boundary
- **Error handling**: Comprehensive validation before fetch: empty bindings, duplicate vars, source exclusivity (exactly one of sheet or data per binding), non-empty var names. All error paths return structured JSON with descriptive messages.
- **Concurrency**: Sheet fetches are parallelized via `futures::future::join_all()` — bindings are fetched concurrently, not sequentially.
- **Python sandboxing**: Uses `execute_sandboxed_helper()` from `python_node` module with 30-second timeout; prelude/postlude wrap user code to provide imports and output collection.
- **Flexible deserialization**: Custom `deserialize_bindings_flexible()` accommodates both canonical `[{var,ss_id,sheet}]` and LLM-hallucinated `{var: {ss_id,sheet}}` forms, with clear rejection of ambiguous forms (bare strings).
- **Output safety**: Three separate byte caps (output, stdout, error) truncate responses with UTF-8 boundary safety; truncation flags included in response.
- **Write-back modes**: Multi-sheet write-back with two modes: `replace` (collision policies: fail/auto_suffix/overwrite) and `update_in_place` (diff-write via `diff_writer.rs`). Collision retry via `parse_policy()` from `sheet_collision.rs`.
- **Test coverage**: 28 tests covering arg parsing, parallel fetch, error self-correction, inline data, binding aliases, dict deserialization, multi-sheet write-back with collision retry, update_in_place, wide sheets, and policy preservation. Tests use wiremock and conditionally skip if pandas not installed.
- **Deprecation note**: This tool (`gsheets_run_python`) is soft-deprecated as of 2026-07-02 in favor of `data_run_python` (unified CSV/XLSX/Sheets/SQL tool). Still registered and maintained for backward compatibility with persisted graphs; real deletion deferred to Phase 2.
