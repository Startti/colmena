# src/libs/colmena/src/llm/infrastructure/persistence/sqlite_conversation_repository.rs

**Layer:** infrastructure  
**Purpose:** Implements `ConversationRepository` trait with SQLite backend for persistent LLM conversation history storage, keyed by session/node identity and supporting message addition, retrieval, deletion, and summary metadata.

## Symbols

- `SqliteConversationRepository` (struct, pub) — Wrapper around `SqlitePool` implementing the conversation repository port
- `new` (fn, pub) — Constructor that wraps a `SqlitePool`
- `ConversationRepository` impl block — Trait implementation for SQLite-backed persistence
  - `get_by_id` (async fn, pub) — Queries and reconstructs `Conversation` from rows, branching on `agent_session_id` presence
  - `add_message` (async fn, pub) — Inserts a message row with UUID, session keys, role, content, tool_calls JSON, and timestamp
  - `delete` (async fn, pub) — Removes all messages for a conversation key
  - `get_with_summaries` (async fn, pub) — Queries messages with optional `summary` field, reconstructs to `StoredMessage` tuples
  - `set_summary` (async fn, pub) — Updates `summary` field for the nth message in conversation order via subquery
- `summary_tests` (mod, cfg(test)) — Test module for roundtrip verification
  - `pool` (async fn) — Creates in-memory SQLite table fixture
  - `key` (fn) — Constructs test `ConversationKey`
  - `set_and_get_summary_roundtrip_sqlite` (async test) — Verifies `add_message` + `set_summary` + `get_with_summaries` end-to-end

## File-level notes

- **Duplication of session-key branching** (improvement): `if let Some(agent) = &key.agent_session_id` block repeats across `get_by_id`, `get_with_summaries`, and `set_summary` with only SQL query text differing. Could extract to a helper that returns the binding tuple and lets caller decide the SQL string.

- **Repeated message parsing logic** (improvement): Lines 50–86 and 188–226 duplicate role string matching and message construction (role → LlmMessage factory, handling tool_calls). Extract to a helper `fn parse_row_to_message` that takes the row and returns `LlmMessage`.

- **Unwrap on fallible constructors** (improvement): `LlmMessage::system/user/assistant/assistant_with_tool_calls/tool` each call `.unwrap()` on potentially fallible operations (lines 68–83, 206–221). If a constructor fails (e.g. invalid message content), the repository panics instead of returning `Result<T, LlmError>`. Should propagate errors or use `.map_err()` to convert to `LlmError::RequestFailed`.

- **Silent role fallback** (improvement): Unrecognized role strings default to `MessageRole::User` (lines 64, 202) without logging or error. Data corruption in the database would silently re-categorize messages. Should either validate on storage or error on retrieval.

- **Unused variable** (dead_candidate): Line 57 binds `_created_at_str` but never uses it. Can be dropped or its extraction deferred until needed.

- **Missing test for agent_session_id branch**: `set_and_get_summary_roundtrip_sqlite` exercises the happy path but does not test deletion or the fallback `session_id`-only branch (lines 144–148, 252–266).
