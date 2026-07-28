# src/libs/colmena/src/llm/domain/llm_request.rs

**Layer:** domain  
**Purpose:** Manages LLM request construction with message normalization (coalescing consecutive same-role messages for provider compatibility) and request validation. Handles tool definitions and tool choice configuration.

## Symbols

- `coalesce_consecutive_same_role` (fn, pub) — Merges adjacent messages with the same role (except Tool, which may appear consecutively for parallel results); normalizes wire shape for providers requiring strictly alternating roles
- `merge_same_role` (fn, private) — Helper joining two same-role messages by concatenating content, tool_calls, and files; fallback chain ensures infallible construction for valid inputs
- `LlmRequest` (struct, pub) — Encapsulates a complete LLM API request: messages, config, stream flag, optional tools, and tool choice strategy
- `LlmRequest::new` (impl, pub) — Constructor validating that messages are non-empty, coalescing consecutive same-role messages, and enforcing role alternation (Tool messages allowed consecutively)
- `LlmRequest::with_id` (impl, pub) — Builder method setting a custom request ID
- `LlmRequest::with_tools` (impl, pub) — Builder method attaching a vector of tool definitions to the request
- `LlmRequest::with_tool_choice` (impl, pub) — Builder method specifying tool choice strategy (e.g. "auto", "none", or specific function name)
- `LlmRequest::id` (impl, pub) — Getter returning reference to the request ID
- `LlmRequest::messages` (impl, pub) — Getter returning slice of messages in the request
- `LlmRequest::config` (impl, pub) — Getter returning reference to the LlmConfig
- `LlmRequest::stream` (impl, pub) — Getter returning the stream flag (boolean)
- `LlmRequest::is_streaming` (impl, pub) — Convenience getter returning the stream flag [FLAG: improvement — duplicates `stream()` getter; consider removing one for a cleaner public API]
- `LlmRequest::message_count` (impl, pub) — Convenience getter returning the count of messages
- `LlmRequest::last_message` (impl, pub) — Convenience getter returning an optional reference to the final message
- `LlmRequest::first_message` (impl, pub) — Convenience getter returning an optional reference to the initial message
- `LlmRequest::tools` (impl, pub) — Getter returning an optional slice of tool definitions
- `LlmRequest::tool_choice` (impl, pub) — Getter returning an optional string reference for the tool choice setting
- `LlmRequest::has_tools` (impl, pub) — Predicate checking whether tools are present and non-empty

## File-level notes

- **Message coalescing design**: The `coalesce_consecutive_same_role` function self-heals poisoned conversation histories (e.g., dangling user messages left by failed API turns) without touching persistence; `recall_history` retains originals. This is a pure function with no side effects.
- **Tool message exception**: Consecutive Tool messages are intentionally preserved (line 117 `continue`), as they represent parallel tool results keyed by distinct `tool_call_id` — a legitimate multi-turn pattern that providers require.
- **Fallback construction pattern**: The `merge_same_role` function (lines 69–73) uses a three-tier fallback: built message → space message → empty assistant. This is defensive and correct, ensuring the function cannot panic given valid inputs.
- **Comprehensive test coverage**: 13 test functions verify coalescing (empty, singleton, alternating), poisoned-history self-healing, role validation, file/tool-call merging, and request construction edge cases.
- **Builder pattern**: The struct uses a standard builder pattern (`with_id`, `with_tools`, `with_tool_choice`) for optional fields, enabling fluent configuration.
