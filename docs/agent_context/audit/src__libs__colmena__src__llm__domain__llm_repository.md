# src/libs/colmena/src/llm/domain/llm_repository.rs

**Layer:** domain  **Purpose:** Defines the core LLM provider port trait and streaming type. This is the hexagonal-architecture boundary that allows the domain to depend on an abstraction rather than concrete provider implementations.

## Symbols

- `LlmStream` (type alias, pub) — Pinned, boxed async stream of LlmStreamChunk results for provider-agnostic streaming consumption across trait boundaries.
- `LlmRepository` (trait, pub) — Core domain port trait requiring Send + Sync; defines the contract that all LLM provider adapters must implement. Annotated with mockall::automock for testing.
- `call` (async method on LlmRepository, pub) — Makes a single synchronous call to the LLM, consuming an LlmRequest and returning LlmResponse or LlmError.
- `stream` (async method on LlmRepository, pub) — Initiates a streaming call to the LLM, returning an LlmStream of chunks or an LlmError.
- `health_check` (async method on LlmRepository, pub) — Tests provider connectivity and readiness; returns Result<(), LlmError>.
- `provider_name` (method on LlmRepository, pub) — Returns a static string identifier for the concrete provider implementation.

## File-level notes

- Clean, minimal, and focused. All symbols are essential components of the domain port contract.
- No dead code, stubs, or TODOs detected.
- Well-documented with brief doc comments on all trait methods.
- Mockall integration enables straightforward unit testing without real provider calls.
- Proper use of async_trait for trait methods with async/await support.
