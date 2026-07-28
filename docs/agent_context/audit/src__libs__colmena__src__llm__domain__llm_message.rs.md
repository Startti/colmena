# src/libs/colmena/src/llm/domain/llm_message.rs

**Layer:** domain  **Purpose:** Core domain value objects representing LLM conversation messages, file attachments, and provider file references. No infrastructure dependencies; defines contracts for message handling across the LLM module.

## Symbols

- `MessageRole` (enum, pub) — Role discriminant: System, User, Assistant, Tool
- `impl Display for MessageRole` — Formats MessageRole as lowercase string via `as_str()`
- `impl FromStr for MessageRole` — Parses string to MessageRole (case-insensitive); returns `InvalidMessageRole` error
- `MessageRole::as_str()` (method, pub) — Returns static &'static str representation ("system", "user", "assistant", "tool")
- `FileData` (struct, pub) — Attachment metadata: document_id, mime_type, filename, size_hint, source, retained_inline_bytes
- `FileSource` (enum, pub) — Discriminated union: InlineBytes (Vec<u8>), SignedUrl (String), Uploaded (ProviderFileRef)
- `ProviderFileRef` (struct, pub) — Provider-side file reference: provider, provider_file_id, mime_type, filename, expires_at
- `FileData::inline()` (method, pub) — Constructor for in-memory file attachments
- `is_text_like()` (fn, pub) — Determines if MIME type is text-readable by LLMs (text/*, application/json, *+json); normalizes and strips parameters
- `LlmMessage` (struct, pub) — Conversation message: role, content, tool_call_id, tool_calls, files, timestamp (private fields)
- `LlmMessage::new()` (method, pub) — Base constructor with validation: non-Assistant roles require non-empty content; trims content; sets UTC timestamp
- `LlmMessage::system()` (method, pub) — Convenience constructor for system messages
- `LlmMessage::user()` (method, pub) — Convenience constructor for user messages
- `LlmMessage::user_with_files()` (method, pub) — Constructor for user messages with file attachments
- `LlmMessage::assistant()` (method, pub) — Convenience constructor for assistant messages
- `LlmMessage::assistant_with_tool_calls()` (method, pub) — Constructor for assistant messages with tool calls
- `LlmMessage::tool()` (method, pub) — Constructor for tool response messages with call_id
- `LlmMessage::role()` (method, pub) — Getter for message role
- `LlmMessage::content()` (method, pub) — Getter for message content
- `LlmMessage::tool_call_id()` (method, pub) — Getter for tool_call_id if present
- `LlmMessage::tool_calls()` (method, pub) — Getter for tool_calls slice if present
- `LlmMessage::files()` (method, pub) — Getter for files slice if present
- `LlmMessage::files_mut()` (method, pub) — Mutable getter for files Vec if present
- `LlmMessage::timestamp()` (method, pub) — Getter for message timestamp
- `LlmMessage::with_timestamp()` (method, pub) — Builder: set custom timestamp, consume and return Self
- `tests` (mod) — 12 unit tests covering message construction, validation, role parsing, MIME type classification, file source serialization

## File-level notes

- **Encapsulation**: All LlmMessage fields are private with public getters; validates content non-emptiness on construction (except Assistant, which can be empty per provider semantics)
- **Serialization**: Serde derives with `skip_serializing_if = "Option::is_none"` for optional fields; `timestamp` and `retained_inline_bytes` are never serialized (marked `#[serde(skip)]`)
- **Tests**: Comprehensive coverage of message creation, role parsing (case-insensitive), empty/whitespace validation, MIME classification (text/* and *json variants), and FileSource serde round-trips
- **Documentation**: Spanish comments throughout (document_id uniqueness requirement, retained_inline_bytes lifecycle, text-like rationale); English docstring for `is_text_like()` explaining provider Files API optimization
- **No flagged issues**: Code is clean, well-tested, follows good encapsulation patterns, no TODOs or unfinished stubs detected
