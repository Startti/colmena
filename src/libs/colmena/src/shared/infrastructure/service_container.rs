use crate::llm::{
    application::{LlmCallUseCase, LlmHealthCheckUseCase, LlmStreamUseCase},
    domain::ProviderKind,
    infrastructure::LlmProviderFactory,
};
use std::sync::Arc;

pub struct ServiceContainer {
    // LLM Use Cases
    pub llm_call: LlmCallUseCase,
    pub llm_stream: LlmStreamUseCase,
    pub llm_health_check: LlmHealthCheckUseCase,
}

impl ServiceContainer {
    pub fn new(provider: ProviderKind) -> Self {
        let repository = LlmProviderFactory::create(provider);

        Self {
            llm_call: LlmCallUseCase::new(repository.clone()),
            llm_stream: LlmStreamUseCase::new(repository.clone()),
            llm_health_check: LlmHealthCheckUseCase::new(repository),
        }
    }

    pub fn new_with_custom_repository(
        repository: Arc<dyn crate::llm::domain::LlmRepository>,
    ) -> Self {
        Self {
            llm_call: LlmCallUseCase::new(repository.clone()),
            llm_stream: LlmStreamUseCase::new(repository.clone()),
            llm_health_check: LlmHealthCheckUseCase::new(repository),
        }
    }
}

// Factory for creating service containers for each provider
pub struct ServiceContainerFactory;

impl ServiceContainerFactory {
    pub fn create_all() -> Vec<(ProviderKind, ServiceContainer)> {
        vec![
            (
                ProviderKind::OpenAi,
                ServiceContainer::new(ProviderKind::OpenAi),
            ),
            (
                ProviderKind::Gemini,
                ServiceContainer::new(ProviderKind::Gemini),
            ),
            (
                ProviderKind::Anthropic,
                ServiceContainer::new(ProviderKind::Anthropic),
            ),
        ]
    }

    pub fn create_for_provider(provider: ProviderKind) -> ServiceContainer {
        ServiceContainer::new(provider)
    }
}
