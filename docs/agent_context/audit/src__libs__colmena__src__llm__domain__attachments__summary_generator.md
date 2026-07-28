# src/libs/colmena/src/llm/domain/attachments/summary_generator.rs

**Layer:** domain  **Purpose:** Defines the port (trait) and value objects for attachment summary generation, including configuration, input/output envelopes, and error types. Summaries run in parallel with the main llm_call as one-shot, history-less invocations.

## Symbols

- `SummaryInput` (struct, pub) — value object holding filename, mime_type, and source payload to summarize
- `SummarySource` (enum, pub) — discriminated union for text (ExtractedText) or binary (ImageBytes) summary inputs
- `SummarySource::ExtractedText` (variant, pub) — pre-extracted and char-truncated text from PDF, plain, markdown
- `SummarySource::ImageBytes` (variant, pub) — raw image bytes attached as vision input to the model
- `SummaryConfig` (struct, pub) — configuration for one summary call: provider, model, api_key, max_output_chars, timeout
- `SummaryOutcome` (enum, pub) — result envelope for summary generation: Generated(String), Skipped{reason}, Failed{reason}
- `SummaryOutcome::Generated` (variant, pub) — successful summary output text
- `SummaryOutcome::Skipped` (variant, pub) — summary skipped (expected outcome, persisted as null description)
- `SummaryOutcome::Failed` (variant, pub) — summary attempt failed (expected outcome, persisted as null description)
- `SummaryError` (enum, pub) — error type for unexpected infrastructure failures (network, malformed request)
- `SummaryError::LlmCallFailed` (variant, pub) — LLM call failed with details
- `SummaryError::EmptyResponse` (variant, pub) — model returned empty response
- `AttachmentSummaryGenerator` (trait, pub) — port trait for generating single-line summaries of attachments (mockable)
- `generate` (method, pub, async) — generates a summary for one attachment given SummaryInput and SummaryConfig

## File-level notes

- **Design intent:** Skipped/Failed outcomes are expected and flow through normal control (persisted as null), not raised as errors. Only infrastructure surprises (network, parse) become `SummaryError`.
- **Test coverage:** 3 unit tests verify SummaryInput/SummaryOutcome/SummaryError value objects.
- **Trait mockability:** `#[cfg_attr(test, mockall::automock)]` enables generated mocks for testing without live API calls.
- **No implementation:** This is a pure domain contract; infrastructure layer provides adapters (e.g., gemini_summary.rs, openai_summary.rs).
