use crate::llm::domain::{LlmProvider, LlmRepository};
use crate::llm::infrastructure::{OpenAiAdapter, GeminiAdapter, AnthropicAdapter};
use std::sync::Arc;

pub struct LlmProviderFactory;

impl LlmProviderFactory {
    pub fn create(provider: LlmProvider) -> Arc<dyn LlmRepository> {
        match provider {
            LlmProvider::OpenAi => Arc::new(OpenAiAdapter::new()),
            LlmProvider::Gemini => Arc::new(GeminiAdapter::new()),
            LlmProvider::Anthropic => Arc::new(AnthropicAdapter::new()),
        }
    }

    pub fn create_all() -> Vec<(LlmProvider, Arc<dyn LlmRepository>)> {
        vec![
            (LlmProvider::OpenAi, Self::create(LlmProvider::OpenAi)),
            (LlmProvider::Gemini, Self::create(LlmProvider::Gemini)),
            (LlmProvider::Anthropic, Self::create(LlmProvider::Anthropic)),
        ]
    }
}