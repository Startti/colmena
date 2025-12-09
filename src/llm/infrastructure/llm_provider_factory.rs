use crate::llm::domain::{LlmRepository, ProviderKind};
use crate::llm::infrastructure::{AnthropicAdapter, GeminiAdapter, MockAdapter, OpenAiAdapter};
use std::sync::Arc;

pub struct LlmProviderFactory;

impl LlmProviderFactory {
    pub fn create(kind: ProviderKind) -> Arc<dyn LlmRepository> {
        match kind {
            ProviderKind::OpenAi => Arc::new(OpenAiAdapter::new()),
            ProviderKind::Gemini => Arc::new(GeminiAdapter::new()),
            ProviderKind::Anthropic => Arc::new(AnthropicAdapter::new()),
            ProviderKind::Mock => Arc::new(MockAdapter::new()),
        }
    }

    pub fn create_all() -> Vec<(ProviderKind, Arc<dyn LlmRepository>)> {
        vec![
            (ProviderKind::OpenAi, Self::create(ProviderKind::OpenAi)),
            (ProviderKind::Gemini, Self::create(ProviderKind::Gemini)),
            (
                ProviderKind::Anthropic,
                Self::create(ProviderKind::Anthropic),
            ),
        ]
    }
}
