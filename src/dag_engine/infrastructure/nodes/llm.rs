use crate::domain::node::{ExecutableNode, NodeInputs};
use colmena::llm::application::llm_call_use_case::LlmCallUseCase;
use colmena::llm::domain::{LlmConfig, LlmMessage, LlmProvider, ProviderKind, ConversationRepository, ThreadId};
use colmena::llm::infrastructure::LlmProviderFactory;
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

pub struct LlmNode {
    pub repository: Option<Arc<dyn ConversationRepository>>,
}

impl LlmNode {
    pub fn new(repository: Option<Arc<dyn ConversationRepository>>) -> Self {
        Self { repository }
    }
}

#[async_trait::async_trait]
impl ExecutableNode for LlmNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
    ) -> Result<Value, Box<dyn StdError>> {
        // 1. Resolve Configuration (Inputs > Config)
        
        // Provider
        let provider_str = inputs.get("provider").and_then(|v| v.as_str())
            .or_else(|| config.get("provider").and_then(|v| v.as_str()))
            .ok_or("Missing 'provider' in inputs or config")?;
            
        let provider_kind = match provider_str.to_lowercase().as_str() {
            "openai" => ProviderKind::OpenAi,
            "gemini" => ProviderKind::Gemini,
            "anthropic" => ProviderKind::Anthropic,
            "mock" => ProviderKind::Mock,
            _ => return Err(format!("Invalid provider '{}'. Supported: openai, gemini, anthropic, mock", provider_str).into()),
        };

        // API Key
        let api_key_raw = inputs.get("api_key").and_then(|v| v.as_str())
            .or_else(|| config.get("api_key").and_then(|v| v.as_str()))
            .ok_or("Missing 'api_key' in inputs or config")?;

        let api_key = if api_key_raw.starts_with("${") && api_key_raw.ends_with("}") {
            let var_name = &api_key_raw[2..api_key_raw.len() - 1];
            std::env::var(var_name).map_err(|_| format!("Environment variable {} not found", var_name))?
        } else {
            api_key_raw.to_string()
        };

        // Model
        let model = inputs.get("model").and_then(|v| v.as_str())
            .or_else(|| config.get("model").and_then(|v| v.as_str()))
            .map(|s| s.to_string());

        // Thread ID (for memory)
        let thread_id = inputs.get("thread_id").and_then(|v| v.as_str())
            .or_else(|| config.get("thread_id").and_then(|v| v.as_str()))
            .map(|s| ThreadId(s.to_string()));

        // 2. Create Provider and Config
        let provider = LlmProvider::new(provider_kind.clone(), api_key.to_string(), model)?;
        let mut llm_config = LlmConfig::new(provider);

        // Optional Params
        if let Some(temp) = inputs.get("temperature").and_then(|v| v.as_f64())
            .or_else(|| config.get("temperature").and_then(|v| v.as_f64())) {
            llm_config = llm_config.with_temperature(temp as f32)?;
        }
        
        if let Some(max_tokens) = inputs.get("max_tokens").and_then(|v| v.as_u64())
            .or_else(|| config.get("max_tokens").and_then(|v| v.as_u64())) {
            llm_config = llm_config.with_max_tokens(max_tokens as u32)?;
        }

        // 3. Construct Messages
        let mut messages = Vec::new();

        // System Message
        if let Some(system) = inputs.get("system_message").and_then(|v| v.as_str())
            .or_else(|| config.get("system_message").and_then(|v| v.as_str())) {
            messages.push(LlmMessage::system(system.to_string())?);
        }

        // Load History if thread_id exists and repository is available
        if let (Some(tid), Some(repo)) = (&thread_id, &self.repository) {
            match repo.get_by_id(tid).await {
                Ok(conversation) => {
                    // Append history messages (excluding system messages if we want to avoid duplication, 
                    // but usually system message is at the start. 
                    // Let's assume history contains previous user/assistant turns)
                    messages.extend(conversation.messages);
                },
                Err(_) => {
                    // Thread might not exist yet, which is fine.
                    // Or DB error. For now, we proceed with empty history or log error.
                }
            }
        }

        // User Prompt
        let prompt = inputs.get("prompt").and_then(|v| v.as_str())
            .or_else(|| config.get("prompt").and_then(|v| v.as_str()))
            .ok_or("Missing 'prompt' in inputs or config")?;
        
        let user_message = LlmMessage::user(prompt.to_string())?;
        messages.push(user_message.clone());

        // 4. Execute Use Case
        let repository = LlmProviderFactory::create(provider_kind);
        let use_case = LlmCallUseCase::new(repository);

        let response = use_case.execute(messages, llm_config).await?;

        // 5. Save to History
        if let (Some(tid), Some(repo)) = (&thread_id, &self.repository) {
            // Save User Message
            repo.add_message(tid, user_message).await?;
            
            // Save Assistant Response
            let assistant_message = LlmMessage::assistant(response.content().to_string())?;
            repo.add_message(tid, assistant_message).await?;
        }

        // 6. Return Output
        Ok(json!({
            "output": {
                "content": response.content(),
                "usage": response.usage()
            }
        }))
    }

    fn schema(&self) -> Value {
        json!({
            "type": "llm_call",
            "config": {
                "provider": "string (openai, gemini, anthropic)",
                "api_key": "string",
                "model": "string (optional)",
                "system_message": "string (optional)",
                "prompt": "string (optional)",
                "temperature": "number (optional)",
                "max_tokens": "integer (optional)",
                "thread_id": "string (optional, enables memory)"
            },
            "inputs": {
                "provider": "string (optional)",
                "api_key": "string (optional)",
                "model": "string (optional)",
                "system_message": "string (optional)",
                "prompt": "string (optional)",
                "temperature": "number (optional)",
                "max_tokens": "integer (optional)",
                "thread_id": "string (optional, enables memory)"
            },
            "outputs": {
                "content": "string",
                "usage": "object"
            }
        })
    }
}
