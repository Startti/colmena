# src/libs/colmena/src/shared/infrastructure/mod.rs

**Layer:** shared (infrastructure)  **Purpose:** Module root providing a re-export facade for configuration resolution and service container dependency injection, bridging environment configuration and LLM use-case wiring for Python and TypeScript bindings.

## Symbols

### Direct Module Exports

- `config_resolver` (pub mod) — Re-exports configuration resolution utilities (ConfigResolver struct and methods)
- `service_container` (pub mod) — Re-exports service container factories for dependency injection

### Re-exported from config_resolver

- `ConfigResolver` (pub struct) — Utility for resolving API keys and building LlmConfig from environment or explicit values
  - `resolve_api_key(provider_kind, explicit_key) -> Result<String, LlmError>` — Resolves API key from explicit value or environment variable by provider kind
  - `create_config(provider_kind, api_key, model, temperature, max_tokens, top_p, frequency_penalty, presence_penalty) -> Result<LlmConfig, LlmError>` — Creates LlmConfig with resolved API key and optional inference parameters
  - `load_env() -> Result<(), LlmError>` — Loads environment variables from .env file using dotenvy (silently succeeds if file missing)

### Re-exported from service_container

- `ServiceContainer` (pub struct) — Holds three injectable LLM use cases (llm_call, llm_stream, llm_health_check)
  - `new(provider: ProviderKind) -> Self` — Creates a service container by instantiating provider and wiring use cases
  - `new_with_custom_repository(repository: Arc<dyn LlmRepository>) -> Self` — Creates a service container with a custom LlmRepository implementation

- `ServiceContainerFactory` (pub struct) — Factory for constructing pre-configured service containers
  - `create_all() -> Vec<(ProviderKind, ServiceContainer)>` — Creates service containers for OpenAI, Google, and Anthropic providers as a tuple vector
  - `create_for_provider(provider: ProviderKind) -> ServiceContainer` — Creates a service container for a specific provider

## File-level notes

- **Barrel pattern:** This is a thin re-export module using Rust's idiomatic `pub mod` + `pub use` to flatten the submodule hierarchy. No symbols defined at this level.
- **Used by:** `node_bindings::llm` and `python_bindings::mod`, which call `ConfigResolver::load_env()` at initialization and instantiate ServiceContainers via the factory.
- **No internal dependencies:** The main mod.rs file declares zero intra-crate imports, delegating all logic to submodules.
- **Service construction:** Both ConfigResolver and ServiceContainer follow a factory pattern. ConfigResolver is stateless utility methods; ServiceContainerFactory is a true factory that instantiates repositories and wires use cases.
- **API key resolution:** ConfigResolver enforces that explicit keys must be non-empty (trimmed); missing env vars are reported as LlmError::internal_error with a helpful message.
