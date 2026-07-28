# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs

**Layer:** infrastructure  
**Purpose:** Implements the `load_skill` synthetic tool for LLM agents. Builds a ToolDefinition from a SkillRepository's catalog and dispatches tool calls to load skill content and references, formatting output for the LLM.

## Symbols

- `LOAD_SKILL_TOOL_NAME` (const, pub) — constant string identifier for the load_skill tool name
- `build_load_skill_tool_definition` (function, pub) — builds a ToolDefinition by gathering catalog of available skills, sorting names, and embedding them as enum values and description text
- `LoadSkillDispatchResult` (struct, pub) — holds dispatch result: output text, skill name, optional reference, source, and size metadata
- `dispatch_load_skill` (async function, pub) — parses tool call arguments, loads skill or reference content from repository, handles errors as tool output (not as exceptions) to enable ReAct loop recovery
- `skill_error_as_result` (function, private) — converts a SkillError into a LoadSkillDispatchResult with plain error text for LLM consumption
- `into_tool_result` (function, pub) — converts LoadSkillDispatchResult into a ToolResult object by checking for "Error:" prefix to set success flag
- `FakeRepo` (struct, private, test) — mock implementation of SkillRepository for unit tests with two hardcoded skills
- `mk_call` (function, private, test) — helper to construct ToolCall objects from JSON arguments for testing
- `tool_definition_includes_all_skill_names_in_enum` (test) — verifies tool definition includes all skill names in the name parameter's enum
- `tool_definition_description_lists_each_skill` (test) — verifies description embeds all skill names and descriptions
- `dispatch_load_by_name_returns_body_and_references_block` (test) — verifies skill loading returns body plus references list
- `dispatch_load_reference_returns_reference_body` (test) — verifies reference-specific loading returns correct reference content
- `dispatch_missing_skill_returns_error_output` (test) — verifies unknown skill name produces error output
- `dispatch_undeclared_reference_returns_error_output` (test) — verifies missing reference produces error output with available options
- `dispatch_missing_name_parameter_is_invalid_tool_call` (test) — verifies omitted required 'name' parameter returns LlmError
- `into_tool_result_success_on_normal_output` (test) — verifies normal output produces success=true
- `into_tool_result_failure_on_error_prefix` (test) — verifies "Error:" prefix produces success=false

## File-level notes

- Error handling is intentional: errors from repository.load_skill/load_reference are wrapped as tool output strings (not propagated as LlmError) so the LLM's ReAct loop can continue and potentially recover (lines 151–152 explain design).
- Minor efficiency note: dispatch_load_skill reloads the skill when handling a reference request (lines 133–137) purely to determine the source metadata; this is acknowledged as acceptable because repository implementations are expected to be in-memory caches.
- No unfinished code: no todo!(), unimplemented!(), unreachable!(), or FIXME comments; all error paths are handled.
- Test coverage is comprehensive: 7 test functions cover happy path (skill + reference), error cases (missing skill, undeclared reference, invalid JSON), and tool result conversion.
- Catalog sorting is duplicated across lines 18–20 (for names enum) and 23–25 (for description lines), but serves distinct purposes and is not a concern.
