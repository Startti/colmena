# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_tools.rs

**Layer:** infrastructure  
**Purpose:** Provides synthetic LLM tool adapters for CRDT document operations (sheets, cells, ranges, artifacts). Each tool builds a schema, parses LLM arguments, delegates to `crdt_documents::tool_executor`, and returns structured JSON.

## Symbols

### Constants
- `TOOL_LIST_SHEETS` (const, pub) — tool name constant "crdt_doc_list_sheets"
- `TOOL_LIST_SHEETS_OF` (const, pub) — tool name constant "crdt_doc_list_sheets_of"
- `TOOL_READ` (const, pub) — tool name constant "crdt_doc_read"
- `TOOL_SET_CELL` (const, pub) — tool name constant "crdt_doc_set_cell"
- `TOOL_SET_RANGE` (const, pub) — tool name constant "crdt_doc_set_range"
- `TOOL_ADD_SHEET` (const, pub) — tool name constant "crdt_doc_add_sheet"
- `TOOL_GET_RECENT_CHANGES` (const, pub) — tool name constant "crdt_doc_get_recent_changes"
- `TOOL_LIST_MY_ARTIFACTS` (const, pub) — tool name constant "crdt_doc_list_my_artifacts"
- `TOOL_CREATE_ARTIFACT` (const, pub) — tool name constant "crdt_doc_create_artifact"

### Arg Structs
- `ListSheetsArgs` (struct, pub) — empty args struct; no LLM-visible parameters
- `ListSheetsOfArgs` (struct, pub) — artifact_id parameter for cross-artifact sheet listing
- `ReadArgs` (struct, pub) — sheet_id, optional range (A1 or A1:B2), include_formulas flag
- `SetCellArgs` (struct, pub) — sheet_id, addr (with "address" alias), value; supports formula strings
- `SetRangeArgs` (struct, pub) — sheet_id, start_addr (with "start" alias), values_2d (row-major 2D array)
- `AddSheetArgs` (struct, pub) — sheet name parameter
- `GetRecentChangesArgs` (struct, pub) — since_event_id cursor, sheet_id filter, limit, optional artifact_id (F feature)
- `ListMyArtifactsArgs` (struct, pub) — optional limit parameter
- `CreateArtifactArgs` (struct, pub) — artifact name parameter

### Tool Builders
- `tool_list_sheets()` (fn, pub) → ToolDefinition — builds tool definition for list_sheets
- `tool_list_sheets_of()` (fn, pub) → ToolDefinition — builds tool definition for list_sheets_of
- `tool_read()` (fn, pub) → ToolDefinition — builds tool definition for read
- `tool_set_cell()` (fn, pub) → ToolDefinition — builds tool definition for set_cell
- `tool_set_range()` (fn, pub) → ToolDefinition — builds tool definition for set_range
- `tool_add_sheet()` (fn, pub) → ToolDefinition — builds tool definition for add_sheet
- `tool_get_recent_changes()` (fn, pub) → ToolDefinition — builds tool definition for get_recent_changes
- `tool_list_my_artifacts()` (fn, pub) → ToolDefinition — builds tool definition for list_my_artifacts
- `tool_create_artifact()` (fn, pub) → ToolDefinition — builds tool definition for create_artifact

### Executors
- `execute_list_sheets(ctx)` (fn, pub) → Value — lists sheets with formula counts (payload advisory); error if artifact not found
- `list_sheets_of_runtime(runtime, aid)` (fn, pub) → Value — core projection used by both Local mode and REST handler; walks Y.Doc CRDT cells to compute n_rows/n_cols
- `execute_list_sheets_of(ctx, args)` (async fn, pub) → Value — dispatch to Local or WsPeer mode; WsPeer downcasts backend to RestBackend
- `execute_read(ctx, args)` (fn, pub) → Value — reads sheet cells with optional A1:B2 range filtering; supports include_formulas for {v}/{v,f,fs} shape
- `execute_set_cell(ctx, args)` (async fn, pub) → Value — single-cell write with formula recalc, warnings, event recording
- `execute_set_range(ctx, args)` (async fn, pub) → Value — 2D block write starting at start_addr; aggregates recalc counts and warnings
- `execute_add_sheet(ctx, args)` (async fn, pub) → Value — creates new sheet with generated ID; records event
- `execute_get_recent_changes(ctx, args)` (async fn, pub) → Value — fetches change stream with cursor, sheet filter, limit; supports cross-artifact audit (F); excludes own-origin events
- `execute_list_my_artifacts(ctx, args)` (async fn, pub) → Value — lists artifacts for a session; requires session_id
- `execute_create_artifact(ctx, args)` (async fn, pub) → Value — creates new artifact in Local (registry) or WsPeer (REST POST) mode

### Dispatch Wrappers
- `dispatch_crdt_doc_list_sheets(ctx, _args)` (async fn, pub) → Value — async wrapper; ignores args
- `dispatch_crdt_doc_read(ctx, args)` (async fn, pub) → Value — parses ReadArgs from JSON, calls execute_read
- `dispatch_crdt_doc_set_cell(ctx, args)` (async fn, pub) → Value — parses SetCellArgs, calls execute_set_cell
- `dispatch_crdt_doc_set_range(ctx, args)` (async fn, pub) → Value — parses SetRangeArgs, calls execute_set_range
- `dispatch_crdt_doc_add_sheet(ctx, args)` (async fn, pub) → Value — parses AddSheetArgs, calls execute_add_sheet
- `dispatch_crdt_doc_list_sheets_of(ctx, args)` (async fn, pub) → Value — parses ListSheetsOfArgs, calls execute_list_sheets_of
- `dispatch_crdt_doc_get_recent_changes(ctx, args)` (async fn, pub) → Value — parses GetRecentChangesArgs, calls execute_get_recent_changes
- `dispatch_crdt_doc_list_my_artifacts(ctx, args)` (async fn, pub) → Value — parses ListMyArtifactsArgs, calls execute_list_my_artifacts
- `dispatch_crdt_doc_create_artifact(ctx, args)` (async fn, pub) → Value — parses CreateArtifactArgs, calls execute_create_artifact

### Aggregators
- `build_all_crdt_doc_tools()` (fn, pub) → Vec<ToolDefinition> — returns all 11 CRDT tools including import_sheet and tool_run_python

### Helpers
- `parse_a1_to_rc(addr)` (fn, pub(super)) → Option<(u32, u32)> — parses A1 to 0-indexed (row, col) with checked overflow; used in list_sheets_of_runtime
- `parse_a1(addr)` (fn, private) → Option<(u32, u32)> — parses A1 to 0-indexed (row, col); used in execute_read and parse_range [FLAG: duplication]
- `parse_range(range)` (fn, private) → Option<((u32, u32), (u32, u32))> — parses "A1:B2" to ((r0,c0), (r1,c1)); used in execute_read
- `col_letter(col)` (fn, private) → String — converts 0-indexed column to letter(s) e.g. 0→"A", 26→"AA"; used in execute_set_range

### Test Module
- `tests` (mod, #[cfg(test)]) — test suite with 21 tests

### Test Helpers
- `make_runtime()` (async fn, private) → Arc<CrdtDocumentsRuntime> — creates temp runtime in /tmp with ULID-based path
- `fresh_ctx()` (async fn, private) → (CrdtDocsContext, PathBuf) — creates fresh Local context + temp dir
- `fresh_ctx_with_session(session_id)` (async fn, private) → (CrdtDocsContext, PathBuf) — creates fresh Local context with caller-supplied session_id

### Tests
- `lists_two_sheets()` (async test) — verifies list_sheets returns 2 created sheets with correct names and IDs
- `returns_error_for_unknown_artifact()` (async test) — verifies artifact_not_found error for unregistered ID
- `list_sheets_reports_formula_count()` (async test) — D-T7: verifies formula_count per sheet (Sheet1: 2 formulas, Sheet2: 0)
- `list_sheets_tool_def_has_correct_name()` (test) — verifies tool name matches TOOL_LIST_SHEETS constant
- `list_sheets_schema_has_no_visible_params()` (test) — verifies artifact_id not exposed in JSON schema
- `set_cell_then_read_returns_value()` (async test) — integration: set A1="hello", read returns it
- `set_range_writes_2d_block()` (async test) — integration: write 2D block starting at B2, verify values at B2/C2/B3/C3
- `execute_set_cell_surfaces_cells_recalculated_and_warnings()` (async test) — D-T5: verifies cells_recalculated and warnings shape (incl. NeedsBrowser warning)
- `execute_set_range_aggregates_recalc_and_warnings()` (async test) — verifies range write aggregates recalc counts and warnings across batch
- `read_with_include_formulas_returns_v_f_fs()` (async test) — D-T6: formula-aware read returns {v}/{v,f,fs} cell shape with formula source tag
- `read_with_range_filters()` (async test) — verifies range filtering (A1:B2) returns only in-range cells
- `get_recent_changes_empty_then_populated()` (async test, #[ignore]) — change stream with cursor, sheet filter; own-origin filtering
- `list_my_artifacts_returns_session_artifacts()` (async test, #[ignore]) — session-scoped artifact listing
- `create_artifact_returns_new_id_local_mode()` (async test) — verifies artifact creation returns ID with "art_" prefix
- `set_cell_args_accept_address_alias()` (test) — D-T16: "address" alias for "addr" field parses correctly
- `set_cell_args_canonical_addr_still_works()` (test) — canonical "addr" field still accepted
- `set_range_args_accept_start_and_values_aliases()` (test) — "start" alias for start_addr and "values" alias for values_2d
- `read_accepts_single_a1_range()` (async test) — D-T16: single A1 (no colon) auto-expands to A1:A1

## File-level notes

- **Duplication of A1 parsing**: `parse_a1_to_rc` (checked arithmetic) and `parse_a1` (unchecked arithmetic line 393: `col = col * 26 + ...`) are nearly identical. The unchecked version could overflow on pathological input; the checked version is safer. Consider consolidating into a single implementation.

- **Two ignored tests**: `get_recent_changes_empty_then_populated` (line 1285) and `list_my_artifacts_returns_session_artifacts` (line 1357) are skipped due to cross-test DB state issues (require isolated DB). Documented with explanation.

- **Arg struct aliases for UX**: SetCellArgs accepts both "addr" and "address"; SetRangeArgs accepts both "start_addr"/"start" and "values_2d"/"values". These aliases (D-T16) are intentional LLM ergonomics features and are tested.

- **Local vs WsPeer dispatch patterns**: `execute_list_sheets_of` and `execute_create_artifact` use context-aware dispatch; WsPeer mode downcasts `backend` trait object to `RestBackend` to access `client` and `base_url`. No panic guard if downcast fails (returns error JSON).

- **Event recording**: `execute_set_cell`, `execute_set_range`, and `execute_add_sheet` all call `ctx.mark_dirty()` and `ctx.backend().record_event(...)` to persist changes. `event_id` defaults to 0 on error (line 462, 545, 591).

- **D-T prefixes**: Design trace comments (D-T5, D-T6, D-T7, D-T16, F-T3) mark feature/design decisions and test coverage for payload size advisory, formula inclusion, per-sheet formula count, UX aliases, and cross-artifact feature (F).

- **list_sheets_of_runtime**: Shared projection used by both `execute_list_sheets_of` (Local mode) and a REST handler in the CRDT server. Walks Y.Doc CRDT structure directly to compute row/col bounds from cell addresses.

- **GetRecentChangesArgs artifact_id field**: NEW in subsystem F (cross-artifact audit). Allows `execute_get_recent_changes` to inspect any registered artifact, not just the context's pinned artifact. Default behavior (when omitted) is unchanged (B).

- **Async/sync split**: List operations (`execute_list_sheets`) are sync; write operations and backend queries are async. Dispatch wrappers are always async for uniform interface.
