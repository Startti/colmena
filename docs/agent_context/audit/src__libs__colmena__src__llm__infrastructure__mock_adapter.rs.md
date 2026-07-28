# src/libs/colmena/src/llm/infrastructure/mock_adapter.rs

**Layer:** infrastructure  
**Purpose:** Provides a mock LLM adapter for testing and development, implementing the `LlmRepository` domain port without making real API calls.

## Symbols

- `MockAdapter` (struct, pub) — Mock implementation of the LlmRepository trait for test scenarios
- `MockAdapter::new()` (pub fn) — Constructor that returns a new MockAdapter instance
- `impl Default for MockAdapter` (impl block) — Derives Default via Self::new()
- `impl LlmRepository for MockAdapter` (impl block) — Implements the LlmRepository domain port
- `call()` (async fn) — Mock call handler that echoes back a formatted response based on the last message in the request
- `stream()` (async fn) — Mock stream handler that returns word-split chunks of a formatted mock response  [FLAG: unfinished — is_final hardcoded to false; comment notes "not handling is_final perfectly here"]
- `health_check()` (async fn) — Always returns Ok(), no actual health verification
- `provider_name()` (fn) — Returns the string literal "mock"

## File-level notes

- **Duplication**: Error message "No messages in request" appears twice (lines 24, 42); could be extracted to a constant.
- **Stream finality**: The `stream()` method generates mock chunks from a word-split response but never sets `is_final` to true on the final chunk. This is acknowledged in an inline comment but leaves the stream potentially incomplete from a consumer's perspective.
- **No call tracking**: Unlike more sophisticated mocks, there is no attempt to track or inspect calls for test assertions.
- **Minimal fixture**: Suitable for basic testing but limited for scenarios requiring realistic LLM behavior or error conditions.
