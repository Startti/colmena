# src/libs/colmena/src/llm/domain/memory.rs

**Layer:** domain  
**Purpose:** Defines the conversation memory abstraction as a port trait (`ConversationRepository`) and value objects (`SessionId`, `AgentSessionId`, `ConversationKey`, `Conversation`, `StoredMessage`). Enables multi-layer separation between domain logic and persistence.

## Symbols

- `SessionId` (struct, pub) — Value object wrapping String; identifies the run scope of a single message.
- `AgentSessionId` (struct, pub) — Value object wrapping String; identifies the conversation a message belongs to (None means legacy single-run mode).
- `NodeIdPath` (struct, pub) — Value object wrapping String; path-qualified node identifier (e.g., "router" or "ventas/responder").
- `ConversationKey` (struct, pub) — Composite key identifying a single LLM thread (session_id, agent_session_id, node_id).
- `Conversation` (struct, pub) — Holds a ConversationKey and its Vec<LlmMessage>.
- `StoredMessage` (struct, pub) — Pairs an LlmMessage with an optional cached summary (None means not yet summarized or below threshold).
- `ConversationRepository` (trait, pub, async) — Port trait for conversation persistence; defines repository contract with Send + Sync bound.
- `ConversationRepository::get_by_id` (async method, pub) — Loads all messages for a thread, filtering by agent_session_id when Some, falling back to session_id + node_id.
- `ConversationRepository::add_message` (async method, pub) — Appends a single message to the thread; always writes session_id and node_id; agent_session_id written when present.
- `ConversationRepository::delete` (async method, pub) — Deletes all messages for the given thread using same filter as get_by_id.
- `ConversationRepository::get_with_summaries` (async method, pub, default impl) — Loads messages with their cached summaries; default implementation calls get_by_id and wraps each message in StoredMessage with summary=None (DB impls override).
- `ConversationRepository::set_summary` (async method, pub, default impl) — Persists the summary of a message at ordinal position (0-based in created_at order); default implementation is a no-op (DB impls override).

## File-level notes

- **Zero infrastructure dependencies.** Pure domain types and trait contract; all I/O is deferred to concrete impls.
- **Dual-session keying:** The ConversationKey supports both modern (agent_session_id) and legacy (session_id) modes. The trait documentation clearly explains fallback behavior.
- **Summary storage abstraction:** `get_with_summaries` and `set_summary` use default implementations that are sensible no-ops for simpler backends. Allows DB implementations to optimize (e.g., join summaries in a single query instead of default wrapping each message).
- **Async trait with async_trait macro:** Correctly bounded Send + Sync to support tokio runtime usage.
- **No unfinished code:** No todo!(), unimplemented!(), or stub implementations. Code is complete and correct.
- **No dead code:** All symbols are consumed by application and infrastructure layers (ConversationRepository is the public port; value objects are used throughout).
