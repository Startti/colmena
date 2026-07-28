# src/libs/colmena/src/llm/domain/value_objects/llm_response_id.rs

**Layer:** domain  **Purpose:** Value object wrapping a UUID-based response identifier string. Used as the `id` field in LLM response structures (LlmResponse, ChunkedResponse).

## Symbols

- `LlmResponseId` (struct, pub) — Value object containing a UUID-based string identifier, derives Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize
- `impl LlmResponseId` — Associated function implementations
  - `new()` (fn, pub) — Creates a new LlmResponseId by generating a UUID v4 and converting to string
  - `from_string()` (fn, pub) — Factory to construct from a String with validation (rejects empty strings); returns Result<Self, String>
  - `value()` (fn, pub) — Accessor returning a reference to the inner string value
- `impl Display for LlmResponseId` — Trait implementation formatting the response ID as its string value
- `impl Default for LlmResponseId` — Trait implementation delegating to Self::new()
- `tests` (mod, private) — Test module with 3 unit tests

## File-level notes

- **Pattern consistency**: `from_string()` returns `Result<Self, String>` instead of `Result<Self, LlmError>`. The domain layer defines `LlmError` enum (via thiserror) in `llm/domain/llm_error.rs` for structured error handling, but this value object uses untyped String errors. This is inconsistent with domain-layer patterns and reduces type safety. **Improvement candidate**: migrate to `Result<Self, LlmError>` or a dedicated value object error type.
- **Duplicate pattern**: `llm_response_id.rs` is structurally identical to `llm_request_id.rs` (same error handling, same constructor patterns). No code sharing.
- **Tests**: Basic coverage (creation, from_string success, from_string empty rejection) — no edge cases like UUID v4 validation or Display format verification.
- **Usage**: Used in `llm/domain/llm_response.rs` as the `id` field in `LlmResponse` and `ChunkedResponse` structures. Not exposed in Python/Node bindings (internal domain concept).
