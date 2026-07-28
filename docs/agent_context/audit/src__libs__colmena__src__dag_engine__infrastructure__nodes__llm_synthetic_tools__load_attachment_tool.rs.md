# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs

**Layer:** infrastructure  
**Purpose:** Implements the `load_attachment` synthetic tool for LLM agents. Builds tool definitions that embed a per-session attachment catalog and dispatches tool calls to return sentinels that AgentService intercepts for attachment injection.

## Symbols

- `LOAD_ATTACHMENT_TOOL_NAME` (const, pub) — constant string identifier for the load_attachment tool
- `ATTACHMENTS_SYSTEM_PRELUDE` (const, pub) — auto-injected system message explaining attachment availability and ephemeral semantics to the LLM
- `build_load_attachment_tool_definition` (fn, pub) — constructs ToolDefinition with description embedding the attachment catalog and enum of document IDs from the catalog
- `dispatch_load_attachment` (fn, pub) — executes a load_attachment tool call; returns success sentinel (`__colmena_status: LOAD_ATTACHMENT`) for known document IDs or structured error JSON for unknown IDs
- `mk_attachment` (fn, private) — test helper factory creating ConversationAttachment stub with minimal required fields
- `mk_call` (fn, private) — test helper factory creating ToolCall with JSON-serialized arguments
- `tests::tool_definition_lists_each_attachment` (test) — verifies tool description includes each attachment's ID and label
- `tests::tool_definition_enum_contains_every_id` (test) — verifies enum parameter contains all document IDs from catalog
- `tests::tool_definition_empty_catalog_renders_no_attachments_message` (test) — verifies graceful fallback description when catalog is empty
- `tests::dispatch_known_id_returns_sentinel` (test) — verifies successful dispatch returns sentinel JSON with __colmena_status
- `tests::dispatch_unknown_id_returns_error_json` (test) — verifies unknown document_id returns structured error (not tool error)
- `tests::dispatch_missing_document_id_is_invalid_tool_call` (test) — verifies missing parameter raises InvalidToolCall error
- `prelude_tests::prelude_explains_no_autoinject_behavior` (test) — verifies prelude markdown instructs LLM to call load_attachment proactively
- `prelude_tests::prelude_explains_ephemeral_load_attachment` (test) — verifies prelude markdown warns that load_attachment results are ephemeral per turn

## File-level notes

- **Improvement — redundant branching (lines 51–62):** `build_load_attachment_tool_definition` creates the same `ParameterProperty` in both the empty and non-empty branches, then only adds the enum in the else path. Can be simplified to construct once and conditionally chain `with_enum`.
- **Note — prelude string matching fragility:** Tests at lines 235–251 use loose string-matching assertions on the markdown prelude (`ATTACHMENTS_SYSTEM_PRELUDE`). These assertions are defensive but could miss subtle wording changes that don't break semantics.
- **Note — successful integration:** The module cleanly separates concerns: tool definition building (catalog snapshot → description + enum), dispatch logic (argument extraction + catalog lookup → sentinel or error), and comprehensive test coverage of all paths including edge cases (empty catalog, missing ID, invalid JSON).
