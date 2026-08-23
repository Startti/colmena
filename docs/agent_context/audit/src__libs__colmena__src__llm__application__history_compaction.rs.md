# src/libs/colmena/src/llm/application/history_compaction.rs

**Layer:** application  **Purpose:** Pure functions for compacting conversation history by classifying messages as scaffolding or content, computing token-based boundaries, and building compacted message contexts for LLM consumption.

## Symbols

### Constants
- `SUMMARY_SKIP_THRESHOLD_CHARS` (const, public, line 7) — Threshold (250 chars) below which messages are rendered verbatim in summaries
- `SUMMARY_TARGET_CHARS` (const, public, line 8) — Target length (250 chars) for generated summaries
- `RECENT_TOKEN_BUDGET` (const, public, line 9) — Token budget (2500) for the recent message window
- `DISCOVERY_KEEP_RECENT_MSGS` (const, public, line 10) — Number of recent messages (8) to preserve in discovery round-trips
- `SUMMARY_KEEP_FIRST_MSGS` (const, public, line 11) — Number of initial system/user messages (2) to always keep uncompacted
- `SUMMARY_MAX_LINES` (const, public, line 12) — Maximum line count (100) in the summary block before dropping oldest lines
- `SUMMARIZE_PER_LOAD_CAP` (const, public, line 13) — Maximum summaries (30) to compute per load to avoid LLM calls
- `DISCOVERY_TOOL_NAMES` (const, private, line 15) — Array of tool names (`load_skill`, `describe_tool`) marking discovery scaffolding

### Types
- `ValueClass` (enum, public, lines 19–22) — Classifies a message as `Scaffolding` (discovery tool round-trips) or `Content` (actual agent/user/tool work)

### Functions
- `rendered_size(msg: &LlmMessage) -> usize` (public, lines 25–33) — Computes character-count size of message content plus all serialized tool_call arguments
- `est_tokens(msg: &LlmMessage) -> usize` (private, lines 36–38) — Estimates token count as rendered_size / 4 + 1, aligned with repo test dumps
- `classify_value_class(messages: &[LlmMessage]) -> Vec<ValueClass>` (public, lines 41–78) — Classifies each message by scanning for discovery tool calls and marking round-trips as Scaffolding
- `recent_boundary_by_tokens(messages: &[LlmMessage], classes: &[ValueClass], token_budget: usize) -> usize` (public, lines 88–109) — Walks messages backwards from end, accumulating token count only of Content messages, returns index of first recent message. **This is the panic fix.** The return value is clamped to `[0, messages.len())` for non-empty input (`b.min(messages.len().saturating_sub(1))`) — it can no longer leak the initial `messages.len()` accumulator when the walk breaks on its very first iteration (the newest message alone exceeds `token_budget`). This bounds the INDEX only, not the message content — see the `///` doc comment on the function for the full guarantee.
- `bridge_truncate(s: &str, cap: usize) -> String` (private, lines 117–123) — Truncates string to char-safe limit with ellipsis suffix for runtime display (never persisted). Used for old-zone summary lines and tool-call argument previews, not for the newest message.
- `role_tag(m: &LlmMessage) -> &'static str` (private, lines 125–132) — Returns human-readable role tag ("USER", "SYSTEM", "AGENT", "TOOL") for message classification in summary
- `build_compacted_messages(stored: &[StoredMessage], key: &ConversationKey, repo: &dyn ConversationRepository, summarizer: Option<&Arc<dyn MessageSummarizer>>, recent_token_budget: usize) -> Vec<LlmMessage>` (public async, lines 135–283) — Main entry point: builds compacted message context by keeping first N messages verbatim, summarizing old messages via optional summarizer or fallback digest/truncation, and preserving recent messages in full; returns flattened vector with optional System summary block. When the newest message alone exceeds `recent_token_budget`, every early-return path (`total <= keep_first + 1` at line 146–148; the pair-guard's `b <= keep_first` at line 157–159) returns the raw, unmodified `messages` vector — the recent window degenerates to exactly that one message and it ships **verbatim, for every role**. There is no content transformation of the newest message anywhere in this function.

### Test Helpers
- `tests::tc(id: &str, name: &str) -> ToolCall` (private, lines 291–299) — Constructs minimal ToolCall for test setup
- `tests::Shape` (enum, private, lines 323–329) — Message-count/class-distribution shapes (`AllContent`, `AllScaffolding`, `Alternating`, `OnlyLastOversized`, `OnlyFirstOversized`) enumerated by the boundary invariant sweep
- `tests::build(n, size, shape) -> (Vec<LlmMessage>, Vec<ValueClass>)` (private, lines 344–375) — Builds a message/class fixture for one point in the invariant sweep's state space
- `tests::StubSummarizer` (struct, lines 477–487) — Mock MessageSummarizer returning fixed "RESUMEN" string for integration tests
- `tests::FailSummarizer` (struct, lines 489–499) — Mock MessageSummarizer that panics on call. Used as a trip-wire proving a given fixture's newest/recent message is never routed through the old-zone summarization ladder.
- `tests::ckey() -> ConversationKey` (private, lines 501–507) — Helper creating standard test ConversationKey
- `tests::build_oversized_newest_tool_fixture(repo, k, tool_content) -> usize` (private async, lines 687–712) — Fixture used only by `recent_window_is_never_empty`: keep_first + old-zone filler + Assistant(tool_calls) + Tool(oversized newest). Returns the Tool's index (unused by its current caller).

### Tests
- `classifies_scaffolding_vs_content()` (test, lines 302–318) — Verifies that discovery tools (describe_tool, load_skill) and their responses are marked Scaffolding, others Content
- `recent_boundary_is_always_a_valid_index()` (test, lines 378–408) — Combinatorial contract sweep (message count × size × budget × shape) proving `recent_boundary_by_tokens` always returns a slice-safe, and (for non-empty input) index-safe, boundary. This is the regression guard for the class of bug, not just the reported case — it fails at `n=1, budget=0`, a case neither repro test below reaches.
- `recent_boundary_counts_only_content_tokens()` (test, lines 411–419) — Verifies recent boundary respects token budget and skips Scaffolding messages in count
- `oversized_message_leaves_the_recent_window_on_the_next_turn()` (test, lines 427–451) — Verifies the cost of keeping an oversized newest message is bounded to a single turn: once a message is appended after it, the backward walk breaks on the (now second-newest) oversized message instead, and it leaves the recent window on its own with no extra rule needed
- `rendered_size_includes_tool_call_args()` (test, lines 454–467) — Verifies rendered_size sums message content and all tool_call argument text
- `old_long_nl_gets_summarized_and_cached_recent_stays_full()` (async test, lines 510–533) — Verifies old large messages are summarized and cached via repository, recent messages stay full, when token budget forces boundary
- `short_messages_pass_verbatim_no_summary_block()` (async test, lines 536–548) — Verifies messages below threshold pass through unchanged without triggering summary block
- `repro_adp_panic_last_content_message_alone_exceeds_budget()` (async test, lines 551–597) — PANIC regression test for the exact shape ADP reported: `total == 4`, newest message a 40,000-char `Tool` result, which lands on the pair-guard early return (`b <= keep_first`) and returns the raw vector unmodified. Asserts the run does not panic AND the newest Tool message survives intact and verbatim (role, `tool_call_id`, and byte-identical content), not merely that the output is non-empty.
- `repro_panic_also_fires_on_a_large_user_prompt()` (async test, lines 600–617) — Regression test proving the panic was never resume-specific: an oversized newest `User` prompt on an ordinary turn triggers the same precondition
- `structured_tool_result_becomes_digest_without_calling_summarizer()` (async test, lines 620–681) — Verifies large structured JSON tool results (≥250 chars) in the OLD zone are digested deterministically without invoking the summarizer, and the digest is never persisted as a cached summary
- `oversized_newest_user_prompt_stays_verbatim()` (async test, lines 715–738) — Pins that an oversized newest `User` message survives verbatim, byte-identical to the original
- `oversized_newest_assistant_stays_verbatim()` (async test, lines 741–764) — Pins that an oversized newest `Assistant` message survives verbatim, byte-identical to the original (truncating risks corrupting pending `tool_calls` arguments)
- `recent_window_is_never_empty()` (async test, lines 767–813) — Cross-fixture pin that the synthesized `System` summary is never the last message in the output, for both an oversized-newest-Tool and an oversized-newest-User shape
- `parallel_tool_calls_with_oversized_last_result_ship_the_history_raw()` (async test, lines 821–869) — Pins the second outcome of the pair guard: with two parallel tool calls whose last result is oversized, the guard walks `b` back over both `Tool` messages to `keep_first`, the early return at lines 157–159 fires, and the whole history ships raw with no summary block. Documented in `docs/developer_guide/15_memory_guide.md` §Compactación → "Limitación conocida"

## File-level notes

- **LLM-facing strings**: The summary block and line formatting includes Spanish prose (`andamiaje`, `RESUMEN`, `recall_history`) because this is domain language shown to the model, not documentation or code comments.
- **Error handling**: Graceful fallbacks present throughout:
  - Summarizer failure falls back to truncation
  - System message construction failure falls back to cloning original message
- **Token estimation**: Division by 4 is a heuristic aligned with repository test dumps, not an LLM library standard.
- **Deterministic digest vs. cached summary**: Structured tool results in the OLD zone are distinguished from freeform text (digested in-memory, never cached, vs. cached after summarization), protecting against double-summarization on reload.
- **Tool name mapping**: Built once per compaction to avoid repeated lookups when formatting Scaffolding markers for display.
- **One bounding mechanism, index-only**: the clamp in `recent_boundary_by_tokens` (lines 88–109) bounds the boundary INDEX for every role, keeping the pair guard and the slice `messages[b..]` safe. It never bounds the message CONTENT — an oversized newest message of any role (`User`, `Assistant`, or `Tool`) ships verbatim. A prior revision of this fix also gated a content-degrading transform for the newest `Tool` message specifically; that piece was removed as a scope reduction (2026-08-22) in favor of a separate, still-in-design mechanism that anchors the recent-window boundary to the current interaction. See `docs/developer_guide/15_memory_guide.md` §Compactación → "Ventana de recientes cuando el mensaje más nuevo excede el presupuesto" for the resulting known limitation.
- **Guard against malformed pairs** (lines 154–156): Ensures Tool messages are never isolated without their preceding Assistant message by walking backwards. This guard now always operates on an in-bounds `b` — `recent_boundary_by_tokens`'s clamped contract closes the panic at its source, so this guard can no longer read `messages[b]` out of bounds.
- **Deferred imports** (lines 111–114): Tool digest and repository traits imported inline near their use in `build_compacted_messages`, keeping module dependencies at function scope.
