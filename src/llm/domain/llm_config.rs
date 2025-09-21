use crate::llm::domain::{LlmError, LlmProvider};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        }
    }

    pub fn with_temperature(mut self, temperature: f32) -> Result<Self, LlmError> {
        if !(0.0..=2.0).contains(&temperature) {
            return Err(LlmError::configuration_error(
                "Temperature must be between 0.0 and 2.0",
            ));
        }
        self.temperature = Some(temperature);
        Ok(self)
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Result<Self, LlmError> {
        if max_tokens == 0 {
            return Err(LlmError::configuration_error(
                "Max tokens must be greater than 0",
            ));
        }
        self.max_tokens = Some(max_tokens);
        Ok(self)
    }

    pub fn with_top_p(mut self, top_p: f32) -> Result<Self, LlmError> {
        if !(0.0..=1.0).contains(&top_p) {
            return Err(LlmError::configuration_error(
                "Top_p must be between 0.0 and 1.0",
            ));
        }
        self.top_p = Some(top_p);
        Ok(self)
    }

    pub fn with_frequency_penalty(mut self, penalty: f32) -> Result<Self, LlmError> {
        if !(-2.0..=2.0).contains(&penalty) {
            return Err(LlmError::configuration_error(
                "Frequency penalty must be between -2.0 and 2.0",
            ));
        }
        self.frequency_penalty = Some(penalty);
        Ok(self)
    }

    pub fn with_presence_penalty(mut self, penalty: f32) -> Result<Self, LlmError> {
        if !(-2.0..=2.0).contains(&penalty) {
            return Err(LlmError::configuration_error(
                "Presence penalty must be between -2.0 and 2.0",
            ));
        }
        self.presence_penalty = Some(penalty);
        Ok(self)
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
