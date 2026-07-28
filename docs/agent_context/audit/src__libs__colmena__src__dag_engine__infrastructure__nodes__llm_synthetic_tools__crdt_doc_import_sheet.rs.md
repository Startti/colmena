# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs

**Layer:** infrastructure  
**Purpose:** Implements the `crdt_doc_import_sheet` LLM synthetic tool, enabling cross-sheet analysis by cloning sheets from one CRDT artifact into another with snapshot semantics (later changes to source do not propagate).

## Symbols

- `CrdtDocsContext` (re-export, public) — context for CRDT operation execution in Local or WsPeer modes
- `TOOL_IMPORT_SHEET` (const, public) — name identifier for the synthetic tool (`"crdt_doc_import_sheet"`)
- `MAX_IMPORT_BYTES` (const, public) — maximum import size per operation (100 MB, matching `crdt_doc_run_python` cap)
- `MAX_SHEETS_PER_ARTIFACT` (const, public) — maximum sheets allowed per artifact (100, defensive against agent loops)
- `ImportSheetArgs` (struct, public) — deserialized LLM tool arguments: source_artifact_id, source_sheet_id, optional new_name
- `tool_import_sheet()` (fn, public) — constructs a ToolDefinition from text registry entries for LLM exposure
- `ExtractedSheet` (struct, private) — holds extracted sheet state: name, cells (A1-address/value/type triples), bytes_estimate
- `extract_source_sheet()` (fn, private) → Option — reads source sheet name and cells into owned values via yrs transaction, returns None if sheet not found
- `count_sheets()` (fn, private) → usize — counts sheets in a workbook's materialized state, defensive against unmaterialized workbooks
- `import_sheet_runtime()` (async fn, public) — core import logic: validates self-import, resolves source/dest artifacts, enforces size/sheet-count caps, creates new sheet, writes cells, records audit event
- `execute_import_sheet()` (async fn, public) — dispatcher routing to either Local (direct runtime) or WsPeer (REST backend) mode, extracts event_id, strips it from LLM-visible output
- `dispatch_crdt_doc_import_sheet()` (async fn, public) — JSON args dispatcher deserializing ImportSheetArgs and invoking execute_import_sheet
- `any_to_json()` (fn, private) — converts yrs::Any value types (Null, Bool, Number, BigInt, String) to serde_json::Value, defaults unsupported types to Null

## File-level notes

- **Well-structured validation pipeline**: import_sheet_runtime performs 10 sequential checks (self-import, source existence, sheet existence, size cap, destination existence, sheet-count cap) with structured JSON error returns at each boundary.
- **Dual-mode execution**: execute_import_sheet branches between Local (inline runtime call) and WsPeer (HTTP POST delegation) with symmetric error handling.
- **Event tracking integration**: all mutations route through change_tracker_store.insert_event() with origin keyed on agent_session_id, and event_id is extracted and stripped from LLM surface to keep tool payload stable.
- **Snapshot-only semantics**: import reads a point-in-time snapshot of source sheet (no subscriptions or follow-up propagation); documented in design spec 2026-06-04.
- **A1 cell address handling**: reuses crdt_doc_tools::parse_a1_to_rc() to derive max_row/max_col for result metadata; cells are cloned via apply_set_cell_in_proc (reuses canonical write path, re-derives cell type from JSON value).
- **Silent Null conversion in any_to_json**: line 377 defaults unsupported yrs::Any variants (Byte, Float64Array, etc.) to Null rather than logging or erroring; this is likely intentional for robustness but may silently drop exotic cell content if encountered.
