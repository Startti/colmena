use crate::llm::domain::{LlmConfig, LlmError, LlmMessage, LlmRepository, LlmRequest, LlmStream};
use std::sync::Arc;

pub struct LlmStreamUseCase {
    repository: Arc<dyn LlmRepository>,
}

impl LlmStreamUseCase {
    pub fn new(repository: Arc<dyn LlmRepository>) -> Self {
        Self { repository }
    }

    pub async fn execute(
        &self,
        messages: Vec<LlmMessage>,
        config: LlmConfig,
    ) -> Result<LlmStream, LlmError> {
        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        let request = LlmRequest::new(messages, config, true)?;

        self.repository.stream(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{
        LlmProvider, LlmStreamChunk, LlmStreamPart, MockLlmRepository, ProviderKind,
    };
    use futures::stream;
    use std::sync::Arc;

    fn create_test_config() -> LlmConfig {
        let provider = LlmProvider::new(
            ProviderKind::OpenAi,
            "test_key".into(),
            Some("gpt-4".into()),
        )
        .unwrap();
        LlmConfig::new(provider)
    }

    fn create_mock_stream() -> LlmStream {
        let provider = LlmProvider::new(ProviderKind::OpenAi, "test".into(), None).unwrap();
        let chunk = LlmStreamChunk::new(
            Default::default(),
            LlmStreamPart::Content("data".to_string()),
            provider,
            true,
        );
        Box::pin(stream::iter(vec![Ok(chunk)]))
    }

    #[tokio::test]
    async fn test_execute_stream_success() {
        let mut mock_repo = MockLlmRepository::new();
        let config = create_test_config();
        let messages = vec![LlmMessage::user("hello".to_string()).unwrap()];

        mock_repo
            .expect_stream()
            .times(1)
            .returning(|_| Ok(create_mock_stream()));

        let use_case = LlmStreamUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute(messages, config).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_stream_validation_error() {
        let mock_repo = MockLlmRepository::new();
        let config = create_test_config();

        let use_case = LlmStreamUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute(vec![], config).await;

        assert!(matches!(result, Err(LlmError::EmptyMessages)));
    }

    #[tokio::test]
    async fn test_execute_stream_repository_error() {
        let mut mock_repo = MockLlmRepository::new();
        let config = create_test_config();
        let messages = vec![LlmMessage::user("hello".to_string()).unwrap()];

        mock_repo.expect_stream().times(1).returning(|_| {
            Err(LlmError::NetworkError {
                message: "Stream failed".to_string(),
            })
        });

        let use_case = LlmStreamUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute(messages, config).await;

        assert!(matches!(result, Err(LlmError::NetworkError { .. })));
    }
}
