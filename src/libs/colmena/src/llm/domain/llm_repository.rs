use crate::llm::domain::{LlmError, LlmRequest, LlmResponse, LlmStreamChunk};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

pub type LlmStream = Pin<Box<dyn Stream<Item = Result<LlmStreamChunk, LlmError>> + Send>>;

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait LlmRepository: Send + Sync {
    /// Make a synchronous call to the LLM
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;

    /// Make a streaming call to the LLM
    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError>;

    /// Test connection with the provider
    async fn health_check(&self) -> Result<(), LlmError>;

    /// Get the provider name this repository implements
    fn provider_name(&self) -> &'static str;
}
