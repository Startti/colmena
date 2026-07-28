# src/libs/colmena/src/shared/infrastructure/service_container.rs

**Layer:** infrastructure  **Purpose:** Dependency injection container for LLM use cases across all supported providers (OpenAI, Google, Anthropic). Provides factory methods to instantiate containers with either auto-selected or custom LLM repositories.

## Symbols

- `ServiceContainer` (struct, pub) — DI container holding three LLM use cases (call, stream, health_check)
- `llm_call` (field, pub) — LlmCallUseCase for synchronous LLM calls
- `llm_stream` (field, pub) — LlmStreamUseCase for streaming LLM responses
- `llm_health_check` (field, pub) — LlmHealthCheckUseCase for provider health checks
- `ServiceContainer::new` (pub fn) — Creates container by instantiating LlmProviderFactory for the given provider and wiring three use cases with cloned repository references
- `ServiceContainer::new_with_custom_repository` (pub fn) — Creates container with caller-provided Arc<dyn LlmRepository>, wiring same three use cases with cloned references
- `ServiceContainerFactory` (struct, pub) — Empty factory struct for creating ServiceContainer instances
- `ServiceContainerFactory::create_all` (pub fn) — Returns Vec of tuples pairing all three ProviderKind variants (OpenAi, Google, Anthropic) with corresponding ServiceContainer instances
- `ServiceContainerFactory::create_for_provider` (pub fn) — Returns ServiceContainer for a specific ProviderKind, delegating directly to ServiceContainer::new()

## File-level notes

- No error handling or initialization failures exposed at the factory level; LlmProviderFactory::create() failures would panic (if any) but signature hides them
- ServiceContainerFactory::create_for_provider() is a pass-through wrapper to ServiceContainer::new() with no added value; API consistency may be the intent
- Repository cloning pattern consistent across both constructors: each use case receives an Arc clone for independent ownership
- No configuration or environment variables involved; factory is purely structural
