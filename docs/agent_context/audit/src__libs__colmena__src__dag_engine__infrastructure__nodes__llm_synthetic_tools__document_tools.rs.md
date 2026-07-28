# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs

**Layer:** infrastructure  
**Purpose:** Synthetic LLM tool adapters for document artifact management. Each of seven tools builds a JSON schema, parses LLM arguments, injects session_id from execution context, and dispatches to the corresponding application-layer use case.

## Symbols

### Constants (pub)
- `DOCUMENT_CREATE_TOOL` (const) — tool name constant "document_create"
- `DOCUMENT_APPLY_PATCH_TOOL` (const) — tool name constant "document_apply_patch"
- `DOCUMENT_READ_TOOL` (const) — tool name constant "document_read"
- `DOCUMENT_GET_HEAD_TOOL` (const) — tool name constant "document_get_head"
- `DOCUMENT_LIST_VERSIONS_TOOL` (const) — tool name constant "document_list_versions"
- `DOCUMENT_ROLLBACK_TOOL` (const) — tool name constant "document_rollback"
- `DOCUMENT_LIST_MY_ARTIFACTS_TOOL` (const) — tool name constant "document_list_my_artifacts"
- `DOCUMENTS_SYSTEM_PRELUDE` (const) — included markdown prelude teaching LLM how to use the seven document_* tools end-to-end

### Structs (pub)
- `DocumentCreateArgs` (struct) — deserialized args for create tool (kind, initial_ir, label, retention_limit)
- `DocumentApplyPatchArgs` (struct) — deserialized args for apply_patch tool (artifact_id, base_version, ops)
- `DocumentReadArgs` (struct) — deserialized args for read tool (artifact_id, version, optional slice)
- `DocumentReadSlice` (struct) — selects portion of IR to return (sheets, block_ids, cell_ranges)
- `CellRangeFilter` (struct) — Excel cell range filter (sheet_id, range in A1 style)
- `DocumentGetHeadArgs` (struct) — deserialized args for get_head (artifact_id, optional since_version)
- `DocumentListVersionsArgs` (struct) — deserialized args for list_versions (artifact_id, optional limit)
- `DocumentRollbackArgs` (struct) — deserialized args for rollback (artifact_id, to_version)
- `DocumentListMyArtifactsArgs` (struct) — empty args struct (no LLM-visible parameters)
- `DocumentToolsContext` (struct) — holds Arc-wrapped use case instances, optional SessionArtifactIndex, and SessionId

### Functions (pub)
- `build_document_create_tool()` — builds ToolDefinition for document_create using schemars JsonSchema
- `build_document_apply_patch_tool()` — builds ToolDefinition for document_apply_patch
- `build_document_read_tool()` — builds ToolDefinition for document_read
- `build_document_get_head_tool()` — builds ToolDefinition for document_get_head
- `build_document_list_versions_tool()` — builds ToolDefinition for document_list_versions
- `build_document_rollback_tool()` — builds ToolDefinition for document_rollback
- `build_document_list_my_artifacts_tool()` — builds ToolDefinition for document_list_my_artifacts
- `build_all_document_tools()` — returns Vec of all seven tool definitions in spec order
- `dispatch_document_create()` — async dispatcher: parses args, injects session_id, calls CreateDocumentUseCase, registers in index
- `dispatch_document_apply_patch()` — async dispatcher: parses args, calls ApplyPatchUseCase, updates index on success, returns VersionConflict on conflict
- `dispatch_document_read()` — async dispatcher: parses args, calls ReadDocumentUseCase, applies DocumentReadSlice filter if present
- `dispatch_document_get_head()` — async dispatcher: parses args, calls GetHeadUseCase, returns metadata and version window
- `dispatch_document_list_versions()` — async dispatcher: parses args, calls ListVersionsUseCase, maps entries to JSON
- `dispatch_document_rollback()` — async dispatcher: parses args, calls RollbackUseCase, updates index on success
- `dispatch_document_list_my_artifacts()` — async dispatcher: returns error if session_index missing, otherwise calls SessionArtifactIndex::list_by_session

### Functions (private)
- `apply_slice()` (fn) — applies DocumentReadSlice filter to IR JSON value, best-effort silently ignores unrecognized IR shapes (Excel/Word sheets/blocks/cells)
- `address_in_range()` (fn) — checks if A1-style cell address is within a range ("B5" in "A1:C10", handles column-only ranges "A:A")
- `split_a1()` (fn) — parses A1-style cell address into (column_u32, optional_row_u32)

### Tests (module-level)
- `schema_string()` — helper extracts JSON schema string from ToolDefinition
- `document_create_schema_mentions_kind()` — verifies schema contains kind and initial_ir fields
- `apply_patch_schema_includes_ops_enum()` — verifies schema contains set_cell and A1-style range documentation
- `read_schema_includes_slice()` — verifies schema mentions slice, block_ids, cell_ranges
- `build_all_returns_seven_tools()` — verifies build_all_document_tools returns exactly 7 tools with correct names
- `no_tool_schema_exposes_session_id()` — security test: verifies no tool leaks session_id in input schema
- `list_my_artifacts_takes_no_visible_params()` — verifies list_my_artifacts tool has no LLM-visible parameters
- `address_in_range_handles_basic_cases()` — unit tests address_in_range with single cells, ranges, full-column references

## File-level notes

- **Security enforced**: session_id is never read from LLM input; all dispatchers inject it from DocumentToolsContext per spec §11.1. Malicious LLM-provided session_id is silently ignored by typed structs.
- **Error handling strategy**: all dispatch functions catch serde errors and return JSON `{"error": "..."}` objects; application errors are formatted as strings. VersionConflict returns structured JSON with current_version and conflicts array.
- **Optional session_index**: document_list_my_artifacts explicitly checks for session_index presence and returns a structured error if missing; other tools work without it (silently skip index registration on None).
- **Slice filtering**: apply_slice uses best-effort approach — unrecognized IR shapes are silently ignored (documented in comment), which is appropriate for LLM-facing tool that should gracefully degrade.
- **A1 range parsing**: address_in_range and split_a1 handle edge cases (column-only ranges like "A:A", single cells, reversed ranges) correctly via min/max logic and Option-based row tracking.
- **Tool builders**: All builder functions follow identical pattern: call `super::build_synthetic_tool_with_summary::<ArgsType>()` with tool name and text lookups — no implementation logic visible here (delegated to parent module).
