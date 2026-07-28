# src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs

**Layer:** Infrastructure  
**Purpose:** Bridges LLM tool calls to DAG node execution, orchestrating argument merging (node_schema, $DYNAMIC, field_mapping), secure value injection/masking, and dispatch to 30+ synthetic tools (gsheets, gdocs, SQL, attachments, skills, documents, CRDT, recall_history, etc.).

## Symbols

### Type Aliases & Constants
- `SkillObserver` (type alias) — callback fired when load_skill tool call succeeds with dispatched payload
- `ToolDescribeObserver` (type alias) — callback fired when describe_tool call succeeds
- `DEFAULT_MAX_TOOL_RESULT_STRING_BYTES` (const) — default per-string cap (50 KB) for LLM output scrubbing

### Main Struct
- `DagToolExecutor` (struct) — executes DAG nodes on behalf of LLM tool calls, manages tool configurations, secure values, skills, attachments, and synthetic tool dispatch

### Builders & Constructors
- `DagToolExecutor::new()` — creates new executor with node registry and tool configurations
- `DagToolExecutor::with_subgraph_depth()` — sets current tool nesting depth for recursion guard
- `DagToolExecutor::with_conversation_history()` — wires conversation repository for recall_history tool
- `DagToolExecutor::with_max_tool_result_bytes()` — overrides per-string output cap
- `DagToolExecutor::with_secure_values()` — attaches SecureValueService + session_id for secret injection
- `DagToolExecutor::with_session_id()` — sets session_id without SecureValueService
- `DagToolExecutor::with_agent_session_id()` — sets agent_session_id for stable chat handle scoping
- `DagToolExecutor::with_skills()` — attaches SkillRepository for load_skill dispatch
- `DagToolExecutor::with_skill_observer()` — attaches observer callback after load_skill dispatch
- `DagToolExecutor::with_observer()` — threads ExecutionObserver into tool-invoked nodes for SSE events
- `DagToolExecutor::with_documents()` — attaches DocumentToolsContext for document_* tool dispatch
- `DagToolExecutor::with_crdt_documents()` — attaches CrdtDocsContext for crdt_doc_* tool dispatch
- `DagToolExecutor::with_describe_tool_lookup()` — attaches ToolConfiguration snapshot for describe_tool
- `DagToolExecutor::with_describe_tool_observer()` — attaches observer callback after describe_tool
- `DagToolExecutor::with_attachments()` — attaches ConversationAttachment catalog snapshot
- `DagToolExecutor::with_attachment_storage()` — attaches OutputStorageRepository for byte I/O
- `DagToolExecutor::with_attachment_registry()` — wires live registry fallback for mid-turn attachments

### Attachment Plumbing
- `DagToolExecutor::fetch_attachment_bytes()` — fetches raw bytes of registered attachment by document_id
- `DagToolExecutor::fetch_attachment_stream()` — streaming counterpart for large payloads (multipart, COPY)
- `DagToolExecutor::register_attachment_bytes()` — persists newly produced bytes and returns document_id
- `DagToolExecutor::lookup_attachment_meta()` — looks up original mime_type + filename from catalog
- `DagToolExecutor::lookup_storage_key()` — resolves document_id → storage_key with fallback chain
- `DagToolExecutor::lookup_storage_key_via_registry()` — live registry lookup for mid-turn outputs

### Tool Execution & Configuration
- `DagToolExecutor::execute_inner()` — shared body for `ToolExecutor::execute` + `execute_with_resume_answer`; orchestrates merge strategies, secret injection, node execution, masking, and secure hashing
- `DagToolExecutor::execute_with_resume_answer()` — executes tool with resume_answer injected for suspend/resume
- `DagToolExecutor::execute_toolkit()` — dispatches toolkit sub-tools by routing to ToolkitNode
- `DagToolExecutor::generate_tool_definition()` — builds LLM-facing ToolDefinition from node_schema, $DYNAMIC, parameters, or fallback
- `DagToolExecutor::mark_gsheets_sheet_seen()` — marks sheet as read this turn (idempotent)
- `DagToolExecutor::gsheets_run_python_guarded()` — inspect-and-run guard: previews unseen sheets before execution

### Merge Strategies & Templates
- `DagToolExecutor::resolve_template_string()` — resolves ${var} / ${context.var} in fixed_config strings
- `DagToolExecutor::resolve_value_templates()` — recursively resolves templates in JSON values
- `DagToolExecutor::collect_dynamic_fields()` — scans fixed_config for $DYNAMIC placeholders

### Output Scrubbing
- `DagToolExecutor::scrub_value_for_llm()` — removes binary base64 (data: URIs) and truncates oversized strings with head preservation
- `DagToolExecutor::scrub_tool_result_output()` — applies scrubbing to JSON or text output strings
- `DagToolExecutor::head_truncate()` — truncates string by keeping head + marker, respects UTF-8 boundaries

### Paths & Identifiers
- `DagToolExecutor::ephemeral_subgraph_path()` — generates deterministic ephemeral path (tool/{call_id}) for tool-invoked memory isolation

### ToolExecutor Trait Impl
- `ToolExecutor::execute()` — async; executes tool call and scrubs result before returning to LLM
- `ToolExecutor::available_tools()` — async; returns all available ToolDefinitions (configured tools, toolkits expanded, raw nodes)

### Utility Functions
- `synthesise_default_toolkit_config()` — builds default ToolConfiguration for flag-only toolkit aliases (api_explorer)

## File-level Notes

- **Comprehensive dispatch orchestration:** The file routes 30+ synthetic tool types (gsheets, gdocs, SQL bulk, data_run_python, attachment_run_python, document_*, crdt_doc_*, load_skill, describe_tool, load_attachment, recall_history) plus toolkit sub-tools + raw nodes. Each dispatch path is tested.

- **Three merge strategies:** node_schema (highest priority, fully declarative), $DYNAMIC placeholders (simple substitution, one level deep in containers), deprecated field_mapping + mergeable_fields (backward compatibility, tested).

- **Secure value lifecycle:** secret injection → node execution → outbound masking → optional secure hashing (if `secure: true`). Both paths (inject_secrets success and error) are masked before returning.

- **Attachment plumbing layers:** snapshot lookup (fast, no DB) → live registry fallback (mid-turn outputs) → structured errors for missing wiring. Implements Bulk T0 contract (2026-06-09) for sql_bulk, gsheets xlsx, gdocs create_from_docx/export/insert_image.

- **gsheets inspect-and-run guard:** Option A pattern — reads unseen sheet previews, executes code, returns both in one round-trip (avoids forced re-call pattern).

- **Output scrubbing:** Always-on binary elision (data: URIs never reach LLM), per-string truncation with head preservation (useful for markdown tables/previews).

- **Deprecated backward compatibility:** field_mapping, mergeable_fields, exposed_inputs still supported with `#[allow(deprecated)]` on tests to ensure existing graphs work.

- **Outdated comments:** Lines 1397–1401 mention create_from_docx and export as "stubbed" / "not_yet_wired", but both are actually implemented via the via_executor pattern (Bundle 1, 2026-06-10). Comments should be updated to reflect current state.

- **Builder ergonomics:** 18 builder methods allow flexible wiring of optional dependencies (SecureValueService, SkillRepository, attachment storage, conversation history, observers, etc.). Unset dependencies gracefully fail with structured error messages rather than panics.

- **Test coverage:** 50+ tests covering field mapping, $DYNAMIC, toolkit dispatch, flag-only api_explorer, attachment plumbing, secure values, scrubbing, ephemeral paths, schema validation, and backward compatibility.
