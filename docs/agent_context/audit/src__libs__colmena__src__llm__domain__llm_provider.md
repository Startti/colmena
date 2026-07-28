# src/libs/colmena/src/llm/domain/llm_provider.rs

**Layer:** domain  
**Purpose:** Defines the LLM provider abstraction—the `ProviderKind` enum for supported providers and the `LlmProvider` value object that encapsulates provider configuration with utility methods for defaults and environment variable names.

## Symbols

- `ProviderKind` (enum, pub) — Enum representing supported LLM providers: OpenAi, Google, Anthropic, Mock, and Generated (for Colmena-synthesized artifacts); includes documentation explaining Generated's lazy upload semantics
- `Display for ProviderKind` (trait impl) — Converts ProviderKind to lowercase string representation ("openai", "google", "anthropic", "mock", "generated")
- `FromStr for ProviderKind` (trait impl) — Parses case-insensitive strings into ProviderKind; returns UnsupportedProvider error for unknown values
- `ProviderKind::default_model(&self) -> &'static str` (pub fn) — Returns the default model identifier for each provider ("gpt-4o", "gemini-pro", "claude-3-sonnet", "mock-model", or placeholder for Generated)
- `ProviderKind::env_var_name(&self) -> &'static str` (pub fn) — Returns the API key environment variable name for each provider; intentionally preserves GEMINI_API_KEY for Google (official name)
- `LlmProvider` (struct, pub) — Value object holding provider configuration: kind (ProviderKind), api_key (String), model (String); fields are private with public accessors
- `LlmProvider::new(kind, api_key, model) -> Result<Self, LlmError>` (pub fn) — Constructor that validates api_key is not empty and applies default model if none provided
- `LlmProvider::kind(&self) -> &ProviderKind` (pub fn) — Getter returning reference to provider kind
- `LlmProvider::api_key(&self) -> &str` (pub fn) — Getter returning reference to trimmed API key
- `LlmProvider::model(&self) -> &str` (pub fn) — Getter returning reference to model identifier
- `tests` (mod) — Test module with 8 tests covering provider creation, default model selection, API key trimming, validation, FromStr parsing, and env var naming

## File-level notes

- **Well-documented domain design:** Comments on ProviderKind::Generated explain lazy upload semantics for synthetic artifacts. Comments on env_var_name and default_model justify naming choices (GEMINI_API_KEY is official, gemini-* models are Google's product names).
- **Clean-cut migration visible in tests:** test_provider_kind_from_str_rejects_gemini() documents a deliberate backward-incompatible rename from "gemini" to "google" (no alias).
- **Zero infrastructure dependencies:** Uses only serde, std, and domain error type—proper domain layer isolation.
- **Comprehensive test coverage:** Validates creation success, default model fallback, whitespace trimming, empty key rejection, case-insensitive FromStr parsing, and Display serialization.
- **Immutable value object pattern:** Private fields with public getters; constructor validates preconditions and returns Result.
