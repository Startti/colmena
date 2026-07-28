# src/libs/colmena/src/llm/infrastructure/message_summarizer/llm_message_summarizer.rs

**Layer:** infrastructure  
**Purpose:** One-shot LLM message summarizer adapter using a cheap model; bypasses `LlmCallUseCase` and conversation history to isolate each summarization call.

## Symbols

- `LlmMessageSummarizer` (struct, pub) — Infrastructure adapter holding injected `LlmRepository`, provider kind, API key, model name, and timeout for one-shot summarization
- `LlmMessageSummarizer::new()` (fn, pub) — Constructor accepting repository, provider, credentials, model, and timeout; returns configured adapter instance
- `MessageSummarizer::summarize()` (async fn, pub via trait impl) — Accepts text and target character count; constructs Spanish system prompt, builds LLM request with timeout, cleans response (trim, dequote, collapse newlines), returns error if empty
- `mock_response()` (fn, private test helper) — Constructs a mock `LlmResponse` for unit tests with fixed request ID, provider, and text
- `summarize_returns_trimmed_one_line()` (async test fn) — Verifies `summarize()` trims whitespace, removes quotes, collapses newlines into single-line output

## File-level notes

- Module doc (lines 1–3) explicitly documents the design decision: one-shot, cheap-model call that **bypasses** `LlmCallUseCase` and history tracking—this is intentional, not a workaround.
- Spanish-language system prompt (lines 42–46) is hardcoded and Spanish-only by design; adapter is not localized.
- Line 54: `LlmRequest::new(..., false)` — third parameter is undocumented; context suggests `include_history: false` to enforce the no-history contract, but a comment would clarify intent.
- Error handling is defensive: timeout wraps with descriptive message; empty response returned as `RequestFailed` error.
- Test uses mockall's `expect_call()` and `returning()` correctly; single test covers the happy path and output-cleaning contract.
