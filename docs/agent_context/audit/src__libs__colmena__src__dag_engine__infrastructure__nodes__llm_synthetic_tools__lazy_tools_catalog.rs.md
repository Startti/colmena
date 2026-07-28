# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs

**Layer:** infrastructure  **Purpose:** Pure data structures and functions for lazy tool loading catalog management — reconstructing discovered tools from conversation history, computing catalog summaries, and building the `describe_tool` tool definition for the LLM.

## Symbols

### Public Functions & Types

- `current_turn_slice` (fn, pub) — narrows messages slice to the current user turn (from last `MessageRole::User` onward) for per-turn lazy discovery enforcement
- `CatalogEntry` (struct, pub) — represents one tool in the catalog with `name` and `summary` fields
- `SUMMARY_MAX_CHARS` (const, pub) — maximum length for catalog summary entries: 200 chars
- `FALLBACK_DESCRIPTION_CHARS` (const, pub) — truncation budget when falling back to description: 120 chars
- `summary_for_catalog` (fn, pub) — resolves the display summary for a tool, with truncation to word boundary; uses summary if ≤200 chars, otherwise falls back to first 120 chars of description
- `reconstruct_discovered_set` (fn, pub) — computes the set of tool names already discovered in the conversation via two rules: (1) `describe_tool` calls with that name, or (2) direct tool calls to cataloged tools
- `build_describe_tool_definition` (fn, pub) — constructs the LLM-facing `describe_tool` ToolDefinition, listing pending catalog entries as enum values and including usage guidance

### Private Functions & Types

- `DescribeArgs` (struct, private) — deserializes the JSON arguments of a `describe_tool` call (only the `name` field used)
- `truncate_at_word_boundary` (fn, private) — helper that truncates a string to a char limit on a word boundary (whitespace search from the right)

### Tests

- `returns_summary_when_present_and_within_limit` — validates summary returned as-is when under 200 chars
- `falls_back_to_description_truncated_when_no_summary` — validates fallback to description truncation at ~120 chars
- `truncates_summary_over_200_chars_at_word_boundary` — validates truncation at word boundary for long summaries
- `returns_empty_string_when_neither_summary_nor_description` — validates empty result when both inputs are empty
- `empty_history_yields_empty_set` — validates empty discovered set for empty message history
- `rule1_describe_tool_call_adds_named_tool` — validates rule (1): `describe_tool` calls mark tools as discovered
- `rule2_direct_call_to_cataloged_tool_adds_it` — validates rule (2): direct tool calls mark tools as discovered
- `rule2_ignores_calls_to_uncataloged_tools` — validates uncataloged tools are not added
- `rule1_records_unknown_describe_tool_target` — validates any name in a `describe_tool` call is recorded (catalog mismatch handled elsewhere)
- `malformed_describe_tool_args_are_skipped_silently` — validates silent skip of malformed JSON in tool calls
- `unions_rule1_and_rule2_across_messages` — validates both discovery rules apply across multiple messages
- `definition_lists_pending_in_alphabetical_order` — validates describe_tool lists catalog entries alphabetically
- `definition_description_includes_summaries` — validates the LLM description text includes tool names and summaries
- `definition_required_param_is_name` — validates the `name` parameter is required
- `current_turn_slice_no_user_returns_whole` — validates return of entire slice when no user message present
- `current_turn_slice_starts_at_last_user` — validates slice starts from last user message
- `per_turn_discovery_ignores_prior_turn_describe` — validates per-turn filtering drops prior-turn discoveries
- `per_turn_discovery_keeps_this_turn_describe` — validates per-turn filtering keeps current-turn discoveries

## File-level notes

- **Module purpose**: Pure data types and functions with no I/O or provider awareness; all logic operates over conversation messages and tool metadata.
- **Per-turn enforcement**: Lazy discovery is scoped per turn via `current_turn_slice`, mirroring the gsheets inspect-before-code guard. Tools must be re-discovered each turn.
- **Discovery rules**: Two canonical rules ensure discovered tools are captured even after truncation, mode switches, or manual seeding:
  1. Assistant calls `describe_tool(name=X)` 
  2. Assistant directly calls a tool whose name matches the catalog
- **Error handling**: Malformed JSON in tool call arguments is silently skipped (line 56) — intentional per test design, but could benefit from debug logging for production diagnostics.
- **Word boundary truncation**: The `truncate_at_word_boundary` helper safely handles UTF-8 by working with char indices and finding the last whitespace boundary within the limit.
- **Test coverage**: Comprehensive unit tests cover all public functions, edge cases (empty inputs, long strings, missing fields), discovery rules, per-turn filtering, and the describe_tool definition builder.
