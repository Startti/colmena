use crate::llm::domain::{
    LlmConfig, LlmError, LlmMessage, LlmRepository, LlmRequest, LlmStream, MessageRole,
};
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
        messages: Vec<String>,
        config: LlmConfig,
    ) -> Result<LlmStream, LlmError> {
        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        let llm_messages: Result<Vec<LlmMessage>, LlmError> =
            messages.into_iter().map(LlmMessage::user).collect();

        let request = LlmRequest::new(llm_messages?, config, true)?;

        self.repository.stream(request).await
    }

    pub async fn execute_with_context(
        &self,
        system_message: Option<String>,
        messages: Vec<String>,
        config: LlmConfig,
    ) -> Result<LlmStream, LlmError> {
        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        let mut llm_messages = Vec::new();
        if let Some(sys_msg) = system_message {
            llm_messages.push(LlmMessage::system(sys_msg)?);
        }
        for msg in messages {
            llm_messages.push(LlmMessage::user(msg)?);
        }

        let request = LlmRequest::new(llm_messages, config, true)?;

        self.repository.stream(request).await
    }

    pub async fn execute_conversation(
        &self,
        conversation: Vec<(MessageRole, String)>,
        config: LlmConfig,
    ) -> Result<LlmStream, LlmError> {
        if conversation.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        let llm_messages: Result<Vec<LlmMessage>, LlmError> = conversation
            .into_iter()
            .map(|(role, content)| LlmMessage::new(role, content))
            .collect();

        let request = LlmRequest::new(llm_messages?, config, true)?;

        self.repository.stream(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{LlmProvider, LlmStreamChunk, MockLlmRepository, ProviderKind};
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
        let chunk = LlmStreamChunk::new(Default::default(), "data".to_string(), provider, true);
        Box::pin(stream::iter(vec![Ok(chunk)]))
    }

    #[tokio::test]
    async fn test_execute_stream_success() {
        let mut mock_repo = MockLlmRepository::new();
        let config = create_test_config();

        mock_repo
            .expect_stream()
            .times(1)
            .returning(|_| Ok(create_mock_stream()));

        let use_case = LlmStreamUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute(vec!["hello".to_string()], config).await;

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

        mock_repo.expect_stream().times(1).returning(|_| {
            Err(LlmError::NetworkError {
                message: "Stream failed".to_string(),
            })
        });

        let use_case = LlmStreamUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute(vec!["hello".to_string()], config).await;

        assert!(matches!(result, Err(LlmError::NetworkError { .. })));
    }
}
