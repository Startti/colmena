# src/libs/colmena/src/llm/application/history_compaction.rs

**Layer:** application  **Purpose:** Pure functions for compacting conversation history by classifying messages as scaffolding or content, finding the structural boundary of the currently-open interaction, and building compacted message contexts for LLM consumption.

## Symbols

### Constants
- `SUMMARY_SKIP_THRESHOLD_CHARS` (const, public, line 7) — Threshold (250 chars) below which messages are rendered verbatim in summaries
- `SUMMARY_TARGET_CHARS` (const, public, line 8) — Target length (250 chars) for generated summaries
- `DISCOVERY_KEEP_RECENT_MSGS` (const, public, line 9) — Number of recent messages (8) to preserve in discovery round-trips
- `SUMMARY_KEEP_FIRST_MSGS` (const, public, line 10) — Number of initial system/user messages (2) to always keep uncompacted
- `SUMMARY_MAX_LINES` (const, public, line 11) — Maximum line count (100) in the summary block before dropping oldest lines
- `SUMMARIZE_PER_LOAD_CAP` (const, public, line 12) — Maximum summaries (30) to compute per load to avoid LLM calls
- `DISCOVERY_TOOL_NAMES` (const, private, line 14) — Array of tool names (`load_skill`, `describe_tool`) marking discovery scaffolding

### Types
- `ValueClass` (enum, public, lines 18–21) — Classifies a message as `Scaffolding` (discovery tool round-trips) or `Content` (actual agent/user/tool work)

### Functions
- `rendered_size(msg: &LlmMessage) -> usize` (public, lines 24–32) — Computes character-count size of message content plus all serialized tool_call arguments
- `classify_value_class(messages: &[LlmMessage]) -> Vec<ValueClass>` (public, lines 35–72) — Classifies each message by scanning for discovery tool calls and marking round-trips as Scaffolding
- `current_interaction_start(messages: &[LlmMessage]) -> usize` (public, lines 85–94) — **This is the compaction boundary since 2026-08-22.** Returns the index just after the last `assistant` message carrying no tool calls (the ReAct loop's own close condition, cross-referenced to `agent_service.rs:353/359/676` in the doc comment). `0` when no interaction has closed yet (the whole history is current); `messages.len()` when the newest message itself closes an interaction (reachable on a resume with no new prompt — `build_compacted_messages` special-cases this so the recent window is never empty).
- `bridge_truncate(s: &str, cap: usize) -> String` (private, lines 102–108) — Truncates string to char-safe limit with ellipsis suffix for runtime display (never persisted). Used for old-zone summary lines and tool-call argument previews, not for the newest message.
- `role_tag(m: &LlmMessage) -> &'static str` (private, lines 110–117) — Returns human-readable role tag ("USER", "SYSTEM", "AGENT", "TOOL") for message classification in summary
- `build_compacted_messages(stored: &[StoredMessage], key: &ConversationKey, repo: &dyn ConversationRepository, summarizer: Option<&Arc<dyn MessageSummarizer>>) -> Vec<LlmMessage>` (public async, lines 120–275) — Main entry point: builds compacted message context by keeping first N messages verbatim, summarizing everything before the open interaction via optional summarizer or fallback digest/truncation, and shipping the open interaction (from `current_interaction_start`) verbatim regardless of its size — no token budget of any kind gates this. When nothing is open (`b == messages.len()`), `b` is decremented by one so the closing message stays in the recent window instead of the wire ending on the summary block (lines 146–148). The early return at `total <= keep_first + 1` (lines 130–132) and at `b <= keep_first` (lines 149–151) both hand back the raw, unmodified `messages` vector.

### Test Helpers
- `tests::tc(id: &str, name: &str) -> ToolCall` (private, lines 283–291) — Constructs minimal ToolCall for test setup
- `tests::StubSummarizer` (struct, lines 336–346) — Mock MessageSummarizer returning fixed "RESUMEN" string for integration tests
- `tests::FailSummarizer` (struct, lines 348–358) — Mock MessageSummarizer that panics on call. Used as a trip-wire proving a given fixture's newest/recent message is never routed through the old-zone summarization ladder.
- `tests::ckey() -> ConversationKey` (private, lines 360–366) — Helper creating standard test ConversationKey
- `tests::build_oversized_newest_tool_fixture(repo, k, tool_content) -> usize` (private async, lines 483–512) — Fixture used only by `recent_window_is_never_empty`: keep_first + old-zone filler + a closing Assistant (so a summary zone exists) + Assistant(tool_calls) + Tool(oversized newest). Returns the Tool's index (unused by its current caller).

### Tests
- `classifies_scaffolding_vs_content()` (test, lines 293–310) — Verifies that discovery tools (describe_tool, load_skill) and their responses are marked Scaffolding, others Content
- `rendered_size_includes_tool_call_args()` (test, lines 312–326) — Verifies rendered_size sums message content and all tool_call argument text
- `old_long_nl_gets_summarized_and_cached_recent_stays_full()` (async test, lines 368–395) — Verifies old large messages are summarized and cached via repository while the open interaction stays full raw. Fixture inserts a closing assistant after the third message so msg0..msg2 land in the old zone and msg3..msg9 stay recent.
- `short_messages_pass_verbatim_no_summary_block()` (async test, lines 397–409) — Verifies messages below threshold pass through unchanged without triggering summary block (the `total <= keep_first + 1` early return, unaffected by the boundary rewrite)
- `structured_tool_result_becomes_digest_without_calling_summarizer()` (async test, lines 411–477) — Verifies large structured JSON tool results (≥250 chars) in the OLD zone are digested deterministically without invoking the summarizer, and the digest is never persisted as a cached summary. Fixture closes the interaction right after the structured tool result so it lands in the old zone instead of the open interaction.
- `oversized_newest_user_prompt_stays_verbatim()` (async test, lines 514–541) — Pins that an oversized newest `User` message survives verbatim, byte-identical to the original, because it belongs to the open interaction
- `oversized_newest_assistant_stays_verbatim()` (async test, lines 543–570) — Pins that an oversized newest `Assistant` message survives verbatim, byte-identical to the original (truncating risks corrupting pending `tool_calls` arguments)
- `recent_window_is_never_empty()` (async test, lines 572–620) — Cross-fixture pin that the synthesized `System` summary is never the last message in the output, for both an oversized-newest-Tool and an oversized-newest-User shape
- `interaction_start_is_after_the_last_assistant_without_tool_calls()` (test, lines 622–634) — `current_interaction_start` basic case: index right after the closing assistant
- `an_assistant_with_an_empty_tool_call_vec_also_closes()` (test, lines 636–646) — The ReAct loop returns on both `Some(vec![])` and `None`; detecting the close with `is_none()` alone would miss the non-streaming path
- `several_unanswered_user_messages_all_belong_to_the_open_interaction()` (test, lines 648–657) — Multiple unanswered user turns after a close are all part of the same open interaction
- `a_closing_assistant_as_the_newest_message_leaves_nothing_open()` (test, lines 659–669) — Reachable on a resume with no new prompt: `current_interaction_start` returns `messages.len()` when the newest stored message itself closes
- `without_a_closed_interaction_everything_is_current()` (test, lines 671–681) — No closing assistant anywhere (including the empty slice) returns `0`
- `the_current_question_survives_next_to_an_oversized_tool_result()` (async test, lines 683–734) — **The defect this task closes.** With the old budget-driven boundary, an oversized tool result could push the cut back far enough to summarize away the user's own question that triggered it. Pins that both the open interaction's question and its oversized tool result travel verbatim.
- `a_closed_newest_interaction_still_leaves_a_recent_message()` (async test, lines 736–763) — The recent window must never be empty even when nothing is open (newest message closes its own interaction) — the last message on the wire must not be the `System` summary block.
- `interaction_start_uses_the_last_close_not_the_first()` (test, lines 765–784) — Regression guard: the backward scan must find the LAST closing assistant, not the first; would fail if the scan ran forward instead of `.rev()`

## File-level notes

- **LLM-facing strings**: The summary block and line formatting includes Spanish prose (`andamiaje`, `RESUMEN`, `recall_history`) because this is domain language shown to the model, not documentation or code comments.
- **Error handling**: Graceful fallbacks present throughout:
  - Summarizer failure falls back to truncation
  - System message construction failure falls back to cloning original message
- **Deterministic digest vs. cached summary**: Structured tool results in the OLD zone are distinguished from freeform text (digested in-memory, never cached, vs. cached after summarization), protecting against double-summarization on reload.
- **Tool name mapping**: Built once per compaction to avoid repeated lookups when formatting Scaffolding markers for display.
- **Structural boundary, not a token budget (2026-08-22)**: `current_interaction_start` replaced the token-budget walk (`recent_boundary_by_tokens`, `RECENT_TOKEN_BUDGET`, and their `est_tokens` helper — all deleted) that previously decided the recent-window cut. The old mechanism could push the cut back arbitrarily far when a single message (typically an oversized tool result) blew the budget, summarizing away the very question that triggered it — see the `the_current_question_survives_next_to_an_oversized_tool_result` test above, which is the regression guard for exactly that failure mode. The new boundary is structural: everything from the open interaction's first message onward ships verbatim, whatever it weighs, because it is by definition what the model is currently working on. There is no size cap on the open interaction.
- **No pair guard needed**: the old code walked `b` backward over trailing `Tool` messages so a `Tool` never shipped without its `Assistant`. That guard is gone — `current_interaction_start` always lands on an interaction's first message, which can never be a `Tool` orphaned from its `Assistant` (a `Tool` message is only ever preceded by the `Assistant` that called it, never a boundary).
- **Deferred imports** (lines 96–99): Tool digest and repository traits imported inline near their use in `build_compacted_messages`, keeping module dependencies at function scope.
