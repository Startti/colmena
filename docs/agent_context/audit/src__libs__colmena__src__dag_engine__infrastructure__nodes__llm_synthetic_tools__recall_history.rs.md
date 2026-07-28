# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs

**Layer:** infrastructure  
**Purpose:** Implements the `recall_history` synthetic LLM tool, allowing agents to retrieve and paginate through full original message content from conversation history by turn index. Wired into the dag_tool_executor via `with_conversation_history` builder.

## Symbols

- `TOOL_RECALL_HISTORY` (const, pub) — Tool name identifier string "recall_history"
- `RECALL_PAGE_DEFAULT_CHARS` (const, private) — Default pagination page size (8 KiB) when caller omits `limit`
- `RECALL_PAGE_MAX_CHARS` (const, private) — Hard ceiling on page size (16 KiB) regardless of caller request
- `RecallHistoryArgs` (struct, pub) — Deserialized tool arguments: `turn` (message index), optional `offset` (char position), optional `limit` (max chars per page)
- `RecallHistoryArgs.turn` (field, pub) — Persisted turn index matching `[T<n>]` labels in conversation summary
- `RecallHistoryArgs.offset` (field, pub) — Character offset for pagination (default 0)
- `RecallHistoryArgs.limit` (field, pub) — Maximum characters to return (clamped to 1–16384)
- `tool_recall_history()` (fn, pub) — Factory function returning a complete `ToolDefinition` with schema and descriptions from text registry
- `dispatch_recall_history()` (fn, pub, async) — Main dispatch handler: loads conversation by key, validates turn/offset, retrieves paginated message content, includes role/tool-call metadata, returns JSON response with `next_offset` for continuation

### Test Module

- `StubRepo` (struct, private) — Mock `ConversationRepository` impl for testing; returns fixed message list
- `StubRepo::msgs` (field, private) — Pre-populated message vec for test scenarios
- `key()` (fn, private) — Helper returning a test `ConversationKey` with session/node IDs
- `recall_returns_message_at_turn()` (test, async) — Verifies basic retrieval of message content by turn index
- `recall_out_of_range_returns_error()` (test, async) — Validates error response when `turn` exceeds total messages
- `recall_invalid_args_returns_error()` (test, async) — Ensures malformed JSON args are caught with "invalid_args" error prefix
- `recall_paginates_large_content()` (test, async) — Confirms pagination boundaries: first page respects default (8 KiB), `next_offset` chains correctly, final page has no `next_offset`
- `recall_small_content_single_page()` (test, async) — Verifies single-page response for small content returns `next_offset: null`
- `recall_clamps_limit_to_max()` (test, async) — Confirms oversized `limit` (1M) is clamped to 16 KiB ceiling
- `recall_offset_past_end_returns_error()` (test, async) — Validates "offset_out_of_range" error when `offset` exceeds total chars
- `recall_includes_tool_call_metadata()` (test, async) — Ensures tool_calls array from assistant messages is included in output JSON

## File-level notes

- **Redundant test assertions** (lines 206, 242): Tests assert that `"_truncated"` field is absent from response (`r.get("_truncated").is_none()`). The function never sets this field, making the assertions vacuous (always pass). Possible artifact of earlier design intent or copy-paste from another tool. [FLAG: improvement]
- **Error handling**: Comprehensive boundary checks for turn/offset validity; missing `limit` values default safely; all errors return structured JSON with context
- **Pagination design**: Character-level (not line-based) for Unicode safety; `saturating_add()` prevents overflow; `min()` handles end-of-content boundaries correctly
- **Tool metadata**: Conditionally includes `tool_call_id` (lines 130–131) and `tool_calls` array (lines 133–145) only when present in original message
- **No breaking changes**: Pure infrastructure layer; depends only on `ConversationRepository` (domain trait) and `MessageRole` enum; async dispatch pattern follows dag_tool_executor conventions
