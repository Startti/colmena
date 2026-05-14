//! Maps each supported provider to its cheap-tier model name used as
//! the default for attachment summary generation. Single function,
//! easy to audit and update.

use crate::llm::domain::ProviderKind;

/// Default cheap-tier model per provider. Centralised here so a single
/// edit updates the default when providers ship cheaper variants.
pub fn provider_cheap_tier(provider: &ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Google => "gemini-2.5-flash",
        ProviderKind::OpenAi => "gpt-4o-mini",
        ProviderKind::Anthropic => "claude-haiku-4-5-20251001",
        ProviderKind::Mock => "mock-model",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_cheap_tier_is_gemini_flash() {
        assert_eq!(provider_cheap_tier(&ProviderKind::Google), "gemini-2.5-flash");
    }

    #[test]
    fn openai_cheap_tier_is_gpt4o_mini() {
        assert_eq!(provider_cheap_tier(&ProviderKind::OpenAi), "gpt-4o-mini");
    }

    #[test]
    fn anthropic_cheap_tier_is_haiku() {
        assert_eq!(
            provider_cheap_tier(&ProviderKind::Anthropic),
            "claude-haiku-4-5-20251001"
        );
    }

    #[test]
    fn mock_cheap_tier_is_mock() {
        assert_eq!(provider_cheap_tier(&ProviderKind::Mock), "mock-model");
    }
}
