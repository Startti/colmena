use crate::llm::domain::{ProviderKind, LlmRepository};
use crate::llm::infrastructure::{OpenAiAdapter, GeminiAdapter, AnthropicAdapter};
use std::sync::Arc;

pub struct LlmProviderFactory;

impl LlmProviderFactory {
    pub fn create(provider: ProviderKind) -> Arc<dyn LlmRepository> {
        match provider {
            ProviderKind::OpenAi => Arc::new(OpenAiAdapter::new()),
            ProviderKind::Gemini => Arc::new(GeminiAdapter::new()),
            ProviderKind::Anthropic => Arc::new(AnthropicAdapter::new()),
        }
    }

    pub fn create_all() -> Vec<(ProviderKind, Arc<dyn LlmRepository>)> {
        vec![
            (ProviderKind::OpenAi, Self::create(ProviderKind::OpenAi)),
            (ProviderKind::Gemini, Self::create(ProviderKind::Gemini)),
            (ProviderKind::Anthropic, Self::create(ProviderKind::Anthropic)),
        ]
    }
}