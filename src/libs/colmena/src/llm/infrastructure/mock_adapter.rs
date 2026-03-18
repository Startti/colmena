use crate::llm::domain::{
    LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmStreamPart,
};
use async_trait::async_trait;

pub struct MockAdapter;

impl MockAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MockAdapter {
    fn default() -> Self {
        Self::new()
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

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        use crate::llm::domain::LlmStreamChunk;
        use futures::stream;
        use futures::StreamExt;

        let last_message = request.messages().last().ok_or(LlmError::RequestFailed {
            message: "No messages in request".to_string(),
        })?;

        let full_text = format!("Mock stream response to: {}", last_message.content());
        let chunks: Vec<String> = full_text
            .split_whitespace()
            .map(|s| s.to_string() + " ")
            .collect();

        let request_id = request.id().clone();
        let provider = request.config().provider().clone();

        let stream = stream::iter(chunks).map(move |chunk_text| {
            Ok(LlmStreamChunk::new(
                request_id.clone(),
                LlmStreamPart::Content(chunk_text),
                provider.clone(),
                false, // Not handling is_final perfectly here, simplistic mock
            ))
        });

        Ok(Box::pin(stream))
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }

    fn provider_name(&self) -> &'static str {
        "mock"
    }
}
