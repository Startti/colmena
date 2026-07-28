# src/libs/colmena/src/llm/application/history_compaction.rs

**Layer:** application  **Purpose:** Pure functions for compacting conversation history by classifying messages as scaffolding or content, computing token-based boundaries, and building compacted message contexts for LLM consumption.

## Symbols

### Constants
- `SUMMARY_SKIP_THRESHOLD_CHARS` (const, public) — Threshold (250 chars) below which messages are rendered verbatim in summaries
- `SUMMARY_TARGET_CHARS` (const, public) — Target length (250 chars) for generated summaries
- `RECENT_TOKEN_BUDGET` (const, public) — Token budget (2500) for the recent message window
- `DISCOVERY_KEEP_RECENT_MSGS` (const, public) — Number of recent messages (8) to preserve in discovery round-trips
- `SUMMARY_KEEP_FIRST_MSGS` (const, public) — Number of initial system/user messages (2) to always keep uncompacted
- `SUMMARY_MAX_LINES` (const, public) — Maximum line count (100) in the summary block before dropping oldest lines
- `SUMMARIZE_PER_LOAD_CAP` (const, public) — Maximum summaries (30) to compute per load to avoid LLM calls
- `DISCOVERY_TOOL_NAMES` (const, private) — Array of tool names (`load_skill`, `describe_tool`) marking discovery scaffolding

### Types
- `ValueClass` (enum, public, lines 19–22) — Classifies a message as `Scaffolding` (discovery tool round-trips) or `Content` (actual agent/user/tool work)

### Functions
- `rendered_size(msg: &LlmMessage) -> usize` (public, lines 25–33) — Computes character-count size of message content plus all serialized tool_call arguments
- `est_tokens(msg: &LlmMessage) -> usize` (private, lines 36–38) — Estimates token count as rendered_size / 4 + 1, aligned with repo test dumps
- `classify_value_class(messages: &[LlmMessage]) -> Vec<ValueClass>` (public, lines 41–78) — Classifies each message by scanning for discovery tool calls and marking round-trips as Scaffolding
- `recent_boundary_by_tokens(messages: &[LlmMessage], classes: &[ValueClass], token_budget: usize) -> usize` (public, lines 82–99) — Walks messages backwards from end, accumulating token count only of Content messages, returns index of first recent message within budget
- `bridge_truncate(s: &str, cap: usize) -> String` (private, lines 107–113) — Truncates string to char-safe limit with ellipsis suffix for runtime display (never persisted)
- `role_tag(m: &LlmMessage) -> &'static str` (private, lines 115–122) — Returns human-readable role tag ("USER", "SYSTEM", "AGENT", "TOOL") for message classification in summary
- `build_compacted_messages(stored: &[StoredMessage], key: &ConversationKey, repo: &dyn ConversationRepository, summarizer: Option<&Arc<dyn MessageSummarizer>>, recent_token_budget: usize) -> Vec<LlmMessage>` (public async, lines 125–272) — Main entry point: builds compacted message context by keeping first N messages verbatim, summarizing old messages via optional summarizer or fallback digest/truncation, and preserving recent messages in full; returns flattened vector with optional System summary block

### Test Helpers
- `tests::tc(id: &str, name: &str) -> ToolCall` (private, lines 280–288) — Constructs minimal ToolCall for test setup
- `tests::StubSummarizer` (struct, lines 344–354) — Mock MessageSummarizer returning fixed "RESUMEN" string for integration tests
- `tests::FailSummarizer` (struct, lines 356–366) — Mock MessageSummarizer that panics on call, used to assert summarizer is NOT invoked for structured tool results
- `tests::ckey() -> ConversationKey` (private, lines 368–374) — Helper creating standard test ConversationKey

### Tests
- `classifies_scaffolding_vs_content()` (test, lines 291–307) — Verifies that discovery tools (describe_tool, load_skill) and their responses are marked Scaffolding, others Content
- `recent_boundary_counts_only_content_tokens()` (test, lines 310–318) — Verifies recent boundary respects token budget and skips Scaffolding messages in count
- `rendered_size_includes_tool_call_args()` (test, lines 321–334) — Verifies rendered_size sums message content and all tool_call argument text
- `old_long_nl_gets_summarized_and_cached_recent_stays_full()` (async test, lines 376–400) — Verifies old large messages are summarized and cached via repository, recent messages stay full, when token budget forces boundary
- `short_messages_pass_verbatim_no_summary_block()` (async test, lines 402–415) — Verifies messages below threshold pass through unchanged without triggering summary block
- `structured_tool_result_becomes_digest_without_calling_summarizer()` (async test, lines 417–479) — Verifies large structured JSON tool results (≥250 chars) are digested deterministically without invoking summarizer, and digest is never persisted as cached summary

## File-level notes

- **LLM-facing strings**: The summary block and line formatting (lines 177, 193, 202, 213, 202–243, 246–248) includes Spanish prose (`andamiaje`, `RESUMEN`, `recall_history`) because this is domain language shown to the model, not documentation or code comments.
- **Error handling**: Graceful fallbacks present throughout:
  - Line 209: summarizer failure falls back to truncation
  - Line 266: system message construction failure falls back to cloning original message
- **Token estimation**: Division by 4 is a heuristic aligned with repository test dumps, not an LLM library standard.
- **Deterministic digest vs. cached summary**: Lines 194–203 distinguish between structured tool results (digested in-memory, never cached) and freeform text (cached after summarization), protecting against double-summarization on reload.
- **Tool name mapping** (lines 151–159): Built once per compaction to avoid repeated lookups when formatting Scaffolding markers for display.
- **Guard against malformed pairs** (lines 143–145): Ensures Tool messages are never isolated without their preceding Assistant message by walking backwards.
- **Deferred imports** (lines 101–104): Tool digest and repository traits imported inline near their use in `build_compacted_messages`, keeping module dependencies at function scope.

