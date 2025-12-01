use crate::llm::domain::{LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream};
use async_trait::async_trait;

pub struct MockAdapter;

impl MockAdapter {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl LlmRepository for MockAdapter {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let last_message = request.messages().last().ok_or(LlmError::RequestFailed {
            message: "No messages in request".to_string(),
        })?;

        let response_content = format!("Mock response to: {}", last_message.content());

        LlmResponse::new(
            request.id().clone(),
            response_content,
            request.config().provider().clone(),
        )
    }

    async fn stream(&self, _request: LlmRequest) -> Result<LlmStream, LlmError> {
        Err(LlmError::RequestFailed {
            message: "Streaming not supported in MockAdapter yet".to_string(),
        })
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }

    fn provider_name(&self) -> &'static str {
        "mock"
    }
}
