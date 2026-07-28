# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/data_run_python.rs

**Layer:** infrastructure  
**Purpose:** Provides the `data_run_python` LLM tool, a unified cross-store sandbox tool that binds tabular data from attachments, Google Sheets, SQL, or inline JSON and runs sandboxed Python over them.

## Symbols

### Constants
- `TOOL_DATA_RUN_PYTHON` (const, pub) — tool name string "data_run_python"
- `DATA_PY_PRELUDE` (const) — embedded markdown file with Python sandbox prelude (pandas, numpy, scipy imports)
- `DATA_PY_POSTLUDE` (const) — embedded markdown file with Python sandbox postlude (packages output/output_tables/output_sheets/output_attachments)
- `CODE_TIMEOUT_SECS` (const) — 30-second wall-clock cap for sandbox execution
- `OUTPUT_BYTE_CAP` (const) — 10 KB cap on surfaced output/stdout strings

### Functions
- `wrap_user_code(user_code: &str) -> String` — wraps user Python with prelude/postlude
- `enabled_sources(fixed: &HashMap<String, Value>, agent_has_gsheets: bool) -> EnabledSources` — derives which sources (SQL, gsheets) are available from fixed_config and agent capabilities
- `parse_sql_config(fixed: &HashMap<String, Value>) -> Result<Option<SqlSinkConfig>, String>` — parses and validates optional SQL block from fixed_config; errors on misconfiguration, returns None when absent
- `truncate(s: &str, cap: usize) -> String` — truncates string to byte cap on UTF-8 boundary, appending `[truncated]` marker
- `cap_output(value: &Value, cap: usize) -> (Value, bool)` — caps output value to byte limit; returns (capped_value, was_truncated)
- `dispatch_core(args: Value, enabled: &EnabledSources, sql: Option<&SqlRuntime>, sheets: Option<&Arc<dyn SheetsClient>>, attach: &AttachmentFetcher<'_>, registrar: &AttachmentRegistrar<'_>) -> Value` (async, pub) — testable orchestration seam; parses args, resolves bindings, runs wrapped Python, applies write sinks (output_tables/output_sheets/output_attachments)
- `dispatch_data_run_python_via_executor(exec: &DagToolExecutor, tool_call: &ToolCall, fixed: &HashMap<String, Value>) -> Value` (async, pub) — executor entry point; builds SQL runtime, sheets client, attachment fetcher/registrar from live executor
- `mime_from_name(name: &str) -> String` — derives MIME type (text/csv or application/vnd.openxmlformats...) from filename extension; defaults to application/octet-stream
- `render_gated_description(base: &str, enabled: &EnabledSources) -> String` — strips capability-gated blocks (marked `<!--SQL-->`/`<!--/SQL-->` and `<!--SHEETS-->`/`<!--/SHEETS-->`) from description based on enabled sources
- `tool_data_run_python(enabled: &EnabledSources) -> ToolDefinition` (pub) — builds ToolDefinition, appending dynamically-assembled "Available sources" section based on enabled capabilities

### Structs
- `DataRunPythonArgs` (struct, pub) — deserialized args: bindings (Vec<DataBinding>), code (String), write_to_spreadsheet (Option<String>), on_existing_sheet (Option<String>)
- `EnabledSources` (struct, pub) — capability gating: sql (bool), gsheets (bool)
- `SqlSinkConfig` (struct, pub) — parsed & validated SQL block: connection_url, permissions, allowed_schemas, statement_timeout_ms, work_mem_mb, on_missing_table, on_existing_table, tenant_user_id
- `SqlRuntime` (struct, pub) — fully-resolved SQL runtime: adapter (Arc<dyn SqlConnectionPort>), pool (Arc<sqlx::PgPool>), permissions, allowed_schemas, tenant, on_missing_table, on_existing_table, statement_timeout_ms, work_mem_mb

### Test Helpers (private, in tests module)
- `tests::noop_attach()` — returns a no-op attachment fetcher for tests
- `tests::noop_registrar()` — returns a no-op registrar for tests
- `tests::dispatch_rejects_unconfigured_sql_source()` — verifies SQL binding fails when SQL is not enabled
- `tests::dispatch_inline_binding_happy_path_no_sinks()` (ignored) — verifies inline data execution; marked ignored due to PyO3 per-process interpreter parallelism flaking
- `tests::dispatch_coerces_raw_numpy_scalar_output()` (ignored) — verifies numpy scalar coercion in postlude
- `tests::dispatch_coerces_series_and_nested_numpy_output()` (ignored) — verifies Series/nested numpy coercion
- `tests::tool_definition_lists_only_enabled_sources()` — verifies tool description lists only enabled sources
- `tests::gated_description_keeps_only_enabled_blocks()` — verifies marker-based block stripping
- `tests::gated_description_real_yaml_hides_sheets_when_disabled()` — verifies real YAML description hides irrelevant capabilities
- `tests::args_parse_minimal()` — verifies minimal args parsing
- `tests::wrapped_code_packages_all_sinks()` — verifies prelude/postlude package all sinks
- `tests::prelude_imports_pandas()` — verifies pandas import in prelude
- `tests::sql_config_absent_is_none()` — verifies absence returns None
- `tests::sql_config_missing_allowed_schemas_errors()` — verifies validation
- `tests::sql_config_missing_connection_url_errors()` — verifies validation
- `tests::sql_config_valid_full_block_parses_with_defaults()` — verifies parsing with defaults
- `tests::sql_config_respects_runtime_limits_and_table_policies_and_tenant()` — verifies field extraction
- `tests::sql_config_resolves_env_var_in_connection_url()` — verifies env var resolution
- `tests::tenant_user_id_env_is_resolved_in_both_places()` — verifies env resolution in both top-level field and embedded permissions
- `tests::enabled_sources_sql_true_when_sql_block_present()` — verifies sql flag
- `tests::enabled_sources_gsheets_true_when_agent_has_gsheets()` — verifies gsheets flag
- `tests::enabled_sources_gsheets_true_when_enable_gsheets_flag_set()` — verifies gsheets flag from fixed_config

## File-level notes

- Well-structured with clear section markers (Args, Capability gating, SQL sink configuration, Dispatch, Tool builder, Tests)
- Implements hexagonal architecture: domain traits (SqlConnectionPort, SheetsClient) injected via dispatch_core, with concrete wiring by executor
- Comprehensive validation: SQL config validated for required fields and non-empty allowed_schemas; bindings validated upstream; policy parsing wrapped with structured error response
- Error handling throughout dispatch_core returns structured JSON responses (never panics); gsheets client failures degrade gracefully with logging
- Ignored tests are intentional: PyO3 per-process interpreter flaking under parallel `cargo test` when multiple tests init it; passes in isolation; E2E coverage via live graphs documented in comments
- No security issues: SQL permissions validated separately, attachment handling via trait ports (no direct I/O), env var resolution scoped to known fields

