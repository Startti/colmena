# src/libs/colmena/src/llm/infrastructure/persistence/in_memory_conversation_repository.rs

**Layer:** infrastructure  
**Purpose:** Provides an in-memory HashMap-based implementation of the `ConversationRepository` trait for testing and ephemeral conversation storage, keyed by (agent_session_id or session_id, node_id).

## Symbols

- `InMemoryConversationRepository` (struct, pub) — In-memory HashMap-backed repository for storing LLM conversation messages wrapped in Mutex for thread safety [FLAG: improvement — test name `agent_keying_isolates_two_runs_under_same_chat` is misleading; test actually verifies that agent_session_id keying *shares* history across runs, not isolates them]
- `InMemoryConversationRepository::new()` (fn, pub) — Creates a new empty InMemoryConversationRepository
- `InMemoryConversationRepository::lookup_key()` (fn, private) — Converts ConversationKey to HashMap tuple key (agent_session_id if present else session_id, node_id)
- `ConversationRepository::get_by_id()` (async fn) — Retrieves Conversation by key, returning empty message list if not found
- `ConversationRepository::add_message()` (async fn) — Appends LlmMessage to conversation for given key, creating entry if missing
- `ConversationRepository::delete()` (async fn) — Removes all messages for given conversation key
- `ConversationRepository::get_with_summaries()` (async fn) — Retrieves all StoredMessage entries (including summary field) for given key
- `ConversationRepository::set_summary()` (async fn) — Sets summary string on StoredMessage at specific ordinal; silently ignores if key or ordinal missing
- `tests::k()` (fn, private) — Test helper that constructs ConversationKey from agent_session_id, session_id, and node_id components
- `tests::agent_keying_isolates_two_runs_under_same_chat()` (async test) — Verifies that two runs with same agent_session_id but different session_ids share conversation history at same node_id
- `tests::legacy_keying_does_not_cross_runs()` (async test) — Verifies that two runs without agent_session_id (legacy keying) do not share conversation history across different session_ids
- `tests::node_id_isolates_two_llm_calls_in_same_run()` (async test) — Verifies that different node_ids within same run (same agent_session_id and session_id) maintain isolated conversation histories
- `tests::in_memory_summary_roundtrip()` (async test) — Verifies adding messages and setting/retrieving summaries on StoredMessage entries

## File-level notes

- All `.unwrap()` calls on `Mutex::lock()` (lines 32, 48, 59, 68, 79) assume lock is never poisoned; appropriate for in-memory test fixture but could use `expect()` with descriptive message for clarity
- `set_summary()` silently succeeds if key or ordinal missing (lines 79–83); behavior is acceptable for testing but not error-checked
- File is correctly placed in infrastructure persistence layer and implements all required ConversationRepository trait methods
