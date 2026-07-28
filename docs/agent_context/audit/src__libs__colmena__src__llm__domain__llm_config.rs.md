# src/libs/colmena/src/llm/domain/llm_config.rs

**Layer:** domain  
**Purpose:** Defines LLM configuration and token usage value objects with validation. No infrastructure dependencies; pure domain models for model parameters, token counts, and provider bindings.

## Symbols

### Structs
- `LlmUsage` (struct, pub) — Value object tracking token consumption (prompt, completion, thinking, cache_read, cache_write, total)
- `LlmConfig` (struct, pub) — Configuration value object holding provider and model parameters (temperature, max_tokens, top_p, penalties, thinking_budget, volatile system suffix)

### LlmUsage impl
- `LlmUsage::new(prompt_tokens, completion_tokens) -> Self` — Constructor with prompt/completion tokens and derived total
- `LlmUsage::with_thinking_tokens(self, tokens) -> Self` — Builder adding reasoning tokens and updating total
- `LlmUsage::with_cache_read_tokens(self, tokens) -> Self` — Builder adding cache read token count
- `LlmUsage::with_cache_write_tokens(self, tokens) -> Self` — Builder adding cache write token count

### LlmConfig impl (builders)
- `LlmConfig::new(provider) -> Self` — Constructor with provider and all parameters defaulted to None
- `LlmConfig::with_temperature(self, f32) -> Result<Self, LlmError>` — Builder with bounds check 0.0-2.0
- `LlmConfig::with_max_tokens(self, u32) -> Result<Self, LlmError>` — Builder with validation (must be > 0)
- `LlmConfig::with_top_p(self, f32) -> Result<Self, LlmError>` — Builder with bounds check 0.0-1.0
- `LlmConfig::with_frequency_penalty(self, f32) -> Result<Self, LlmError>` — Builder with bounds check -2.0 to 2.0
- `LlmConfig::with_presence_penalty(self, f32) -> Result<Self, LlmError>` — Builder with bounds check -2.0 to 2.0
- `LlmConfig::with_thinking_budget(self, u32) -> Self` — Builder with no validation, returns Self directly  [FLAG: improvement — inconsistent with other builder methods which return Result]
- `LlmConfig::with_volatile_system_suffix(self, impl Into<String>) -> Self` — Builder normalizing empty/blank strings to None for cache-safe temporal context

### LlmConfig impl (getters)
- `LlmConfig::provider(&self) -> &LlmProvider` — Returns provider reference
- `LlmConfig::api_key(&self) -> &str` — Delegates to provider.api_key()
- `LlmConfig::model(&self) -> &str` — Delegates to provider.model()
- `LlmConfig::temperature(&self) -> Option<f32>` — Returns temperature option
- `LlmConfig::max_tokens(&self) -> Option<u32>` — Returns max_tokens option
- `LlmConfig::top_p(&self) -> Option<f32>` — Returns top_p option
- `LlmConfig::frequency_penalty(&self) -> Option<f32>` — Returns frequency_penalty option
- `LlmConfig::presence_penalty(&self) -> Option<f32>` — Returns presence_penalty option
- `LlmConfig::thinking_budget(&self) -> Option<u32>` — Returns thinking_budget option
- `LlmConfig::volatile_system_suffix(&self) -> Option<&str>` — Returns volatile_system_suffix as deref'd str option

### Tests
- `create_test_provider()` — Helper creating LlmProvider(Google, "test_api_key", Some("gemini-pro"))
- `test_config_creation_defaults` — Verifies all fields initialize to None/default
- `test_with_temperature_valid_and_invalid` — Temperature validation boundaries (1.5 OK, 2.5 error)
- `test_with_max_tokens_valid_and_invalid` — Max_tokens validation (1024 OK, 0 error)
- `test_with_top_p_invalid` — Top_p validation (1.5 out of bounds)
- `test_with_frequency_penalty_invalid` — Frequency_penalty validation (-2.5 out of bounds)
- `test_with_presence_penalty_invalid` — Presence_penalty validation (2.1 out of bounds)
- `test_builder_pattern_chaining` — Chaining multiple builder calls and verifying final state

## File-level notes

- **Validation consistency gap**: `with_temperature`, `with_max_tokens`, `with_top_p`, `with_frequency_penalty`, and `with_presence_penalty` all return `Result<Self, LlmError>` with explicit bounds validation. However, `with_thinking_budget` and `with_volatile_system_suffix` return `Self` directly with no error handling. Consider whether thinking_budget should have a bounds check (e.g., reject zero or enforce model-specific limits) for consistency and to catch configuration errors early.
- **Volatile system suffix normalization**: `with_volatile_system_suffix` correctly normalizes empty/blank strings to `None`, allowing downstream adapters to branch on `Option::is_some()` without string trimming. Good design, well-documented with reference to the cache-safety spec.
- **Provider delegation**: `api_key()` and `model()` delegate to `provider`, keeping the config object lightweight and enforcing that all provider-specific logic flows through the `LlmProvider` type.
- **Field documentation clarity**: Excellent inline comments explain provider-specific token semantics (Gemini thoughtsTokenCount vs. Anthropic cache_read_input_tokens vs. OpenAI reasoning_tokens/cached_tokens).
- **Test coverage**: 7 comprehensive tests validate all builder methods and default state. All validation error paths exercised.
- **No infrastructure coupling**: Pure domain layer; only uses serde for serialization, not any external system integration.
