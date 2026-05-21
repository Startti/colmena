use crate::llm::domain::LlmError;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    OpenAi,
    Google,
    Anthropic,
    Mock,
    /// Synthetic provider used by `AttachmentRegistry` rows for outputs that
    /// originated from Colmena itself (image_generation, image_edit, tts).
    /// These rows store the `storage_key` in `provider_file_id` and are
    /// resolved lazily — the first time `load_attachment` is called from an
    /// actual chat provider (OpenAI/Anthropic/Google), the bytes are read
    /// via `OutputStorageRepository`, uploaded to that provider's Files API,
    /// and a sibling row is inserted with the real provider_file_id.
    Generated,
}

impl Display for ProviderKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderKind::OpenAi => write!(f, "openai"),
            ProviderKind::Google => write!(f, "google"),
            ProviderKind::Anthropic => write!(f, "anthropic"),
            ProviderKind::Mock => write!(f, "mock"),
            ProviderKind::Generated => write!(f, "generated"),
        }
    }
}

impl FromStr for ProviderKind {
    type Err = LlmError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(ProviderKind::OpenAi),
            "google" => Ok(ProviderKind::Google),
            "anthropic" => Ok(ProviderKind::Anthropic),
            "mock" => Ok(ProviderKind::Mock),
            "generated" => Ok(ProviderKind::Generated),
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
            // Model identifier stays as "gemini-*" — Gemini is Google's product name.
            ProviderKind::Google => "gemini-pro",
            ProviderKind::Anthropic => "claude-3-sonnet",
            ProviderKind::Mock => "mock-model",
            // `Generated` is never used to call an LLM — these constants are
            // sentinel placeholders to satisfy the enum's exhaustive matching.
            ProviderKind::Generated => "(generated artifact — no model)",
        }
    }

    pub fn env_var_name(&self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "OPENAI_API_KEY",
            // Env var stays as GEMINI_API_KEY — that is Google's official name for the key.
            ProviderKind::Google => "GEMINI_API_KEY",
            ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
            ProviderKind::Mock => "MOCK_API_KEY",
            ProviderKind::Generated => "GENERATED_API_KEY_UNUSED",
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
            LlmProvider::new(ProviderKind::Google, "test_key".to_string(), None).unwrap();

        assert_eq!(*provider.kind(), ProviderKind::Google);
        assert_eq!(provider.model(), ProviderKind::Google.default_model());
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
        assert_eq!(
            ProviderKind::from_str("openai").unwrap(),
            ProviderKind::OpenAi
        );
        assert_eq!(
            ProviderKind::from_str("Google").unwrap(),
            ProviderKind::Google
        );
        assert_eq!(
            ProviderKind::from_str("ANTHROPIC").unwrap(),
            ProviderKind::Anthropic
        );

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

    #[test]
    fn test_provider_kind_from_str_rejects_gemini() {
        // Clean-cut rename: "gemini" was renamed to "google" with no backward-compat alias.
        let result = ProviderKind::from_str("gemini");
        assert!(matches!(result, Err(LlmError::UnsupportedProvider { .. })));
    }

    #[test]
    fn test_provider_kind_display() {
        assert_eq!(ProviderKind::OpenAi.to_string(), "openai");
        assert_eq!(ProviderKind::Google.to_string(), "google");
        assert_eq!(ProviderKind::Anthropic.to_string(), "anthropic");
        assert_eq!(ProviderKind::Mock.to_string(), "mock");
    }

    #[test]
    fn test_provider_kind_env_var_name_preserves_gemini() {
        // The env var name intentionally stays as GEMINI_API_KEY — that is
        // the official name Google uses in its Gemini SDK / docs. Renaming
        // it would confuse users who already have it set.
        assert_eq!(ProviderKind::Google.env_var_name(), "GEMINI_API_KEY");
    }
}
