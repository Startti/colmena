use crate::llm::domain::{LlmConfig, LlmError, LlmProvider, ProviderKind};
use std::env;

pub struct ConfigResolver;

impl ConfigResolver {
    /// Resolve API key from explicit value or environment variable
    pub fn resolve_api_key(
        provider_kind: &ProviderKind,
        explicit_key: Option<String>,
    ) -> Result<String, LlmError> {
        if let Some(key) = explicit_key {
            if key.trim().is_empty() {
                return Err(LlmError::InvalidApiKey);
            }
            return Ok(key.trim().to_string());
        }

        let env_var = provider_kind.env_var_name();
        env::var(env_var).map_err(|_| {
            LlmError::internal_error(format!(
                "API key not found in environment variable '{}' and no explicit key provided",
                env_var
            ))
        })
    }

    /// Create LlmConfig with resolved API key
    #[allow(clippy::too_many_arguments)]
    pub fn create_config(
        provider_kind: ProviderKind,
        api_key: Option<String>,
        model: Option<String>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        top_p: Option<f32>,
        frequency_penalty: Option<f32>,
        presence_penalty: Option<f32>,
    ) -> Result<LlmConfig, LlmError> {
        let resolved_api_key = Self::resolve_api_key(&provider_kind, api_key)?;

        let provider = LlmProvider::new(provider_kind, resolved_api_key, model)?;

        let mut config = LlmConfig::new(provider);

        if let Some(temp) = temperature {
            config = config.with_temperature(temp)?;
        }

        if let Some(tokens) = max_tokens {
            config = config.with_max_tokens(tokens)?;
        }

        if let Some(p) = top_p {
            config = config.with_top_p(p)?;
        }

        if let Some(penalty) = frequency_penalty {
            config = config.with_frequency_penalty(penalty)?;
        }

        if let Some(penalty) = presence_penalty {
            config = config.with_presence_penalty(penalty)?;
        }

        Ok(config)
    }

    /// Load environment variables from .env file if available
    pub fn load_env() -> Result<(), LlmError> {
        dotenvy::dotenv().ok(); // Ignore if .env doesn't exist
        Ok(())
    }
}
