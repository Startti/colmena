# src/libs/colmena/src/llm/domain/llm_response.rs

**Layer:** domain  
**Purpose:** Defines core LLM response value objects (`LlmResponse`, `LlmStreamChunk`) and suspend signal handling (`SuspendInfo`), including streaming response parts and builder methods for rich response composition.

## Symbols

### SuspendInfo (struct, pub)
- `tool_call_id: String` — tool call ID that triggered the suspend  
- `questions: serde_json::Value` — pending questions array from suspend output  
- `raw_output: String` — raw JSON string from the tool output  

### LlmResponse (struct, pub)
- `id: LlmResponseId` — response ID (private)  
- `request_id: LlmRequestId` — linked request ID (private)  
- `message: LlmMessage` — response message content (private)  
- `usage: Option<LlmUsage>` — token usage data (private)  
- `provider: LlmProvider` — LLM provider and model info (private)  
- `timestamp: DateTime<Utc>` — response timestamp (private)  
- `finish_reason: Option<String>` — finish reason from model (private)  
- `tool_calls: Option<Vec<ToolCall>>` — tool calls requested by LLM (private)  
- `thinking_content: Option<String>` — reasoning/thinking blocks from model (private)  
- `suspend: Option<SuspendInfo>` — suspend signal if tool returned SUSPENDED (private)  

### LlmResponse methods (impl)
- `new(request_id, content, provider) -> Result<Self>` — constructs response with string content  
- `with_message(request_id, message, provider) -> Self` — constructs with explicit LlmMessage  
- `with_usage(self, usage) -> Self` — builder: attaches token usage  
- `with_finish_reason(self, reason) -> Self` — builder: sets finish reason  
- `with_timestamp(self, timestamp) -> Self` — builder: overrides timestamp  
- `with_tool_calls(self, tool_calls) -> Self` — builder: attaches tool calls and updates message  [FLAG: improvement — silent error handling with unwrap_or on assistant_with_tool_calls failure]
- `with_content(self, content) -> Self` — builder: updates message content, aware of existing tool_calls  [FLAG: improvement — silent error handling with unwrap_or on message construction]
- `id(&self) -> &LlmResponseId` — getter: response ID  
- `request_id(&self) -> &LlmRequestId` — getter: linked request ID  
- `content(&self) -> &str` — getter: delegated to message.content()  
- `message(&self) -> &LlmMessage` — getter: full message  
- `usage(&self) -> Option<&LlmUsage>` — getter: usage data  
- `provider(&self) -> &LlmProvider` — getter: provider  
- `model(&self) -> &str` — getter: delegated to provider.model()  
- `timestamp(&self) -> &DateTime<Utc>` — getter: response timestamp  
- `finish_reason(&self) -> Option<&str>` — getter: finish reason  
- `is_complete(&self) -> bool` — utility: returns true if finish_reason is set  
- `token_count(&self) -> Option<u32>` — utility: extracts total_tokens from usage  
- `tool_calls(&self) -> Option<&[ToolCall]>` — getter: tool calls slice  
- `has_tool_calls(&self) -> bool` — utility: checks if tool_calls is non-empty  
- `with_thinking_content(self, thinking) -> Self` — builder: attaches thinking/reasoning block (no-op if empty)  
- `thinking_content(&self) -> Option<&str>` — getter: thinking content  
- `suspended(tool_call_id, questions, raw_output) -> Self` — constructs minimal response signaling SUSPENDED event with sentinel-filled fields  
- `suspend(&self) -> Option<&SuspendInfo>` — getter: suspend signal  

### ToolCallChunk (struct, pub)
- `index: usize` — tool call index in stream  
- `id: String` — tool call ID  
- `name: String` — tool/function name  
- `args_chunk: String` — JSON arguments chunk  
- `provider_signature: Option<String>` — opaque provider-specific signature (e.g., Gemini thoughtSignature)  

### LlmStreamPart (enum, pub)
- `Content(String)` — text content chunk  
- `ToolCallChunk(ToolCallChunk)` — streaming tool call chunk  
- `Usage(LlmUsage)` — usage metrics  
- `LlmToolCallStart(ToolCall)` — tool call invocation started  
- `LlmToolCallFinish(ToolResult)` — tool call execution finished  
- `LlmMessageStart` — message stream start signal  
- `LlmMessageFinish(Option<LlmUsage>)` — message stream end with final usage  
- `ThinkingStart` — reasoning block started  
- `ThinkingContent(String)` — reasoning token delta  
- `ThinkingEnd` — reasoning block ended  

### LlmStreamChunk (struct, pub)
- `id: LlmResponseId` — chunk ID (private)  
- `request_id: LlmRequestId` — linked request ID (private)  
- `part: LlmStreamPart` — streaming part variant (private)  
- `provider: LlmProvider` — LLM provider (private)  
- `timestamp: DateTime<Utc>` — chunk timestamp (private)  
- `is_final: bool` — whether this is the final chunk (private)  
- `finish_reason: Option<String>` — finish reason if final (private)  

### LlmStreamChunk methods (impl)
- `new(request_id, part, provider, is_final) -> Self` — constructs stream chunk  
- `with_finish_reason(self, reason) -> Self` — builder: sets finish reason  
- `id(&self) -> &LlmResponseId` — getter: chunk ID  
- `request_id(&self) -> &LlmRequestId` — getter: linked request ID  
- `part(&self) -> &LlmStreamPart` — getter: streaming part  
- `content(&self) -> &str` — utility: extracts string if part is Content, else empty string  
- `provider(&self) -> &LlmProvider` — getter: provider  
- `model(&self) -> &str` — getter: delegated to provider.model()  
- `timestamp(&self) -> &DateTime<Utc>` — getter: chunk timestamp  
- `is_final(&self) -> bool` — getter: is final chunk flag  
- `finish_reason(&self) -> Option<&str>` — getter: finish reason  

### Test module
- `create_test_provider()` — test helper creating Google provider  
- `test_response_creation()` — verifies basic LlmResponse construction and field access  
- `test_response_builder_methods()` — verifies usage, finish_reason, token_count builders  
- `test_stream_chunk_creation()` — verifies LlmStreamChunk construction and field access  
- `suspend_info_set_and_retrieved()` — verifies LlmResponse::suspended() creates SuspendInfo correctly  
- `non_suspended_response_returns_none_for_suspend()` — verifies normal responses have None suspend  

## File-level notes

- **Silent error handling in builders**: `with_tool_calls()` (line 103–104) and `with_content()` (line 111–114) use `unwrap_or()` to silently fall back to the original message if LlmMessage construction fails. Callers cannot detect or diagnose message construction failures. Consider propagating or logging the error.

- **Suspend signal construction**: `suspended()` creates sentinel-filled response with Mock provider, "__suspended__" message/api_key, and freshly generated request_id/id. All fields except `suspend` are placeholders. This is documented but unconventional; callers must check `suspend()` before using other fields.

- **Builder pattern consistency**: Builders (`with_*`) consistently return `Self` for chaining. Exception: `new()` and `with_message()` differ in error handling (new returns Result, with_message returns Self).

- **Serde skip attributes**: `tool_calls`, `thinking_content`, and `suspend` fields skip serialization when None, keeping wire format clean.

- **Tests**: Complete coverage of construction, builders, and suspend behavior. All tests use test provider helper.
