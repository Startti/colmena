# src/libs/colmena/src/dag_engine/domain/events.rs

**Layer:** domain  **Purpose:** Defines the `DagExecutionEvent` tagged enum representing all DAG execution milestones (node boundaries, LLM streaming, tool calls, reasoning, errors, subgraph nesting, batch progress, liveness heartbeats). Serialized to SSE for streaming front-end consumption.

## Symbols

- `DagExecutionEvent` (pub enum, 26 variants) — Tagged union of all execution event types, derived Serialize/Deserialize with serde tag="event" content="data"
  - `NodeStart` — node initialization with `node_id`, `node_type`, `inputs`, `config`
  - `NodeFinish` — node completion with `node_id` and `output`
  - `LlmToken` — incremental token from LLM stream
  - `LlmToolCall` — incremental chunk of a tool call being accumulated
  - `LlmUsage` — token accounting (prompt, completion, optional thinking/cache tokens)
  - `LlmToolCallStart` — tool execution phase beginning with `tool_args`
  - `LlmToolCallFinish` — tool execution completion with success flag and output
  - `LlmMessageStart` — LLM message generation beginning
  - `LlmMessageFinish` — LLM message generation end with optional usage
  - `ThinkingToken` — token from internal/hidden LLM reasoning (planner, critic, reactor, agent subgraphs)
  - `ReasoningStart` — provider reasoning block opened for `node_id` with block `id`
  - `ReasoningDelta` — incremental reasoning token for open reasoning block
  - `ReasoningEnd` — reasoning block closed
  - `GraphFinish` — entire DAG completed with `output`
  - `Error` — unrecoverable execution failure with message
  - `Cancelled` — intentional cancellation (distinct from error) with optional `reason` and `partial_output`
  - `TurnStart` — orchestrator loop turn beginning at turn number
  - `SubgraphNodeFinish` — subgraph-specific node completion (distinguished from regular NodeFinish)
  - `GraphUsageSummary` — per-node cost/audit summary with model and provider names
  - `SkillLoaded` — skill dynamically loaded (fires alongside tool_call_start/finish for frontend UI)
  - `ToolDescribed` — lazy tool schema revealed (fires alongside tool_call_start/finish for discovery UI)
  - `BatchProgress` — for_each batch progress snapshot (total, completed, ok, err, in_flight)
  - `BatchItemFinished` — for_each single item completion with index, key, status
  - `Progress` — liveness heartbeat emitted when in-flight node silent for configured interval; never resets idle watchdog
  - `SubgraphWrapped` — wraps child event with flat nesting (depth incremented, not re-nested; path is human-readable lineage)

- `default_subgraph_depth()` (fn) — Returns 1 as default depth for legacy serialized `SubgraphWrapped` events missing explicit depth field

- `impl DagExecutionEvent`
  - `node_id(&self)` (pub fn) — Extracts the `node_id` field when present; returns `None` for graph-level or boundary events (TurnStart, GraphFinish, GraphUsageSummary, Error, Cancelled, SubgraphWrapped)
  - `advances_heartbeat_clock(&self)` (pub fn) — Classifies whether event represents "real" progress (content, tokens, node boundaries, reasoning, tool calls) vs. bookkeeping (LlmUsage, LlmMessageStart, LlmMessageFinish, TurnStart, Progress itself); used to gate `last_forwarded` advancement; wrapped events follow their base event's classification

- `tests::tool_described_serializes_with_event_tag()` (#[test]) — Verifies ToolDescribed event serializes with correct tag and data structure
- `tests::batch_progress_serializes_with_event_tag()` (#[test]) — Verifies BatchProgress event serializes with correct tag and field values
- `tests::cancelled_serializes_with_event_tag()` (#[test]) — Verifies Cancelled event serializes with reason and partial_output
- `tests::cancelled_roundtrips()` (#[test]) — Verifies Cancelled event deserializes to identical struct after round-trip (both Some and None cases)
- `tests::heartbeat_clock_classification()` (#[test]) — Verifies content events advance heartbeat, bookkeeping events do not, and wrapped events follow base event logic
- `tests::subgraph_wrapped_carries_depth_and_path()` (#[test]) — Verifies SubgraphWrapped serializes and deserializes depth and path correctly
- `tests::subgraph_wrapped_defaults_depth_to_one_when_absent()` (#[test]) — Backward-compat test: legacy SubgraphWrapped without depth/path deserializes with depth=1, path=""
- `tests::progress_serializes_and_roundtrips()` (#[test]) — Verifies Progress heartbeat event round-trips with node_id and idle_secs

## File-level notes

- **No flags.** Clean domain layer with comprehensive test coverage (8 tests, 146 LOC). All variants are well-documented and used in the execution loop. No `todo!()`, `unimplemented!()`, or dead code detected. Enum is exhaustive with single-letter match arms catching non-nodeId events.
- Serde configuration uses `tag="event"` / `content="data"` for human-readable SSE JSON (`{ "event": "node_start", "data": { ... } }`).
- **Heartbeat logic:** `advances_heartbeat_clock()` distinguishes content (resets idle timeout) from bookkeeping (heartbeat must still fire). Wrapped events delegate to inner event type. `Progress` itself never advances the clock (prevents watchdog reset).
- **Backward compat:** `default_subgraph_depth()` and `#[serde(default)]` on `path` handle legacy serialized SubgraphWrapped events gracefully (old events without depth/path deserialize with depth=1, path="").
- **Nesting:** SubgraphWrapped uses flat wrapping (depth incremented, not boxed nested), with optional path field enabling human-readable lineage (`parent>…>node`) for front-end tree rendering.
