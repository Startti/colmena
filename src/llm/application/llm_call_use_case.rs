use crate::llm::domain::{
    LlmConfig, LlmError, LlmMessage, LlmRepository, LlmRequest, LlmResponse, MessageRole,
};
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
        messages: Vec<String>,
        config: LlmConfig,
    ) -> Result<LlmResponse, LlmError> {
        // 1. Validate input
        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        // 2. Convert strings to LlmMessage objects
        let llm_messages: Result<Vec<LlmMessage>, LlmError> =
            messages.into_iter().map(LlmMessage::user).collect();

        // 3. Create request
        let request = LlmRequest::new(llm_messages?, config, false)?;

        // 4. Execute call
        self.repository.call(request).await
    }

    pub async fn execute_with_context(
        &self,
        system_message: Option<String>,
        messages: Vec<String>,
        config: LlmConfig,
    ) -> Result<LlmResponse, LlmError> {
        // 1. Validate input
        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        // 2. Build message list
        let mut llm_messages = Vec::new();
        if let Some(sys_msg) = system_message {
            llm_messages.push(LlmMessage::system(sys_msg)?);
        }
        for msg in messages {
            llm_messages.push(LlmMessage::user(msg)?);
        }

        // 3. Create request
        let request = LlmRequest::new(llm_messages, config, false)?;

        // 4. Execute call
        self.repository.call(request).await
    }

    pub async fn execute_conversation(
        &self,
        conversation: Vec<(MessageRole, String)>,
        config: LlmConfig,
    ) -> Result<LlmResponse, LlmError> {
        // 1. Validate input
        if conversation.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        // 2. Convert conversation to LlmMessage objects
        let llm_messages: Result<Vec<LlmMessage>, LlmError> = conversation
            .into_iter()
            .map(|(role, content)| LlmMessage::new(role, content))
            .collect();

        // 3. Create request
        let request = LlmRequest::new(llm_messages?, config, false)?;

        // 4. Execute call
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
        let result = use_case.execute(vec!["hello".to_string()], config).await;

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

        // 1. Setup mock expectation to return an error
        mock_repo.expect_call().times(1).returning(|_| {
            Err(LlmError::NetworkError {
                message: "Connection timed out".to_string(),
            })
        });

        // 2. Create use case and execute
        let use_case = LlmCallUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute(vec!["hello".to_string()], config).await;

        // 3. Assert error
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::NetworkError { message } => assert_eq!(message, "Connection timed out"),
            _ => panic!("Expected NetworkError"),
        }
    }
}
