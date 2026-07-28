# src/libs/colmena/src/llm/domain/message_summarizer.rs

**Layer:** domain  
**Purpose:** Defines `MessageSummarizer` trait — a port for summarizing text blocks to a soft target length, enabling pluggable summarization implementations (cheap model in production, stubs in tests).

## Symbols

- `MessageSummarizer` (trait, pub) — Async trait defining a contract for text summarization; marked Send + Sync for thread-safe use across the application layer.
- `summarize` (method, pub) — Async method that accepts text and a soft target character count, returning a concise one-line summary or error.

## File-level notes

- **Spanish comments**: Usage guidance is in Spanish ("Resume un único bloque…"), consistent with codebase documentation conventions.
- **Minimal design**: Trait-only file; no implementation details. Real adapters belong in `infrastructure/` layer.
- **Error handling**: Uses `LlmError` for failures, propagating domain errors correctly.
- **Async trait**: Uses `#[async_trait]` macro, standard pattern for async methods in trait definitions.
- **No imports beyond scope**: Only depends on `LlmError` from the same module and `async_trait` crate.
