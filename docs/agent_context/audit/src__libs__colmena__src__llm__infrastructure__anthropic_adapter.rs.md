# src/libs/colmena/src/llm/infrastructure/anthropic_adapter.rs

**Layer:** infrastructure  
**Purpose:** Implements the Anthropic LLM provider adapter, handling message/request conversion, streaming SSE parsing, and prompt-caching optimization for the domain-level LlmRepository trait.

## Symbols

### Structs

- `AnthropicAdapter` (pub struct) — HTTP client wrapper and base URL config for Anthropic API calls
- `AnthropicMessage` (struct) — Serializable request message with role and content (text or blocks)
- `AnthropicContent` (enum) — Message content abstraction; serializes as either plain text string or structured block array
- `AnthropicContentBlock` (enum, #[serde(tag = "type")]) — Content block types: text, image, document, tool_use, tool_result
- `AnthropicMediaSource` (struct) — Media source metadata: source_type (base64/file/url), mime_type, data, file_id, or url
- `AnthropicResponse` (struct) — Deserialized non-streaming response with content blocks, usage counters, and stop_reason
- `AnthropicUsage` (struct) — Token counts including cache_read_input_tokens and cache_creation_input_tokens
- `AnthropicStreamEvent` (struct) — Single SSE event from Anthropic: event_type, index, content_block, delta, message, usage
- `AnthropicStreamMessage` (struct) — Message-level metadata in SSE stream start event
- `AnthropicStreamUsage` (struct) — Token usage in SSE events (input_tokens, output_tokens, cache counters)
- `AnthropicResponseBlock` (enum, #[serde(tag = "type")]) — Response block types: text, thinking, tool_use, other (catch-all for beta blocks)
- `AnthropicStreamBlock` (enum, #[serde(tag = "type")]) — Stream block types: text, thinking, tool_use, other
- `AnthropicStreamDelta` (struct) — Delta metadata in SSE: delta_type (text_delta/thinking_delta/input_json_delta), text, thinking, partial_json, stop_reason
- `SseEvent` (enum) — SSE wrapper; single Message variant carrying parsed data string
- `SseParser<S>` (struct) — Generic SSE parser; accumulates byte stream into buffer and yields complete double-newline-delimited messages

### Implementations

#### AnthropicAdapter impl Default
- `default() -> Self` — Delegates to `new()`

#### AnthropicAdapter impl (direct methods)
- `new() -> Self` — Constructor with production Anthropic URL (https://api.anthropic.com/v1)
- `with_base_url(base_url: String) -> Self` — Constructor with custom endpoint (for testing/proxies)
- `base_url(&self) -> &str` — Getter for configured base URL (exposed for tests and diagnostics)
- `convert_messages(&self, request: &LlmRequest) -> Result<(Option<String>, Vec<AnthropicMessage>), LlmError>` — Transforms LlmRequest messages to Anthropic format; extracts system message, handles user/assistant/tool roles, converts files to media sources (base64 inline, file_id uploaded, url for images), returns error for unresolved non-image SignedUrls
- `build_request_body(&self, request: &LlmRequest) -> Result<serde_json::Value, LlmError>` — Constructs full JSON request body: model, messages, stream flag, system/volatile_suffix caching blocks, temperature, max_tokens, top_p, thinking budget, and tools with cache_control marker on last tool; prompt caching enabled by default (5 min ephemeral breakpoints on system + last tool)

#### AnthropicAdapter impl LlmRepository (trait)
- `call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>` — Non-streaming call: POST to /messages, parse JSON response, extract text/thinking/tool_use/usage blocks, assemble LlmResponse with finish_reason
- `stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError>` — Streaming call: POST with stream=true, parse SSE events, yield LlmStreamChunk events for content/thinking/tool-calls/usage; handles zero-arg tools by synthesizing {} on content_block_stop when no input_json_delta was received
- `health_check(&self) -> Result<(), LlmError>` — Minimal connectivity test: POSTs a 1-token message without API key; returns Ok if endpoint responds (200 or 401), else error
- `provider_name(&self) -> &'static str` — Returns "anthropic"

#### SseParser<S> impl (direct)
- `new(stream: S) -> Self` — Constructor with empty buffer

#### SseParser<S> impl Stream
- `poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>>` — Implements Stream trait; loops polling byte stream, accumulates into buffer, yields complete messages (delimited by \n\n) after stripping "data: " prefix

## Symbols (cont'd): Tests

All test functions are private helpers or #[test] marked. Notable coverage:

- `new_uses_production_default()` — Verifies default URL
- `with_base_url_overrides()` — Verifies custom base URL
- `build_request_with_file()` — Helper: constructs LlmRequest with FileData
- `convert_messages_serializes_uploaded_pdf_as_file_id()` — PDF file upload → "file" source_type with file_id
- `convert_messages_returns_error_on_signed_url()` — Non-image SignedUrl → InternalError (requires Files API resolution)
- `convert_messages_serializes_signed_url_image_as_url_source()` — Image SignedUrl → "url" source_type, no file_id
- `convert_messages_returns_error_on_signed_url_for_non_image()` — Rejects non-image SignedUrl
- `convert_messages_serializes_inline_pdf_as_base64()` — Inline PDF bytes → base64-encoded data
- `anth_request_with_system_and_tools()` — Helper: constructs request with system message and tool definitions
- `cache_control_marker_on_system_message_block()` — System serializes as block array with cache_control: ephemeral marker
- `cache_control_marker_on_last_tool_only()` — Only the last tool in tools array carries cache_control marker
- `cache_control_works_without_tools()` — System is marked even when tools array is absent
- `anth_request_with_suffix()` — Helper: constructs request with optional volatile_system_suffix
- `volatile_suffix_emits_two_system_blocks_marker_on_first_only()` — Stable system + volatile suffix = two blocks; marker on first only
- `no_suffix_keeps_single_marked_system_block()` — Single stable system block carries marker

## File-level notes

- **Prompt caching (default ON, 2026-06-09):** System message and last tool definition are marked with `cache_control: {"type": "ephemeral"}` to enable 5-minute Anthropic request cache (~10% billing). Volatile temporal suffix (e.g., date/time block) is emitted as a second unmarked system block so it never busts the cache.
- **Zero-arg tools:** Anthropic may not emit input_json_delta for tools with no arguments (e.g., `get_amadeus_token()`). The adapter tracks which tool_use blocks received deltas; on content_block_stop, if a tool_use never received a delta, a synthesized `{}` is emitted so the caller's tool-call accumulator can parse arguments.
- **File handling:** Inline bytes → base64, uploaded files → file_id, signed URLs for images → url (Anthropic fetches server-side), signed URLs for non-images → error (must resolve via Files API first). Unsupported MIME types are logged with eprintln and silently dropped.
- **Extended thinking:** When `thinking_budget` is configured, temperature is removed (Anthropic requires temp=1 for extended thinking).
- **Error resilience:** Tool arguments that fail JSON parsing default to empty object `{}` (line 140); response parsing errors propagate (line 346). Minor inconsistency, though defensive fallback for replayed tool calls from previous messages is reasonable.
- **Health check:** Sends minimal message without API key; success (200) or auth failure (401) both indicate reachable endpoint.
- **SSE parser:** Accumulates bytes until double-newline delimiter, yields "data: ..." lines; tolerates pings and unknown event types.

