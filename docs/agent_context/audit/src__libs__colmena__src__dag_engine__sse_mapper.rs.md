# src/libs/colmena/src/dag_engine/sse_mapper.rs

**Layer:** infrastructure  **Purpose:** Stateful converter from `DagExecutionEvent` domain events to SSE Data Stream Protocol JSON frames, handling nested subgraph events with level/path tracking and token accumulation.

## Symbols

- `SseMapper` (struct) — Stateful mapper maintaining text block IDs, node types, and cumulative token counters; mirrors the canonical SSE output of `colmena run`
- `SseMapper::default()` (impl Default) — Returns `Self::new()`
- `SseMapper::new()` (fn) — Constructor initializing empty maps, sets, and zero token counters
- `SseMapper::deep_base()` (fn, private) — Unwraps nested `SubgraphWrapped` events recursively to extract the base (non-wrapped) event; tolerates both flat and legacy nested wrappers
- `SseMapper::level_and_path()` (fn, private) — Computes nesting level (sum of accumulated `depth` values) and lineage path (from outermost `SubgraphWrapped.path` or base event `node_id`); level-0 events return path as node ID
- `SseMapper::map()` (fn, pub) — Main public method converting one `DagExecutionEvent` into a `Vec<Value>` of SSE JSON protocol parts; performs two-phase processing: state management (text blocks, tokens, tool IDs) then protocol mapping; injects additive `level` and `path` fields into every output frame
- `SseMapper::clean_inputs()` (fn, private) — Filters out internal fields (those starting with `__` and `session_id`) from an inputs object for SSE emission
- `tests::tool_call_sequence()` (fn, test helper) — Returns a sequence of four `DagExecutionEvent` frames representing a streamed tool call
- `tests::test_tool_input_start_emitted_once_before_first_delta()` (test) — Verifies that `tool-input-start` is emitted only on the first `LlmToolCall` chunk, subsequent chunks emit only `tool-input-delta`
- `tests::test_tool_input_available_and_output()` (test) — Verifies `tool-input-available` on `LlmToolCallStart` and `tool-output-available` on `LlmToolCallFinish`
- `tests::test_subgraph_tool_input_start_emitted_once()` (test) — Verifies that wrapped `LlmToolCall` tracks tool IDs separately (`seen_sub_tool_ids`) and emits `subgraph-tool-input-start` only once per call ID
- `tests::test_top_level_and_subgraph_tool_ids_are_independent()` (test) — Verifies that a tool_id can appear in both top-level and subgraph contexts without suppressing the subgraph-level start event
- `tests::cancelled_maps_to_cancelled_then_finish()` (test) — Verifies that a `Cancelled` event emits both a UX `cancelled` frame and a `finish` terminator; validates state cleanup
- `tests::progress_maps_to_status_running_part()` (test) — Verifies that `Progress` events map to `status` frames with `stage: "running"`
- `tests::double_nested_wrapped_llm_token_maps_to_subgraph_text_delta_level_2()` (test) — Regression test (Fase A): verifies that doubly-nested `SubgraphWrapped` (level 2) accumulates depth across layers and produces `subgraph-text-delta` with correct `level` and `path` (was dropping double-nested events)
- `tests::level_zero_frames_carry_level_and_path()` (test) — Fase B: verifies that non-wrapped events carry `level: 0` and `path` = node_id
- `tests::message_boundaries_forward_as_agent_turn()` (test) — Fase C5: verifies that `LlmMessageStart`/`LlmMessageFinish` forward as lightweight `agent-turn` frames (never `finish`/`error`); tests both top-level and wrapped variants
- `tests::wrapped_progress_maps_to_status_running_part()` (test) — Verifies that wrapped `Progress` events map to `status` frames with correct level and path

## File-level notes

- **Duplication risk (improvement):** The mapper implements a two-phase algorithm: Phase 1 manages state (text blocks, tokens, tool IDs, lines 96–174) and Phase 2 generates protocol output (lines 177–623). Within Phase 2, every non-wrapped event handling (lines 178–439) has a near-identical duplicate in the `SubgraphWrapped` inner match (lines 440–622) with the same JSON structure but different type prefixes (`"text-delta"` → `"subgraph-text-delta"`, etc.). This duplication spans ~200 lines and creates maintenance risk: adding a new event type, changing a field, or fixing a bug requires edits in two places. Candidate refactor: extract a helper that generates the base JSON, then apply a prefix strategy or return both variants.

- **State management sound:** The mapper correctly partitions top-level vs. subgraph tool IDs (`seen_top_tool_ids` and `seen_sub_tool_ids`), allowing the same tool_id to fire independently in parent and child agents without collision (test validates this at line 768–801).

- **Nested visibility implemented:** Nesting level and lineage path are computed correctly across legacy nested wrappers (double-nested events accumulate depth, test at line 846–874) and injected into every output frame via `or_insert`, preserving existing fields if already set. Regression test for the double-nested case confirms the fix.

- **Token accumulation correct:** The mapper accumulates prompt, completion, thinking, and cache-related tokens across all events, including wrapped variants, and emits them only in the final `finish` frame; conditional fields (thinking, cache tokens) are omitted if zero.

- **Test coverage strong:** Regression tests cover double-nested wrapping, level-0 behavior, agent-turn boundary forwarding, tool ID independence, and state cleanup on `Cancelled`. No gaps in visible test scenarios.

- **No error handling gaps:** JSON parsing (lines 262–263, 274–275, 508–509, 520–521) safely falls back to `Value::String` on malformed input; no panics at protocol boundaries.
