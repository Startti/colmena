use crate::llm::domain::{LlmConfig, LlmError, LlmMessage, LlmRepository, LlmRequest, LlmResponse};
use std::sync::Arc;

pub struct LlmCallUseCase {
    repository: Arc<dyn LlmRepository>,
}

impl LlmCallUseCase {
    pub fn new(repository: Arc<dyn LlmRepository>) -> Self {
        Self { repository }
    }

    pub async fn execute(
        &self,
        messages: Vec<LlmMessage>,
        config: LlmConfig,
    ) -> Result<LlmResponse, LlmError> {
        // 1. Validate input
        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        // 2. Create request
        let request = LlmRequest::new(messages, config, false)?;

        // 3. Execute call
        self.repository.call(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{LlmProvider, MockLlmRepository, ProviderKind};
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

    #[tokio::test]
    async fn test_execute_success() {
        let mut mock_repo = MockLlmRepository::new();
        let config = create_test_config();
        let messages = vec![LlmMessage::user("hello".to_string()).unwrap()];

        // 1. Setup mock expectation
        mock_repo.expect_call().times(1).returning(|req| {
            LlmResponse::new(
                req.id().clone(),
                "response".into(),
                req.config().provider().clone(),
            )
        });

        // 2. Create use case and execute
        let use_case = LlmCallUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute(messages, config).await;

        // 3. Assert success
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content(), "response");
    }

    #[tokio::test]
    async fn test_execute_validation_error_empty_messages() {
        let mock_repo = MockLlmRepository::new(); // No expectations, should not be called
        let config = create_test_config();

        let use_case = LlmCallUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute(vec![], config).await; // Empty messages

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), LlmError::EmptyMessages);
    }

    #[tokio::test]
    async fn test_execute_repository_error() {
        let mut mock_repo = MockLlmRepository::new();
        let config = create_test_config();
        let messages = vec![LlmMessage::user("hello".to_string()).unwrap()];

        // 1. Setup mock expectation to return an error
        mock_repo.expect_call().times(1).returning(|_| {
            Err(LlmError::NetworkError {
                message: "Connection timed out".to_string(),
            })
        });

        // 2. Create use case and execute
        let use_case = LlmCallUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute(messages, config).await;

        // 3. Assert error
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::NetworkError { message } => assert_eq!(message, "Connection timed out"),
            _ => panic!("Expected NetworkError"),
        }
    }
}
