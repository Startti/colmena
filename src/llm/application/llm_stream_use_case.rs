use crate::llm::domain::{
    LlmRepository, LlmRequest, LlmMessage, LlmError, MessageRole, LlmStream, ProviderKind,
};
use crate::shared::infrastructure::ConfigResolver;
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
        provider: ProviderKind,
        api_key: Option<String>,
        model: Option<String>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        top_p: Option<f32>,
        frequency_penalty: Option<f32>,
        presence_penalty: Option<f32>,
    ) -> Result<LlmStream, LlmError> {
        // 1. Validate input
        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        // 2. Convert strings to LlmMessage objects (assuming user messages for simplicity)
        let llm_messages: Result<Vec<LlmMessage>, LlmError> = messages
            .into_iter()
            .map(LlmMessage::user)
            .collect();

        let llm_messages = llm_messages?;

        // 3. Create configuration
        let config = ConfigResolver::create_config(
            provider,
            api_key,
            model,
            temperature,
            max_tokens,
            top_p,
            frequency_penalty,
            presence_penalty,
        )?;

        // 4. Create request (with streaming enabled)
        let request = LlmRequest::new(llm_messages, config, true)?;

        // 5. Execute streaming call
        self.repository.stream(request).await
    }

    pub async fn execute_with_context(
        &self,
        system_message: Option<String>,
        messages: Vec<String>,
        provider: ProviderKind,
        api_key: Option<String>,
        model: Option<String>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        top_p: Option<f32>,
        frequency_penalty: Option<f32>,
        presence_penalty: Option<f32>,
    ) -> Result<LlmStream, LlmError> {
        // 1. Validate input
        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        // 2. Build message list with optional system message
        let mut llm_messages = Vec::new();

        if let Some(sys_msg) = system_message {
            llm_messages.push(LlmMessage::system(sys_msg)?);
        }

        // Add user messages
        for msg in messages {
            llm_messages.push(LlmMessage::user(msg)?);
        }

        // 3. Create configuration
        let config = ConfigResolver::create_config(
            provider,
            api_key,
            model,
            temperature,
            max_tokens,
            top_p,
            frequency_penalty,
            presence_penalty,
        )?;

        // 4. Create request (with streaming enabled)
        let request = LlmRequest::new(llm_messages, config, true)?;

        // 5. Execute streaming call
        self.repository.stream(request).await
    }

    pub async fn execute_conversation(
        &self,
        conversation: Vec<(MessageRole, String)>,
        provider: ProviderKind,
        api_key: Option<String>,
        model: Option<String>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        top_p: Option<f32>,
        frequency_penalty: Option<f32>,
        presence_penalty: Option<f32>,
    ) -> Result<LlmStream, LlmError> {
        // 1. Validate input
        if conversation.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        // 2. Convert conversation to LlmMessage objects
        let llm_messages: Result<Vec<LlmMessage>, LlmError> = conversation
            .into_iter()
            .map(|(role, content)| LlmMessage::new(role, content))
            .collect();

        let llm_messages = llm_messages?;

        // 3. Create configuration
        let config = ConfigResolver::create_config(
            provider,
            api_key,
            model,
            temperature,
            max_tokens,
            top_p,
            frequency_penalty,
            presence_penalty,
        )?;

        // 4. Create request (with streaming enabled)
        let request = LlmRequest::new(llm_messages, config, true)?;

        // 5. Execute streaming call
        self.repository.stream(request).await
    }
}