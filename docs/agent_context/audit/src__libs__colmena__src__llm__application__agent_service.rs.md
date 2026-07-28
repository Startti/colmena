# src/libs/colmena/src/llm/application/agent_service.rs

**Layer:** application  
**Purpose:** ReAct agent loop orchestration service that coordinates LLM calls with tool execution, including consecutive-call loop guards, lazy tool loading guards, load_attachment sentinel handling, discovery tool compaction, and forced-synthesis rescue on iteration ceiling.

## Symbols

### Constants
- `DISCOVERY_KEEP_RECENT_MSGS` (const usize) — trailing message count to preserve verbatim when compacting old discovery/scaffolding tool results (load_skill, describe_tool)
- `DISCOVERY_TOOL_NAMES` (const &[&str]) — registry of "andamiaje" tool names whose results are collapsed to markers
- `REPEAT_NUDGE_TEXT` (const &str) — LLM-facing text prepended when a tool call with identical (name+args) signature repeats
- `RESCUE_SYNTHESIS_TEXT` (const &str) — LLM-facing instruction for forced final synthesis ("rescue") appended as user message when loop guard triggers

### Type Aliases
- `ToolsProvider` (pub type) — closure `Fn(&[LlmMessage]) -> Vec<ToolDefinition> + Send + Sync` that derives the tool list to send on each ReAct iteration (enables lazy tool loading without teaching AgentService about lazy mode)

### Traits
- `LoadAttachmentResolver` (pub async trait) — resolves document_id to ready-to-use FileData; implementations verify/refresh provider_file_id and return recoverable error strings; returning Ok(None) means document_id not in session (agent closes tool call with not_found result)

### Structs
- `AgentRunParams<'a>` (pub struct) — parameters for agent execution: session_id, optional prompt/messages, config, tools, tool_executor, max_tool_repeats (default 3), max_turns (env fallback 50), optional on_token callback, optional tools_provider (lazy loading), optional attachment_resolver, optional agent_session_id (for attachment resolver), optional lazy_catalog_names (describe-before-use guard set)
- `AgentService` (pub struct) — ReAct orchestrator holding Arc<dyn LlmRepository>, Arc<dyn ConversationRepository>, optional Arc<dyn MessageSummarizer> for history compaction

### Implementations
- `impl AgentService`
  - `new()` (pub fn) — constructor taking llm_repository and conversation_repository, initializes message_summarizer to None
  - `with_message_summarizer()` (pub fn) — builder that injects cheap-model summarizer for per-message cache-aware compaction, returns Self for chaining
  - `run()` (pub async fn) — main ReAct loop (127–717): loads conversation history with cached summaries, strips stale leading temporal blocks (migration 2026-06-11), adds user prompt or resumes from existing messages, computes semantic-summary base once, loops until max_turns or LLM stops requesting tools, manages consecutive-signature loop guard (nudge at 2+, rescue at max_tool_repeats), detects SUSPENDED/LOAD_ATTACHMENT sentinels mid-execution, accumulates usage and content across turns, returns LlmResponse with final answer or suspend info
  - `invoke_llm()` (async fn) — one LLM round-trip (streaming or call) with LlmMessageStart/Finish bracket; streams accumulate tool call chunks by index into HashMap, emit LlmStreamPart callbacks for observability, return assembled response and completion usage

### Functions
- `default_hard_turn_cap()` (fn) — reads COLMENA_HARD_TURN_CAP env var (positive integer, fallback 50); pure cost/termination backstop that reaches forced synthesis same as loop guard
- `accumulate_usage()` (fn) — folds one response's usage (prompt/completion/thinking/cache tokens) into running cumulative
- `tool_call_signature()` (fn) — canonical (name, arguments) signature used to detect repeated tool calls; parses JSON arguments with recursive key-sorting so {"a":1,"b":2} and {"b":2,"a":1} collapse to one key; invalid JSON falls back to raw string; uses \u{0} separator so name and args never collide
- `canonical_json()` (fn) — deterministic, key-sorted serialization of JSON value (for signatures only); recursively sorts object keys, preserves array order, stringifies scalars
- `strip_leading_temporal_block()` (fn) — migration shim (2026-06-11): removes stale "## Temporal & Geographic Context" header block from loaded system messages (pre-fix conversations baked it into persisted system; fresh temporal block now injected per turn as volatile suffix); returns empty if block is only section, unchanged if no header
- `strip_temporal_from_stored()` (fn) — non-dropping wrapper that applies temporal strip to StoredMessage vector without changing count/order (so ordinals stay aligned with DB / recall_history); applied to stored_now before compaction
- `compact_discovery_tools_in_history()` (fn) — compacts load_skill/describe_tool results older than keep_recent_msgs into one-line markers (saves ~95% token cost for smoke runs); idempotent (skips already-marked messages); provider-agnostic (each adapter serializes LlmMessage in its own format); non-discovery tool results stay intact

### Test Module (lines 1020–2789)
- `stateful_conv_mock()` (fn) — configures MockConversationRepo backed by shared mutable state so get_with_summaries reflects appends via add_message (agent loop reloads history twice, so stateless snapshot would return stale)
- Helper constructors: `fc()`, `tool_call()`, `loop_tool_call()`, `text_response()`, `tool_call_response()`, `named_tool_call()` — build test fixtures
- **18 async tests:**
  - `strip_temporal_removes_leading_block_keeps_rest()` — strip leaves rest of system message
  - `strip_temporal_drops_block_when_only_section()` — strip returns empty when temporal is sole content
  - `strip_temporal_leaves_non_temporal_system_untouched()` — non-temporal system unchanged
  - `strip_temporal_from_stored_strips_legacy_system_keeps_count()` — non-dropping strip preserves ordinals
  - `tool_call_signature_is_key_order_independent()` — canonical JSON collapses key order
  - `tool_call_signature_is_name_and_args_sensitive()` — name and args changes alter signature
  - `tool_call_signature_handles_nested_and_invalid_json()` — nested keys normalized, invalid JSON falls back to raw string
  - `compact_noop_when_history_shorter_than_keep_recent()` — no compaction below threshold
  - `compact_noop_when_no_load_skill_in_history()` — non-discovery tools unchanged
  - `compact_replaces_old_load_skill_results_with_markers()` — old discovery results compacted, recent preserved
  - `compact_preserves_recent_load_skill_results()` — recent results stay verbatim
  - `compact_skips_non_load_skill_tools_even_if_old()` — crdt_doc_run_python etc. never compacted
  - `compact_is_idempotent_does_not_remark_already_marked()` — re-compacting marked message is no-op
  - `discovery_compaction_markers_old_describe_tool()` — describe_tool results also compacted
  - `test_agent_service_simple_response_no_tools()` — happy path: prompt → LLM text answer
  - `test_agent_service_with_tool_call()` — tool execution: prompt → LLM tool call → execute → LLM final answer
  - `run_with_no_prompt_continues_from_existing_messages()` — resume path: no prompt means continue from history
  - `repeated_signature_nudges_then_rescues_with_synthesis()` — loop guard: 1st executes, 2nd nudged, 3rd triggers rescue (forced synthesis)
  - `lazy_guard_redirects_undiscovered_call_to_schema()` — lazy loading: cataloged tool NOT loaded this turn → redirect to schema via describe_tool
  - `distinct_signatures_are_never_nudged()` — different (name+args) signatures all execute (no nudge)
  - `streak_resets_when_a_different_signature_appears()` — A-A-B-A: streak resets after B, A starts fresh
  - `max_turns_ceiling_triggers_synthesis_not_error()` — max_turns reached → forced synthesis, not error
  - `single_shot_max_turns_one_returns_directly()` — max_turns=1 + text answer → direct return (guard never engages)
  - `two_identical_calls_in_one_turn_nudges_the_second()` — same turn, twin calls (same signature) → 1st executes, 2nd nudged
  - `three_identical_calls_in_one_turn_rescue_intra_turn()` — same turn, triplet calls → 1st executes, 2nd nudged, 3rd hits max and flags rescue
  - `detects_suspended_tool_result_and_short_circuits()` — SUSPENDED sentinel → short-circuit, persist only assistant (not tool result), return suspended response
  - `load_attachment_sentinel_injects_synthetic_user_message_and_continues()` — LOAD_ATTACHMENT sentinel → resolve doc → synthetic user_with_files in-memory only, marker persisted, continue
  - `load_attachment_emits_tool_output_available_for_sse()` — load_attachment emits LlmToolCallFinish SSE event with metadata (no content)
  - `load_attachment_synthetic_message_is_not_persisted_to_history()` — dual behavior: in-memory stream sees synthetic user_with_files, persisted history sees only marker

## File-level notes

- **Architecture alignment**: Implements ReAct pattern as application-layer use case, delegating LLM calls and conversation persistence to domain-level traits (LlmRepository, ConversationRepository). Tool execution delegated to ToolExecutor trait.
- **Migration shim**: Two-point temporal block stripping (load + reload-for-compaction) handles legacy conversations persisted before 2026-06-11 fix without re-sending stale blocks.
- **Loop guards implemented**:
  1. Consecutive-signature guard: nudges at 2+, rescues at max_tool_repeats; resets on different signature (model progress).
  2. Lazy describe-before-use guard: when lazy mode enabled (lazy_catalog_names Some), undiscovered cataloged tools redirected to schema via describe_tool (never executed blind).
  3. Hard turn ceiling: max_turns env fallback ensures termination even on infinite ReAct loops.
- **Streaming & observability**: Supports streaming and non-streaming LLM calls; emits on_token callbacks for SSE (LlmMessageStart, Content, ToolCallStart, ToolCallFinish, LlmMessageFinish); accumulates tool call chunks by index into HashMap during stream reassembly.
- **Sentinel handling**: Detects SUSPENDED (short-circuits, returns suspend info) and LOAD_ATTACHMENT (resolves doc, injects synthetic user_with_files in-memory, persists marker only, continues) mid-execution via JSON parsing of tool result.
- **Discovery tool compaction**: load_skill, describe_tool results replaced by markers for older messages (saves ~95% redundant tokens); idempotent; non-discovery tools (crdt_doc_run_python, sql_query, etc.) preserved (stateful).
- **Comprehensive test coverage**: 18 tests exercise happy path, tool execution, loop guards (streak, rescue, intra-turn), lazy loading, max_turns, single-shot, suspend/HITL, load_attachment (persisted vs. ephemeral, SSE).
- **Code quality**: No panics, no raw unwraps on non-trivial Results, all async boundaries correct, error propagation via ?, explicit error handling in critical paths (attachment resolution, tool execution, JSON sentinel parsing).
