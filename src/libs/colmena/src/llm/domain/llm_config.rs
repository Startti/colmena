use crate::llm::domain::{LlmError, LlmProvider};
use serde::{Deserialize, Serialize};

/// Emits `Some(n)` as `n` and `None` as `0`, so a field is never simply missing.
/// Deserialization is unaffected — `Option` still accepts an absent field, which
/// keeps histories written by older builds readable.
fn serialize_opt_u32_as_zero<S>(value: &Option<u32>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_u32(value.unwrap_or(0))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LlmUsage {
    /// Fresh input tokens — the portion of the prompt billed at the full input
    /// rate, with any cache-served tokens excluded.
    ///
    /// **Normalized across providers.** The three provider APIs disagree on
    /// whether cached tokens are part of the input count: Anthropic reports
    /// `input_tokens` already net of cache (disjoint), while OpenAI and Gemini
    /// report cached tokens as a *subset* of `prompt_tokens` /
    /// `promptTokenCount`. Each adapter converts to the disjoint form via
    /// [`LlmUsage::with_cached_input_tokens_included`], so this field means the
    /// same thing everywhere and `prompt + cache_read + cache_write` is always
    /// the true input size. Verified live 2026-08-23 against all three APIs.
    pub prompt_tokens: u32,
    /// Tokens in the text output (Gemini: text only; Anthropic/OpenAI: includes thinking).
    pub completion_tokens: u32,
    /// Thinking / reasoning tokens (Gemini `thoughtsTokenCount`, OpenAI `reasoning_tokens`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_tokens: Option<u32>,
    /// Tokens read from the prompt cache (Anthropic `cache_read_input_tokens`,
    /// OpenAI `cached_tokens`, Gemini `cachedContentTokenCount`). Billed at a
    /// steep discount — never fold this into [`Self::prompt_tokens`].
    ///
    /// Always serialized, as `0` when absent: an omitted field could not be told
    /// apart from a provider that never reports one, and those two cases have
    /// opposite cost implications.
    #[serde(serialize_with = "serialize_opt_u32_as_zero")]
    pub cache_read_tokens: Option<u32>,
    /// Tokens written to the prompt cache (Anthropic `cache_creation_input_tokens`,
    /// OpenAI `cache_write_tokens` on GPT-5.6 and later).
    ///
    /// Billed at a *premium* over fresh input — 1.25x on both providers — so it
    /// is kept separate from [`Self::cache_read_tokens`], which bills at 0.1x.
    /// The two rates differ by more than 12x, which is why they are never
    /// collapsed into one "cache" figure.
    ///
    /// Providers that cache automatically and charge nothing to create the entry
    /// (Gemini implicit caching, OpenAI before GPT-5.6) report no write at all,
    /// and this stays `0` for them. That is correct, not a missing datum.
    ///
    /// Always serialized, as `0` when absent.
    #[serde(serialize_with = "serialize_opt_u32_as_zero")]
    pub cache_write_tokens: Option<u32>,
    /// Every token the turn touched: prompt + completion + thinking +
    /// cache_read + cache_write. Cache tokens are counted here because they are
    /// real tokens the provider processed and billed; omitting them understated
    /// a cached Anthropic turn by ~80%.
    pub total_tokens: u32,
}

impl LlmUsage {
    pub fn new(prompt_tokens: u32, completion_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
            ..Default::default()
        }
    }

    pub fn with_thinking_tokens(mut self, tokens: u32) -> Self {
        self.thinking_tokens = Some(tokens);
        self.recompute_total();
        self
    }

    pub fn with_cache_read_tokens(mut self, tokens: u32) -> Self {
        self.cache_read_tokens = Some(tokens);
        self.recompute_total();
        self
    }

    pub fn with_cache_write_tokens(mut self, tokens: u32) -> Self {
        self.cache_write_tokens = Some(tokens);
        self.recompute_total();
        self
    }

    /// Record cache-read tokens that the provider counted *inside* its prompt
    /// total, subtracting them so [`Self::prompt_tokens`] is left holding only
    /// fresh input.
    ///
    /// Use this for OpenAI and Gemini. Anthropic already reports the two counts
    /// disjointly and must use [`Self::with_cache_read_tokens`] instead —
    /// calling this for Anthropic would subtract the cache twice.
    ///
    /// Saturating: a provider that ever reported more cached tokens than prompt
    /// tokens would floor `prompt_tokens` at 0 rather than wrap.
    pub fn with_cached_input_tokens_included(mut self, cached: u32) -> Self {
        self.prompt_tokens = self.prompt_tokens.saturating_sub(cached);
        self.cache_read_tokens = Some(cached);
        self.recompute_total();
        self
    }

    /// Record cache-write tokens that the provider counted *inside* its prompt
    /// total, subtracting them so [`Self::prompt_tokens`] is left holding only
    /// fresh input.
    ///
    /// The write-side twin of [`Self::with_cached_input_tokens_included`], for
    /// OpenAI GPT-5.6 and later: there the three categories *partition* the
    /// input, so `cached + written + uncached == prompt_tokens`. Anthropic
    /// reports its write disjointly and must use
    /// [`Self::with_cache_write_tokens`] instead — calling this for Anthropic
    /// would subtract tokens that were never in the prompt count.
    pub fn with_cache_write_tokens_included(mut self, written: u32) -> Self {
        self.prompt_tokens = self.prompt_tokens.saturating_sub(written);
        self.cache_write_tokens = Some(written);
        self.recompute_total();
        self
    }

    /// Recomputed from scratch on every mutation so builder call order cannot
    /// change the result.
    fn recompute_total(&mut self) {
        self.total_tokens = self.prompt_tokens
            + self.completion_tokens
            + self.thinking_tokens.unwrap_or(0)
            + self.cache_read_tokens.unwrap_or(0)
            + self.cache_write_tokens.unwrap_or(0);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    provider: LlmProvider,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    top_p: Option<f32>,
    frequency_penalty: Option<f32>,
    presence_penalty: Option<f32>,
    thinking_budget: Option<u32>,
    /// Cache-safe temporal context (2026-06-11): a block injected at the END
    /// of the system message, OUTSIDE the cacheable prefix. Regenerated every
    /// request (carries the current timestamp). Each adapter places it after
    /// the stable system content: Anthropic emits it as a 2nd system block
    /// without a `cache_control` marker; OpenAI/Gemini concatenate it after
    /// the stable system text. Keeping it out of the cached prefix lets the
    /// timestamp stay fresh per turn without busting prompt caching. See
    /// `docs/superpowers/specs/2026-06-11-temporal-block-cache-safe-design.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volatile_system_suffix: Option<String>,
}

impl LlmConfig {
    pub fn new(provider: LlmProvider) -> Self {
        Self {
            provider,
            temperature: None,
            max_tokens: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            thinking_budget: None,
            volatile_system_suffix: None,
        }
    }

    pub fn with_temperature(mut self, temperature: f32) -> Result<Self, LlmError> {
        if !(0.0..=2.0).contains(&temperature) {
            return Err(LlmError::InvalidTemperature);
        }
        self.temperature = Some(temperature);
        Ok(self)
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Result<Self, LlmError> {
        if max_tokens == 0 {
            return Err(LlmError::MaxTokensIsZero);
        }
        self.max_tokens = Some(max_tokens);
        Ok(self)
    }

    pub fn with_top_p(mut self, top_p: f32) -> Result<Self, LlmError> {
        if !(0.0..=1.0).contains(&top_p) {
            return Err(LlmError::InvalidTopP);
        }
        self.top_p = Some(top_p);
        Ok(self)
    }

    pub fn with_frequency_penalty(mut self, penalty: f32) -> Result<Self, LlmError> {
        if !(-2.0..=2.0).contains(&penalty) {
            return Err(LlmError::InvalidFrequencyPenalty);
        }
        self.frequency_penalty = Some(penalty);
        Ok(self)
    }

    pub fn with_presence_penalty(mut self, penalty: f32) -> Result<Self, LlmError> {
        if !(-2.0..=2.0).contains(&penalty) {
            return Err(LlmError::InvalidPresencePenalty);
        }
        self.presence_penalty = Some(penalty);
        Ok(self)
    }

    pub fn with_thinking_budget(mut self, thinking_budget: u32) -> Self {
        self.thinking_budget = Some(thinking_budget);
        self
    }

    /// Set the cache-safe volatile suffix (temporal block). See the field
    /// docs. Empty/blank strings are normalized to `None` so adapters can
    /// branch on `Option::is_some` without trimming.
    pub fn with_volatile_system_suffix(mut self, suffix: impl Into<String>) -> Self {
        let s = suffix.into();
        self.volatile_system_suffix = if s.trim().is_empty() { None } else { Some(s) };
        self
    }

    // Getters
    pub fn provider(&self) -> &LlmProvider {
        &self.provider
    }

    pub fn api_key(&self) -> &str {
        self.provider.api_key()
    }

    pub fn model(&self) -> &str {
        self.provider.model()
    }

    pub fn temperature(&self) -> Option<f32> {
        self.temperature
    }

    pub fn max_tokens(&self) -> Option<u32> {
        self.max_tokens
    }

    pub fn top_p(&self) -> Option<f32> {
        self.top_p
    }

    pub fn frequency_penalty(&self) -> Option<f32> {
        self.frequency_penalty
    }

    pub fn presence_penalty(&self) -> Option<f32> {
        self.presence_penalty
    }

    pub fn thinking_budget(&self) -> Option<u32> {
        self.thinking_budget
    }

    pub fn volatile_system_suffix(&self) -> Option<&str> {
        self.volatile_system_suffix.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::ProviderKind;

    #[test]
    fn usage_always_serializes_both_cache_fields() {
        // The wire contract ADP binds to: the cache line is always there, even
        // when the turn had no cache activity at all.
        let json = serde_json::to_value(LlmUsage::new(100, 10)).unwrap();
        assert_eq!(json["cache_read_tokens"], 0);
        assert_eq!(json["cache_write_tokens"], 0);
        // thinking_tokens keeps its `> 0` gate and stays absent.
        assert!(json.get("thinking_tokens").is_none());
        // No field was renamed.
        for key in ["prompt_tokens", "completion_tokens", "total_tokens"] {
            assert!(json.get(key).is_some(), "missing expected key `{key}`");
        }
    }

    #[test]
    fn usage_deserializes_when_cache_fields_are_absent() {
        // Histories written by older builds omit the cache fields entirely.
        // `Option` must still accept that rather than fail the whole record.
        let usage: LlmUsage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 100,
            "completion_tokens": 10,
            "total_tokens": 110
        }))
        .expect("absent cache fields must deserialize");
        assert_eq!(usage.cache_read_tokens, None);
        assert_eq!(usage.cache_write_tokens, None);
    }

    #[test]
    fn usage_roundtrips_through_json() {
        // LlmUsage crosses the subgraph boundary as JSON, so serialize ->
        // deserialize must not lose or alter a value.
        let original = LlmUsage::new(404, 8).with_cache_read_tokens(1809);
        let back: LlmUsage =
            serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap();
        assert_eq!(back.prompt_tokens, 404);
        assert_eq!(back.cache_read_tokens, Some(1809));
        assert_eq!(back.cache_write_tokens, Some(0), "0 round-trips as Some(0)");
        assert_eq!(back.total_tokens, original.total_tokens);
    }

    // Helper para crear un LlmProvider de prueba.
    fn create_test_provider() -> LlmProvider {
        LlmProvider::new(
            ProviderKind::Google,
            "test_api_key".to_string(),
            Some("gemini-pro".to_string()),
        )
        .unwrap()
    }

    #[test]
    fn test_config_creation_defaults() {
        let provider = create_test_provider();
        let config = LlmConfig::new(provider);

        assert_eq!(config.provider().kind(), &ProviderKind::Google);
        assert!(config.temperature().is_none());
        assert!(config.max_tokens().is_none());
        assert!(config.top_p().is_none());
        assert!(config.frequency_penalty().is_none());
        assert!(config.presence_penalty().is_none());
    }

    #[test]
    fn test_with_temperature_valid_and_invalid() {
        let provider = create_test_provider();
        let config = LlmConfig::new(provider);

        // Válido
        let config_with_temp = config.clone().with_temperature(1.5).unwrap();
        assert_eq!(config_with_temp.temperature(), Some(1.5));

        // Inválido
        let result = config.clone().with_temperature(2.5);
        assert_eq!(result.unwrap_err(), LlmError::InvalidTemperature);
    }

    #[test]
    fn test_with_max_tokens_valid_and_invalid() {
        let provider = create_test_provider();
        let config = LlmConfig::new(provider);

        // Válido
        let config_with_tokens = config.clone().with_max_tokens(1024).unwrap();
        assert_eq!(config_with_tokens.max_tokens(), Some(1024));

        // Inválido
        let result = config.clone().with_max_tokens(0);
        assert_eq!(result.unwrap_err(), LlmError::MaxTokensIsZero);
    }

    #[test]
    fn test_with_top_p_invalid() {
        let provider = create_test_provider();
        let config = LlmConfig::new(provider);
        let result = config.with_top_p(1.5);
        assert_eq!(result.unwrap_err(), LlmError::InvalidTopP);
    }

    #[test]
    fn test_with_frequency_penalty_invalid() {
        let provider = create_test_provider();
        let config = LlmConfig::new(provider);
        let result = config.with_frequency_penalty(-2.5);
        assert_eq!(result.unwrap_err(), LlmError::InvalidFrequencyPenalty);
    }

    #[test]
    fn test_with_presence_penalty_invalid() {
        let provider = create_test_provider();
        let config = LlmConfig::new(provider);
        let result = config.with_presence_penalty(2.1);
        assert_eq!(result.unwrap_err(), LlmError::InvalidPresencePenalty);
    }

    #[test]
    fn test_builder_pattern_chaining() {
        let provider = create_test_provider();
        let config = LlmConfig::new(provider)
            .with_temperature(0.8)
            .unwrap()
            .with_max_tokens(2048)
            .unwrap()
            .with_top_p(0.9)
            .unwrap()
            .with_frequency_penalty(-1.0)
            .unwrap()
            .with_presence_penalty(1.0)
            .unwrap();

        assert_eq!(config.temperature(), Some(0.8));
        assert_eq!(config.max_tokens(), Some(2048));
        assert_eq!(config.top_p(), Some(0.9));
        assert_eq!(config.frequency_penalty(), Some(-1.0));
        assert_eq!(config.presence_penalty(), Some(1.0));
    }
}
