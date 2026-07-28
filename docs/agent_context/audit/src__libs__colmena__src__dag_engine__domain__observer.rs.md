# src/libs/colmena/src/dag_engine/domain/observer.rs

**Layer:** domain  **Purpose:** Defines the event vocabulary (`NodeEvent` enum) and observation port (`ExecutionObserver` trait) for the DAG execution runtime. All DAG events (LLM tokens, tool calls, batch progress, reasoning blocks, subgraph propagation) flow through this interface.

## Symbols

- `NodeEvent` (enum, pub) — Exhaustive event type emitted by the DAG engine during execution; variants cover LLM streaming, tool invocation, usage stats, batch operations, thinking/reasoning blocks, and child subgraph events.
- `NodeEvent::LlmToken` — Streaming token from an LLM's text generation.
- `NodeEvent::LlmToolCall` — Streaming chunk of a tool invocation (incremental args).
- `NodeEvent::LlmUsage` — Token usage statistics (prompt, completion, thinking, cache metrics).
- `NodeEvent::LlmToolCallStart` — Marks the beginning of a tool execution.
- `NodeEvent::LlmToolCallFinish` — Marks the end of a tool execution with success/failure and output.
- `NodeEvent::SkillLoaded` — Fired when `load_skill` synthetic tool successfully loads a skill or reference; includes source and size metadata.
- `NodeEvent::ToolDescribed` — Fired when `describe_tool` synthetic tool reveals a tool's schema.
- `NodeEvent::BatchProgress` — Coarse progress snapshot from `for_each` node (total, completed, ok, err, in_flight).
- `NodeEvent::BatchItemFinished` — Per-item completion event from `for_each` with status (ok/err).
- `NodeEvent::LlmMessageStart` — Marks the start of an LLM message generation.
- `NodeEvent::LlmMessageFinish` — Marks the end of an LLM message with optional usage stats.
- `NodeEvent::ThinkingToken` — Streaming token from an internal "thinking" LLM call (planner, critic, reactor, or subgraph agent); distinct from `LlmToken` to separate thinking from final response.
- `NodeEvent::ReasoningStart` — Marks the opening of a provider reasoning block (extended thinking / thinking models).
- `NodeEvent::ReasoningDelta` — Incremental token within a reasoning block.
- `NodeEvent::ReasoningEnd` — Marks the closing of a reasoning block.
- `NodeEvent::SubgraphChildEvent` — Raw `DagExecutionEvent` from a child subgraph; child node IDs preserved for parent stream re-yield.
- `ExecutionObserver` (trait, pub) — Port definition for observation; implementers receive events during DAG execution; bounds are `Send + Sync` for thread-safe observer registration.
- `ExecutionObserver::on_event` (method, pub) — Callback invoked by the runtime for each emitted event.
- `tests` (mod, private) — Test harness with variant construction smoke tests.
- `tool_described_variant_constructible` (fn, private, test) — Verifies `ToolDescribed` variant can be constructed and pattern-matched.
- `skill_loaded_variant_constructible` (fn, private, test) — Verifies `SkillLoaded` variant can be constructed and pattern-matched.

## File-level notes

- No infrastructure dependencies; pure domain trait and value object.
- Well-documented with doc comments on event variants that need explanation (`SkillLoaded`, `ToolDescribed`, `ThinkingToken`, reasoning variants, `SubgraphChildEvent`).
- `SubgraphChildEvent` uses `serde_json::Value` for flexibility with heterogeneous child event types; intentional by design.
- `ExecutionObserver::on_event` uses `&self` (not `&mut self`), enabling concurrent/async observation without locking.
- Tests are minimal smoke tests (construction + pattern match) rather than exhaustive, but adequate for a port definition.
