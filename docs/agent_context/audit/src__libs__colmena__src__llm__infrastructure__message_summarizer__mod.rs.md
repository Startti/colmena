# src/libs/colmena/src/llm/infrastructure/message_summarizer/mod.rs

**Layer:** infrastructure  
**Purpose:** Module barrel exporting `LlmMessageSummarizer`, a one-shot LLM-based conversation message summarizer adapter that calls a cheap provider directly, bypassing conversation history.

## Symbols

### mod.rs
- `pub mod llm_message_summarizer` (mod) — submodule containing the summarizer implementation
- `pub use llm_message_summarizer::LlmMessageSummarizer` (re-export) — publicly export the summarizer struct

### llm_message_summarizer.rs
- `LlmMessageSummarizer` (struct, pub) — adapter implementing `MessageSummarizer` trait; wraps LlmRepository, provider, api_key, model, and timeout
- `LlmMessageSummarizer::new()` (fn, pub) — constructor accepting repository, provider kind, API key, model name, and timeout duration
- `MessageSummarizer::summarize()` (async fn, trait impl) — one-shot summarization via LLM; sends system prompt (Spanish, single-line constraint) + user text; strips quotes/newlines; fails on empty response or timeout
- `mock_response()` (fn, #[cfg(test)]) — test helper that constructs a mock LlmResponse
- `summarize_returns_trimmed_one_line()` (test) — verifies trimming and single-line output

## File-level notes

- **Usage**: Called from `dag_engine::infrastructure::nodes::llm` to summarize conversation messages during agent execution.
- **Design**: Intentionally one-shot (no conversation history), bypasses `LlmCallUseCase` and `llm_node_history` — supports the conversation semantic summary feature via cheap-tier LLM calls.
- **Localization**: Spanish system prompt (all-caps instruction, target chars, actionable content only).
- **Error handling**: Timeout handled via `tokio::time::timeout`; empty responses rejected with structured error.
- **Test coverage**: Single integration test mocking the repository; exercise trimming, newline removal, and quote stripping.
