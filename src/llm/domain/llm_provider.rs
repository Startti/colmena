use crate::llm::domain::LlmError;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    OpenAi,
    Gemini,
    Anthropic,
    Mock,
}

impl Display for ProviderKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderKind::OpenAi => write!(f, "openai"),
            ProviderKind::Gemini => write!(f, "gemini"),
            ProviderKind::Anthropic => write!(f, "anthropic"),
            ProviderKind::Mock => write!(f, "mock"),
        }
    }
}

impl FromStr for ProviderKind {
    type Err = LlmError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(ProviderKind::OpenAi),
            "gemini" => Ok(ProviderKind::Gemini),
            "anthropic" => Ok(ProviderKind::Anthropic),
            _ => Err(LlmError::UnsupportedProvider {
                provider: s.to_string(),
            }),
        }
    }
}

impl ProviderKind {
    pub fn default_model(&self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "gpt-4o",
            ProviderKind::Gemini => "gemini-pro",
            ProviderKind::Anthropic => "claude-3-sonnet",
            ProviderKind::Mock => "mock-model",
        }
    }

    pub fn env_var_name(&self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "OPENAI_API_KEY",
            ProviderKind::Gemini => "GEMINI_API_KEY",
            ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
            ProviderKind::Mock => "MOCK_API_KEY",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmProvider {
    kind: ProviderKind,
    api_key: String,
    model: String,
}

impl LlmProvider {
    pub fn new(
        kind: ProviderKind,
        api_key: String,
        model: Option<String>,
    ) -> Result<Self, LlmError> {
        if api_key.trim().is_empty() {
            return Err(LlmError::InvalidApiKey);
        }

        let model = model.unwrap_or_else(|| kind.default_model().to_string());

        Ok(Self {
            kind,
            api_key: api_key.trim().to_string(),
            model,
        })
    }

    // Getters
    pub fn kind(&self) -> &ProviderKind {
        &self.kind
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation_success() {
        let provider = LlmProvider::new(
            ProviderKind::OpenAi,
            "test_key".to_string(),
            Some("gpt-4".to_string()),
        )
        .unwrap();

        assert_eq!(*provider.kind(), ProviderKind::OpenAi);
        assert_eq!(provider.api_key(), "test_key");
        assert_eq!(provider.model(), "gpt-4");
    }

    #[test]
    fn test_provider_creation_uses_default_model() {
        let provider =
            LlmProvider::new(ProviderKind::Gemini, "test_key".to_string(), None).unwrap();

        assert_eq!(*provider.kind(), ProviderKind::Gemini);
        assert_eq!(provider.model(), ProviderKind::Gemini.default_model());
    }

    #[test]
    fn test_provider_creation_trims_api_key() {
        let provider =
            LlmProvider::new(ProviderKind::Anthropic, "  spaced_key  ".to_string(), None).unwrap();
        assert_eq!(provider.api_key(), "spaced_key");
    }

    #[test]
    fn test_provider_creation_fails_on_empty_api_key() {
        let result = LlmProvider::new(ProviderKind::OpenAi, "".to_string(), None);
        assert!(matches!(result, Err(LlmError::InvalidApiKey)));

        let result_whitespace = LlmProvider::new(ProviderKind::OpenAi, "   ".to_string(), None);
        assert!(matches!(result_whitespace, Err(LlmError::InvalidApiKey)));
    }

    #[test]
    fn test_provider_kind_from_str() {
        // Casos exitosos (case-insensitive)
        assert_eq!(
            ProviderKind::from_str("openai").unwrap(),
            ProviderKind::OpenAi
        );
        assert_eq!(
            ProviderKind::from_str("Gemini").unwrap(),
            ProviderKind::Gemini
        );
        assert_eq!(
            ProviderKind::from_str("ANTHROPIC").unwrap(),
            ProviderKind::Anthropic
        );

        // Caso de error
        let result = ProviderKind::from_str("unknown_provider");
        assert!(result.is_err());
        if let Err(LlmError::UnsupportedProvider { provider }) = result {
            assert_eq!(provider, "unknown_provider");
        } else {
            panic!(
                "Expected an UnsupportedProvider error, but got {:?}",
                result
            );
        }
    }
}
