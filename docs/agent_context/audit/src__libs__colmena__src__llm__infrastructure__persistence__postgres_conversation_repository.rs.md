# postgres_conversation_repository.rs

**Layer:** infrastructure  
**Purpose:** PostgreSQL adapter implementing the ConversationRepository port (domain trait). Provides persistence of LLM conversation messages with dual keying (session_id or agent_session_id), message round-trip serialization/deserialization, summaries, and CRUD operations on the `llm_node_history` table.

## Symbols

- `PostgresConversationRepository` (struct, pub) — Wrapper struct holding a PgPool, implements ConversationRepository for PostgreSQL persistence
- `PostgresConversationRepository::new` (fn, pub) — Constructor creating a repository from a PgPool
- `ConversationRepository::get_by_id` (async fn) — Fetches all messages for a conversation key, branching on agent_session_id vs session_id; deserializes role/content/tool_calls into LlmMessage structs; orders by created_at and id
- `ConversationRepository::add_message` (async fn) — Inserts a single LlmMessage into llm_node_history, extracting role/content/tool_call_id/tool_calls and setting created_at to now
- `ConversationRepository::delete` (async fn) — Deletes all messages for a conversation key (by agent_session_id/session_id + node_id)
- `ConversationRepository::get_with_summaries` (async fn) — Like get_by_id but also returns the summary field per StoredMessage wrapper
- `ConversationRepository::set_summary` (async fn) — Updates summary column for a specific message by ordinal (offset) within ordered conversation
- `summary_tests` (mod, #[cfg(test)]) — Integration test module for summary feature
- `key` (fn, private) — Helper constructing a ConversationKey with session_id, agent_session_id, and node_id for testing
- `set_and_get_summary_roundtrip` (async test fn, #[ignore]) — Integration test verifying round-trip of set_summary + get_with_summaries; requires live DATABASE_URL

## File-level notes

- **Code duplication (improvement)**: Role string-to-enum mapping logic (lines 59–65, 194–200) is duplicated nearly identically. Should be factored into a helper function.
- **Code duplication (improvement)**: Message construction from row (lines 67–84, 202–218) duplicates the same match statement with nearly identical logic. Extracting a helper function would reduce maintenance burden and improve clarity.
- **Query branching pattern (improvement)**: The agent_session_id vs session_id branching repeats 3 times (get_by_id, delete, get_with_summaries) as nearly identical if-let chains. Could be unified via a macro or helper to reduce boilerplate.
- **Unsafe unwrap() at persistence boundary (improvement)**: Lines 68, 69, 74, 76, 83 and 203–204, 209, 211, 217 use `.unwrap()` on LlmMessage constructors, which can panic if message construction fails. Boundary should propagate errors as `Result<T, LlmError>` instead of panicking, even if those constructors are unlikely to fail in practice (strings originate from the database).
- **Unused variable**: Line 57 `_created_at` is prefixed with underscore intentionally (common Rust pattern for values read but not used), no action needed.
- **Test isolation**: The integration test uses a fixed agent session ID string (`"pg_summary_test_001"`) and relies on `delete` for cleanup, but no parallel-test safeguard exists; safe for serial test runs.
