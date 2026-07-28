# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs

**Layer:** infrastructure  **Purpose:** Implements the `describe_tool` synthetic tool dispatcher that generates curated markdown documentation for tools and handles introspection queries from the LLM during lazy tool loading.

## Symbols

- `DESCRIBE_TOOL_NAME` (const, pub) — constant string "describe_tool" naming this synthetic tool
- `DescribeToolDispatchResult` (struct, pub) — holds the tool_call_id, output markdown, and tool_name from a describe_tool dispatch
- `generate_tool_markdown` (fn, pub) — produces the markdown the LLM sees by filtering out fixed fields and fixed_config-shadowed entries, rendering a name + description + parameter table
- `collect_visible_fields` (fn, private) — returns only LLM-visible fields from node_schema: excludes `fixed`-marked fields and entries shadowed by top-level `fixed_config`
- `dispatch_describe_tool` (async fn, pub) — parses describe_tool arguments, looks up the requested tool in the configuration catalog, returns curated markdown or "Error: ... not found" string
- `into_tool_result` (fn, pub) — converts DescribeToolDispatchResult into a ToolResult, marking success based on whether output starts with "Error:"

## File-level notes

- All public functions are well-tested with comprehensive coverage of success paths (known tool, unknown tool, missing argument) and edge cases (fixed fields, fixed_config shadowing).
- The test suite uses private helpers (`empty_field`, `fixed_field`, `cfg_minimal`, `mk_call`) to construct test fixtures — standard pattern for integration tests.
- Error detection in `into_tool_result` uses string-prefix matching ("Error:") which is consistent with the error format in `dispatch_describe_tool` (line 104).
- No external API calls; pure value transformation and markdown rendering.
