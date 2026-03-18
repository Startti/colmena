use crate::llm::domain::{LlmError, LlmProvider};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl LlmUsage {
    pub fn new(prompt_tokens: u32, completion_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::ProviderKind;

    // Helper para crear un LlmProvider de prueba.
    fn create_test_provider() -> LlmProvider {
        LlmProvider::new(
            ProviderKind::Gemini,
            "test_api_key".to_string(),
            Some("gemini-pro".to_string()),
        )
        .unwrap()
    }

    #[test]
    fn test_config_creation_defaults() {
        let provider = create_test_provider();
        let config = LlmConfig::new(provider);

        assert_eq!(config.provider().kind(), &ProviderKind::Gemini);
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
