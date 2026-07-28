# src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs

**Layer:** infrastructure  **Purpose:** Adapter implementing the LlmRepository port for Google's Gemini API. Handles message/tool conversion, request building, both unary and streaming responses, with support for vision files, thinking models, and prompt caching.

## Symbols

### Public structs and impls
- `GeminiAdapter` (struct, pub) — HTTP client adapter for Gemini API; holds reqwest Client and base_url
- `GeminiAdapter::new()` (fn, pub) — Constructor with production Gemini endpoint
- `GeminiAdapter::with_base_url()` (fn, pub) — Builder overriding base_url for testing/alternative endpoints
- `GeminiAdapter::base_url()` (fn, pub) — Getter for configured endpoint; exposed for diagnostics
- `Default for GeminiAdapter` (impl) — Delegates to `new()`
- `LlmRepository for GeminiAdapter` (impl, async_trait) — Core port implementation with four methods:
  - `call()` — Unary LLM request; builds body, sends POST, parses response including tool calls, thinking content, and usage metadata
  - `stream()` — Streaming response using custom JsonStreamParser; yields Content/ToolCall/Thinking/Usage chunks with proper state management across chunks
  - `health_check()` — Endpoint connectivity check using dummy API key and hardcoded model
  - `provider_name()` — Returns "google" string identifier

### Private methods on GeminiAdapter
- `convert_messages()` (fn, priv) — Transforms LlmRequest messages to Gemini's Content format; handles System/User/Assistant/Tool roles; validates inline and uploaded files; wraps non-object tool responses in `{ "result": ... }`
- `convert_tools_to_gemini()` (fn, priv) — Maps ToolDefinition array to Gemini's functionDeclarations; omits empty `parameters` (Gemini silently fails if present)
- `build_request_body()` (fn, priv) — Assembles JSON request body: contents, systemInstruction (with cache-safe volatile suffix), tools, generationConfig (temperature, maxOutputTokens, topP, thinkingConfig)

### Private data structures (serde Serialize/Deserialize)
- `GeminiContent` — One turn of conversation; role + parts or text field (newer models)
- `GeminiPart` — Atomic content unit; text, function_call, function_response, inline_data (base64 blob), file_data (uploaded), thought (reasoning flag), thought_signature (opaque replay token for thinking models)
- `GeminiInlineData` — Embedded media: MIME type + base64-encoded bytes
- `GeminiFileData` — Uploaded media reference: MIME type + provider file URI
- `GeminiFunctionCall` — Tool call: name + args (JSON value)
- `GeminiResponse` — API response envelope: candidates array + optional usageMetadata
- `GeminiCandidate` — One completion: content + finish_reason
- `GeminiUsage` — Token counts: prompt, candidates, thoughts, cachedContent (from implicit prefix cache)

### Custom stream parser
- `JsonStreamParser<S>` (struct, generic over Stream) — Custom parser for Gemini's newline-delimited JSON objects; tracks buffer state, brace nesting, string quoting to extract complete JSON objects on-demand
- `JsonStreamParser::new()` (fn) — Constructor
- `Stream impl for JsonStreamParser` (async iterator) — Yields Vec<u8> containing one complete JSON object per chunk; handles flow control (Pending) and network errors

### Tests
- `build_request_body_serializes_uploaded_pdf_as_file_data()` — Verifies uploaded files serialize to fileData
- `build_request_body_returns_error_on_signed_url()` — Confirms SignedUrl (unresolved) is rejected pre-adapter
- `build_request_body_serializes_inline_pdf_as_inline_data()` — Verifies inline media encodes to base64 inlineData
- Scalar tool response wrapping tests (5 tests) — Validates non-object responses wrap in `{ "result": ... }`, objects pass through, non-JSON strings wrap
- Thought signature round-trip tests (3 tests) — Validates opaque `thoughtSignature` from thinking models is carried and replayed verbatim
- Cache read token tests (3 tests) — Validates implicit prefix cache counts (cachedContentTokenCount) populate LlmUsage::cache_read_tokens, zero counts omitted
- Base URL tests — Verifies production/override constructors
- Volatile suffix tests (2 tests) — Validates cache-safe temporal suffix appended to stable systemInstruction

## File-level notes

- **Gemini 2.5 thinking model support**: Full end-to-end with thought_signature round-trip (opaque per-call replay token required by thinking models). Thinking parts separated from content parts in stream (ThinkingStart/ThinkingContent/ThinkingEnd events).
- **Scalar tool response wrapping (2026-06-01 fix)**: Gemini's `functionResponse.response` requires JSON objects only. Adapter wraps non-objects in `{ "result": <value> }` and passes objects through unchanged. Non-JSON content (error strings) also wrapped as string value. Documented via design plan link.
- **Cache-safe temporal suffix (2026-06-11)**: Volatile suffix appended to systemInstruction's END to allow timestamp changes without breaking Gemini's implicit prefix cache (stable prefix remains unchanged).
- **Implicit prompt caching**: Gemini 2.5+ models automatically cache request prefixes (≥1024 tokens for 2.5-flash, ≥2048 for 2.5-pro). Cache hits surface `cachedContentTokenCount` in usageMetadata; adapter populates LlmUsage::cache_read_tokens for cost-tracking parity with OpenAI/Anthropic.
- **JsonStreamParser custom implementation**: Gemini streams newline-delimited JSON objects, not standard chunked encoding. Parser manually tracks brace nesting and string state to extract complete JSON per iteration (no external streaming JSON lib).
- **File handling**: InlineBytes (base64), Uploaded (provider URI), and SignedUrl (rejected — must be resolved before reaching adapter) are all handled in convert_messages.
- **Tool definitions**: Empty `parameters` are omitted to match Gemini's requirement (silent failure if present with zero properties).
- **Deprecated model usage**: health_check hardcodes gemini-1.5-flash (line 702), which is deprecated as of 2026; should use gemini-2.5-flash per project memory.

