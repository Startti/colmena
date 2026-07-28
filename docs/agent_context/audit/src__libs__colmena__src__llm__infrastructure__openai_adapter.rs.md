# src/libs/colmena/src/llm/infrastructure/openai_adapter.rs

**Layer:** infrastructure  
**Purpose:** Adapter implementing the `LlmRepository` port for the OpenAI API (Chat Completions and Responses APIs), supporting both streaming and non-streaming requests with tool calling and multimodal file attachments.

## Symbols

### Structs & Enums
- `OpenAiAdapter` (struct, pub) — HTTP client + base_url holder for OpenAI API access
- `OpenAiResponse` (struct, private) — Deserialization target for chat completions non-streaming responses
- `OpenAiChoice` (struct, private) — Message + finish_reason from a choice in a response
- `OpenAiMessage` (struct, private) — Optional text content and tool calls from assistant response
- `OpenAiToolCall` (struct, private) — Single tool call entry (id, type, function) from assistant
- `OpenAiFunctionCall` (struct, private) — Function name and arguments from a tool call
- `OpenAiPromptDetails` (struct, private) — Cached token count from prompt usage details
- `OpenAiCompletionDetails` (struct, private) — Reasoning token count from completion usage details
- `OpenAiUsage` (struct, private) — Token counts including optional cache/reasoning details
- `OpenAiStreamChunk` (struct, private) — Deserialization target for streaming chunk (choices + optional usage)
- `OpenAiStreamChoice` (struct, private) — Delta and finish_reason for one streaming choice
- `OpenAiDelta` (struct, private) — Optional text content or tool_calls from a delta
- `OpenAiStreamToolCall` (struct, private) — Streaming tool call with index, id, type, function [FLAG: improvement — see file-level notes]
- `OpenAiStreamFunctionCall` (struct, private) — Optional name and arguments chunk for streaming tool
- `SseEvent` (enum, private) — Wrapper for SSE "data: " message content
- `SseParser<S>` (struct, private) — Buffer-based SSE parser for byte streams

### Trait Implementations
- `impl Default for OpenAiAdapter` — delegates to `new()`
- `impl OpenAiAdapter` — constructor & helper methods (below)
- `impl LlmRepository for OpenAiAdapter` — async call/stream/health_check/provider_name

### Methods (OpenAiAdapter)
- `new()` → Self (pub) — creates adapter with production OpenAI endpoint
- `with_base_url(base_url: String)` → Self (pub) — creates adapter with custom endpoint (for tests/proxies)
- `base_url(&self)` → &str (pub) — accessor for configured endpoint
- `build_messages(&self, request: &LlmRequest)` → Result<Vec<serde_json::Value>, LlmError> (private) — converts domain messages to OpenAI JSON format with volatile suffix handling
- `build_request_body(&self, request: &LlmRequest)` → Result<serde_json::Value, LlmError> (private) — builds complete chat completions request (model, messages, tools, temperature, reasoning_effort, etc.)
- `is_responses_api_required(&self, request: &LlmRequest)` → bool (private) — checks if any message has non-image files (triggers Responses API fallback)
- `call_chat_completions(&self, request: LlmRequest)` → Result<LlmResponse, LlmError> (private async) — makes non-streaming chat completions call and parses response
- `stream_chat_completions(&self, request: LlmRequest)` → Result<LlmStream, LlmError> (private async) — makes streaming chat completions call and returns boxed SSE stream
- `build_responses_request_body(&self, request: &LlmRequest)` → Result<serde_json::Value, LlmError> (private) — builds Responses API request with role-aware serialization (input_text vs output_text)
- `call_responses(&self, request: LlmRequest)` → Result<LlmResponse, LlmError> (private async) — makes non-streaming Responses API call
- `stream_responses(&self, request: LlmRequest)` → Result<Pin<Box<...>>, LlmError> (private async) — makes streaming Responses API call

### Trait Methods (LlmRepository)
- `call(&self, request: LlmRequest)` → Result<LlmResponse, LlmError> (async) — routes to chat_completions or responses based on file type
- `stream(&self, request: LlmRequest)` → Result<Pin<Box<dyn Stream<...>>>, LlmError> (async) — routes to streaming implementation
- `health_check(&self)` → Result<(), LlmError> (async) — checks endpoint health via `/models`
- `provider_name(&self)` → &'static str — returns "openai"

### Functions & Helpers
- `openai_usage_to_llm_usage(u: OpenAiUsage)` → LlmUsage (private) — converts OpenAI usage with cache/reasoning tokens to domain LlmUsage
- `impl<S> Stream for SseParser<S>` — implements async iteration over SSE events with line-by-line parsing and buffer management

### Tests
- `new_uses_production_default()` — verifies default endpoint
- `with_base_url_overrides()` — verifies custom endpoint
- `responses_serializes_uploaded_pdf_with_file_id()` — verifies Responses API PDF handling
- `responses_returns_error_on_signed_url()` — verifies rejection of unresolved SignedUrl for Responses
- `chat_completions_serializes_signed_url_image_as_url()` — verifies chat completions image_url handling
- `chat_completions_returns_error_on_uploaded_image()` — verifies rejection of file_id images in chat completions
- `responses_serializes_assistant_text_as_output_text()` — verifies role-aware serialization (2026-06-07 fix)
- `responses_serializes_assistant_tool_calls_as_function_call_entries()` — verifies tool calls as separate entries
- `responses_serializes_tool_response_as_function_call_output()` — verifies tool message → function_call_output
- `responses_serializes_full_load_attachment_sequence_correctly()` — regression test for E2E Phase 3.1 bug
- `chat_completions_appends_volatile_suffix_after_stable_system()` — verifies cache-safe temporal suffix placement
- `responses_appends_volatile_suffix_after_stable_system()` — verifies suffix handling in Responses API
- `no_suffix_leaves_system_unchanged()` — verifies no-suffix case

## File-level notes

1. **`OpenAiStreamToolCall::index` marked `#[allow(dead_code)]` but is actually used** (line 581): The struct field is dereferenced at lines 410, 415, and 421 in the streaming handler. The allow decorator appears to be incorrect or a leftover from an earlier version. [FLAG: improvement]

2. **Streaming tool call handling only processes first chunk per delta** (lines 406–434): When `choice.delta.tool_calls` is present, only `tool_calls.first()` is processed. If OpenAI sends multiple tool call chunks in a single delta event (which can happen in the streaming protocol), additional chunks are silently dropped. This may be intentional for single-tool scenarios, but the limitation is undocumented and could cause issues with concurrent tool calls. [FLAG: improvement]

3. **Volatile suffix logic duplicated** (lines 48–66 vs. 705–751): Both `build_messages()` and `build_responses_request_body()` implement identical volatile-suffix-appending logic. Should be extracted to a shared helper to reduce duplication and maintenance burden. [FLAG: improvement]

4. **Role-aware serialization for Responses API is correct** (lines 677–827): The 2026-06-07 fix properly converts System/User to `input_text`, Assistant to `output_text`, and Tool to `function_call_output`, addressing the E2E Phase 3.1 bug where all roles were hardcoded as `input_text`. Extensively tested.

5. **File attachment handling correctly splits by API**: Chat Completions (lines 76–113) only accepts images via signed URL or inline bytes; Responses API (lines 757–787) accepts images + PDFs via uploaded file_id. Both reject invalid combinations with clear InternalError messages.

6. **SSE parser is buffer-based and sound**: The `SseParser` (lines 614–675) accumulates bytes, searches for `\n\n` delimiters, and extracts `data: ` lines without regex. No obvious parsing bugs or edge-case gaps noted.

7. **All 13 tests pass**: Coverage includes default/custom URLs, role serialization, file handling (signed URL, inline, uploaded), cache-safe temporal suffixes, and the critical regression test for the load_attachment sequence. Tests are well-scoped and deterministic.
