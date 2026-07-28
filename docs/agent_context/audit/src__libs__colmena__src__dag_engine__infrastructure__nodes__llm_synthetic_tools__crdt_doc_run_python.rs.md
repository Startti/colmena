# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_run_python.rs

**Layer:** infrastructure  
**Purpose:** Implements `crdt_doc_run_python` LLM tool—executes sandboxed Python code against CRDT-backed workbook data with support for DataFrame I/O, collision policies (fail/auto_suffix/overwrite), and deterministic row-level diffing with selective cell updates.

## Symbols

### Public
- `TOOL_RUN_PYTHON: &str` — constant identifier for the tool ("crdt_doc_run_python")
- `RunPythonArgs` (struct, pub, Deserialize/JsonSchema) — captures sheet_ids, code, and collision policy from LLM tool invocation; aliases UX paraphrases (sheets, sheet_names)
  - `sheet_ids: Vec<String>` — sheets to load as dfs[<id>] DataFrames in sandbox
  - `code: String` — user Python code; must define output and/or output_sheets
  - `on_existing_sheet: Option<String>` — collision policy override (default: fail)
- `tool_run_python() -> ToolDefinition` — builds LLM tool definition by calling build_synthetic_tool_with_summary
- `execute_run_python(ctx: &CrdtDocsContext, args: RunPythonArgs) -> serde_json::Value` — main executor; orchestrates extraction→sandbox→output dispatch; returns {output, wrote_sheets, stdout, error, _output_truncated?}
- `dispatch_crdt_doc_run_python(ctx: &CrdtDocsContext, args: serde_json::Value) -> serde_json::Value` — JSON deserialization wrapper for tool dispatcher
- `CrdtDocsContext` (re-exported, pub use) — context for CRDT document and backend access

### Private
- `CRDT_PY_PRELUDE: &str` — included Python text (prelude.md); setup for dfs dict and imports
- `CRDT_PY_POSTLUDE: &str` — included Python text (postlude.md); packages output and output_sheets for return
- `OUTPUT_BYTE_CAP: usize` — response capping constant (10 KB)
- `STDOUT_BYTE_CAP: usize` — stdout capping constant (10 KB)
- `ERROR_BYTE_CAP: usize` — error message capping constant (10 KB)
- `CODE_TIMEOUT_SECS: u64` — sandbox execution timeout (30 seconds, v1 hardcoded)
- `crdt_lookup_tab_by_name(doc: &yrs::Doc, name: &str) -> Option<(String, TabMeta)>` — queries CRDT projection for sheet ID and metadata by name; returns None if sheet absent
- `crdt_replace(doc, ctx, raw_name, entry, policy) -> serde_json::Value` — mode dispatcher for replace; checks collision, applies policy (Fail→error, Overwrite→write, AutoSuffix→iterate names 2-10)
- `crdt_overwrite(doc, ctx, raw_name, entry) -> serde_json::Value` — mode for overwrite; validates schema match (current columns vs input columns); blocks if mismatch and allow_schema_change not set
- `crdt_update_in_place(doc, ctx, raw_name, entry) -> serde_json::Value` — mode for update_in_place; diffs current records vs new_records by key, builds row/col index maps, applies cell-level changes via A1 addresses
- `crdt_write_full(doc, ctx, raw_name, write_name, entry) -> serde_json::Value` — writes DataFrame as new sheet; calls write_records_as_new_sheet and records change event
- `wrap_user_code(user_code: &str) -> String` — concatenates prelude + user code + postlude
- `truncate(s: &str, cap: usize) -> String` — byte-safe string truncation to cap with ellipsis marker
- `truncate_json(v: &serde_json::Value, cap: usize) -> (serde_json::Value, bool)` — serializes JSON to string, truncates if over cap, returns (Value, was_truncated)

### Test module
- `make_runtime() -> Arc<CrdtDocumentsRuntime>` — creates in-memory test runtime with temp storage
- `sheet_ids_accepts_sheets_and_sheet_names_aliases()` — unit test validating serde aliases for field name variants
- `multi_sheet_output_sheets_writes_three_new_tabs()` — integration test seeding 3-row sheet, running Python code to produce 3 output sheets, verifying they exist in runtime
- `update_in_place_patches_only_changed_cells_in_crdt()` — integration test modifying one cell via update_in_place mode, verifying only 1 cell written

## File-level notes

### Potential Issues

1. **Silently ignored cell-write errors (line 424)** — `apply_set_cell_in_proc` is called in update_in_place mode but its Result is discarded with `let _`. If a cell write fails, `cells_written` is still incremented, misleading the user that all changes succeeded. No error tracking for partial failures.

2. **Silently skipped unmapped changes (lines 408–410)** — when diffing finds a change to a key or column that is absent from the current row/column index maps, the change is silently skipped with `continue`. User sees `rows_changed` and `cells` counts but has no visibility into which changes were dropped due to missing keys or column names. Could indicate data loss or schema mismatches not reported.

3. **Hardcoded timeout (line 32, BACKLOG note)** — sandbox timeout is fixed at 30 seconds with no operator-configurable path yet. Noted as future work in BACKLOG.

### Design Notes

- **Error protocol**: Python postlude embeds `_postlude_error` field in output_sheets entries to signal row-level failures (lines 181–183); handled distinctly from Rust execution errors.
- **Collision policy flow**: Three distinct CRDT mutation paths (replace, overwrite, update_in_place) each handle pre-write validation and event recording separately (lines 256–521).
- **Change event tracking**: Every successful write records an origin (agent:session_id or agent:llm) and summary to backend; event_id is tracked on context (lines 429–446).
- **Schema validation**: overwrite mode checks column mismatch and fails closed (lines 306–329) unless `allow_schema_change: true` is explicitly set in the spec dict.
- **Diff-driven updates**: update_in_place uses `diff_records` (external, from sheet_collision module) to compare record sets; only cells with differing values are written, respecting `key`, optional `columns` restriction, and `strict_match` flag.

### Assumptions

- Python prelude and postlude files exist at compile time and are syntactically valid Python.
- `apply_set_cell_in_proc` either always succeeds or panics; calling code assumes no error cases need handling (or ignores them).
- CRDT projection schema includes `sheets[].{id, name}` and supports `build_sheet_records` lookups.
- LLM-provided code is guaranteed safe by the sandbox (enforced by caller, not this module).
