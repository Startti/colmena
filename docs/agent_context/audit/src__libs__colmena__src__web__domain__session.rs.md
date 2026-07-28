# src/libs/colmena/src/web/domain/session.rs

**Layer:** domain  
**Purpose:** Generic conversation-scoped session registry with TTL-based eviction and LRU capacity management, shared by three web-toolkit nodes (SocketIO, CRDT, Serve).

## Symbols

- `ConversationId` (type alias, pub) — convenience alias for `String` representing a conversation identifier
- `SessionKey` (struct, pub) — composite key combining `conversation_id` + `session_name` for session lookup
  - `conversation_id` (field, pub) — conversation identifier
  - `session_name` (field, pub) — session name within conversation
  - `new()` (impl, pub fn) — constructor accepting `Into<String>` for both fields
- `TtlConfig` (struct, pub) — configuration for TTL expiration and capacity limits
  - `idle_ttl_seconds` (field, pub) — idle timeout before eviction
  - `max_lifetime_seconds` (field, pub) — maximum lifetime regardless of activity
  - `max_active_sessions` (field, pub) — hard capacity limit for concurrent sessions
  - `impl Default` (impl) — provides sensible defaults (900s idle, 3600s lifetime, 50 sessions max)
- `SessionEntry<T>` (struct, pub) — wraps a session value with creation and activity timestamps
  - `value` (field, pub) — the session state
  - `created_at` (field, pub) — creation timestamp
  - `last_activity` (field, pub) — timestamp of last access
- `SessionRegistry<T>` (struct, pub) — main async-safe registry managing generic session state
  - `inner` (field, private) — `Arc<Mutex<HashMap<SessionKey, SessionEntry<T>>>>`
  - `ttl` (field, private) — configuration reference
  - `new()` (impl, pub fn) — creates registry wrapped in `Arc`, ready to share across tasks
  - `ttl()` (impl, pub fn) — accessor for configuration
  - `insert()` (impl, pub async fn) — insert or replace entry; returns previous value if any
  - `len()` (impl, pub async fn) — returns current entry count
  - `is_empty()` (impl, pub async fn) — checks if registry holds no sessions
  - `contains()` (impl, pub async fn) — checks presence by key
  - `remove()` (impl, pub async fn) — removes entry and returns its value
  - `with_entry()` (impl, pub async fn) — apply closure to entry if present; updates `last_activity` on access
  - `sweep_expired()` (impl, pub async fn) — evicts entries exceeding idle or lifetime TTL; invokes cleanup closure once per evicted value; returns count
  - `cleanup_conversation()` (impl, pub async fn) — removes all entries for a given `conversation_id`; invokes cleanup closure per evicted value; returns count
  - `insert_with_capacity()` (impl, pub async fn) — insert respecting `max_active_sessions`; evicts oldest-inactive (LRU by `last_activity`) entry if at capacity; skips eviction if replacing existing key
  - `start_sweeper()` (impl, pub fn) — spawns background `tokio` task that periodically invokes `sweep_expired`; returns handle for cancellation; requires generic `Send + 'static` and `Cleanup: Send + Clone + 'static`
- `tests` (module, cfg[test]) — 12 integration tests covering insert/remove, TTL expiration, LRU eviction, sweeper lifecycle, conversation cleanup, and closure invocation guarantees

## File-level notes

- **Zero infrastructure dependencies**: uses only `chrono`, `std::sync::Arc`, `tokio::sync::Mutex`, and `tokio::time`. Fits squarely in domain layer.
- **Comprehensive test coverage**: all public methods and edge cases (double-insert, LRU tie-breaking under non-deterministic HashMap iteration, capacity boundary conditions, same-key re-insert without eviction) tested.
- **Documentation**: well-commented on ownership semantics of `start_sweeper`, TTL logic, and closure lifecycle (sync under lock, may spawn async via closure).
- **No unfinished or dead code**: implementation is complete; all public methods appear used or part of stable API contract.
- **Non-determinism note**: `insert_with_capacity` LRU victim selection breaks HashMap iteration ties arbitrarily (documented on line 184); acceptable for session eviction (no ordering guarantee to callers).
