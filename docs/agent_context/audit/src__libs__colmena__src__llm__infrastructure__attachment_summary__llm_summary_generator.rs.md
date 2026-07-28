# src/libs/colmena/src/llm/infrastructure/attachment_summary/llm_summary_generator.rs

**Layer:** infrastructure  
**Purpose:** LLM-backed implementation of `AttachmentSummaryGenerator` that bypasses `LlmCallUseCase` to generate one-shot summaries without landing in conversation history; supports both extracted text and image inputs.

## Symbols

- `SYSTEM_PROMPT_TEXT` (const, private) — System prompt instructing LLM to catalog text documents by type, topic, and time period
- `SYSTEM_PROMPT_IMAGE` (const, private) — System prompt instructing LLM to catalog images by subject, type, and salient details
- `LlmAttachmentSummaryGenerator` (struct, pub) — Wrapper holding an `LlmRepository` to execute attachment summaries directly
- `LlmAttachmentSummaryGenerator::new` (fn, pub) — Constructor taking an `LlmRepository` arc and returning a new generator instance
- `AttachmentSummaryGenerator::generate` (async fn, pub via trait) — Dispatches on source type (text vs. image), builds LLM request, applies timeout, normalizes/truncates output
- `cfg` (fn, private, test-only) — Test helper constructing a `SummaryConfig` with mock provider and 200-char limit
- `mock_provider` (fn, private, test-only) — Test helper building a mock `LlmProvider`
- `mock_response` (fn, private, test-only) — Test helper constructing a mock `LlmResponse` from a string
- `SlowLlmRepository` (struct, private, test-only) — Custom `LlmRepository` impl that sleeps for a configured delay before returning
- `SlowLlmRepository::call` (async fn, private, test-only) — Sleeps for `delay` duration then returns a canned response
- `SlowLlmRepository::stream` (async fn, private, test-only) — Stub returning `unimplemented!()` (test-only)  [FLAG: unfinished — unimplemented! as marker in test code]
- `SlowLlmRepository::health_check` (async fn, private, test-only) — No-op returning `Ok(())`
- `SlowLlmRepository::provider_name` (fn, private, test-only) — Returns hardcoded `"slow-test"` string
- `tests::generates_summary_from_extracted_text` (async test fn) — Verifies summary generation from text with mock LLM
- `tests::empty_extracted_text_returns_skipped` (async test fn) — Verifies that empty/whitespace-only text returns `Skipped`
- `tests::whitespace_only_response_returns_empty_response_err` (async test fn) — Verifies that LLM returning only whitespace triggers `EmptyResponse` error
- `tests::truncates_oversized_response_to_max_output_chars` (async test fn) — Verifies response truncation to configured char limit
- `tests::per_call_timeout_returns_llm_call_failed` (async test fn) — Verifies that slow provider triggers timeout error
- `tests::collapses_newlines_in_response` (async test fn) — Verifies newline collapsing in output normalization

## File-level notes

- **Responsibility:** Pure infrastructure adapter—no domain logic, only `LlmRepository` delegation.
- **Message building:** Separate paths for text (filename + MIME + truncated content) and image (filename + inline bytes); both use `LlmMessage::user()` / `LlmMessage::user_with_files()` builders.
- **Timeout:** Per-call `tokio::time::timeout()` wrapping the repo.call() ensures slow providers don't starve other summaries; outer batch timeout (caller-managed) acts as hard ceiling.
- **Output normalization:** Trim → quote stripping → newline collapse → char-truncation; all steps preserve safety (empty check before return).
- **Test coverage:** 6 unit tests covering happy path (text + image), empty inputs, whitespace responses, truncation, timeout, and newline collapse. `SlowLlmRepository` is hand-rolled because `mockall`'s sync `returning` cannot await.
- **Error handling:** All builder errors bubble through `SummaryError::LlmCallFailed`; timeout handled separately; response content validated non-empty before return.
