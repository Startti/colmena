# src/libs/colmena/src/llm/infrastructure/scripted_adapter.rs

**Layer:** infrastructure  **Purpose:** Test-only LLM adapter that replays a pre-recorded sequence of responses deterministically, driving engine code paths without burning API quota or tolerating model nondeterminism.

## Symbols

- `ScriptedResponse` (pub enum) — Discriminated response type: `Text(String)` for plain text, `ToolCall { id, tool_name, arguments }` for structured tool invocations with JSON-serialized args
- `ScriptedAdapter` (pub struct) — Holds a `Mutex<Vec<ScriptedResponse>>` queue, reversed for O(1) pop
- `ScriptedAdapter::new` (pub fn) — Constructor: takes vector of scripted responses, reverses for efficient popping, returns initialized adapter
- `ScriptedAdapter::remaining` (pub fn) — Returns count of unconsumed script entries (locks and checks queue length)
- `LlmRepository::call` (async fn) — Pops next response, logs via tracing, returns `LlmResponse` for Text or `Ok(response.with_tool_calls(...))` for ToolCall; errors if queue exhausted
- `LlmRepository::stream` (async fn) — Converts scripted entry to single `LlmStreamChunk` per response type and returns boxed stream; errors if queue exhausted
- `LlmRepository::health_check` (async fn) — Unconditional `Ok(())`
- `LlmRepository::provider_name` (fn) — Returns `"scripted"`
- `tests::make_request` (fn) — Helper that constructs an `LlmRequest` with Mock provider for test calls
- `tests::yields_text_response` (async test) — Verifies Text response yields correct content and no tool calls
- `tests::yields_tool_call_with_serialized_arguments` (async test) — Verifies ToolCall is serialized: JSON Value → JSON string in function.arguments
- `tests::yields_responses_in_script_order` (async test) — Verifies LIFO order (reversed queue) across mixed Text/ToolCall sequence
- `tests::errors_when_script_exhausted` (async test) — Verifies error message when queue is depleted
- `tests::remaining_decrements_per_call` (async test) — Verifies `remaining()` accurately tracks unconsumed entries
- `tests::stream_emits_text_chunk` (async test) — Verifies stream() yields single `LlmStreamPart::Content` chunk
- `tests::stream_emits_tool_call_chunk` (async test) — Verifies stream() yields single `LlmStreamPart::ToolCallChunk` with serialized arguments
- `tests::stream_errors_when_exhausted` (async test) — Verifies stream() errors on empty script

## File-level notes

- **Docstring contradiction (line 32–33)**: Claims "`stream()` is intentionally unsupported" but `stream()` is fully implemented (lines 108–154) and tested. Docstring is outdated and misleading. [FLAG: improvement — update docstring or clarify intent]

- **Silent serialization failure (line 95, 136)**: `serde_json::to_string(&arguments).unwrap_or_default()` masks JSON serialization errors by returning empty string. In test code, this can hide bugs in test setup (e.g., arguments containing non-serializable types). [FLAG: improvement — explicit error handling or test-time panic would catch misconfigured test cases]

- **Repeated mutex poisoning check (lines 52, 63, 112)**: Three identical `.lock().expect("scripted_adapter mutex poisoned")` chains. Factoring into a private helper method would improve readability and reduce duplication. [FLAG: improvement — extract `fn pop_next(&self) -> Result<ScriptedResponse, LlmError>`]

- **Test coverage**: Comprehensive; all code paths (Text, ToolCall, exhaustion, streaming) exercised. No dead or unreachable code.

- **No breaking changes**: Purely test-only adapter, no public API surface beyond `new()` and `remaining()`.
