use crate::domain::node::{ExecutableNode, NodeInputs};
use colmena::llm::application::llm_call_use_case::LlmCallUseCase;
use colmena::llm::domain::{LlmConfig, LlmMessage, LlmProvider, ProviderKind};
use colmena::llm::infrastructure::LlmProviderFactory;
use serde_json::{json, Value};
use std::error::Error as StdError;

pub struct LlmNode;

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
            _ => return Err(format!("Invalid provider '{}'. Supported: openai, gemini, anthropic", provider_str).into()),
        };

        // API Key
        let api_key = inputs.get("api_key").and_then(|v| v.as_str())
            .or_else(|| config.get("api_key").and_then(|v| v.as_str()))
            .ok_or("Missing 'api_key' in inputs or config")?;

        // Model
        let model = inputs.get("model").and_then(|v| v.as_str())
            .or_else(|| config.get("model").and_then(|v| v.as_str()))
            .map(|s| s.to_string());

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

        // User Prompt
        let prompt = inputs.get("prompt").and_then(|v| v.as_str())
            .or_else(|| config.get("prompt").and_then(|v| v.as_str()))
            .ok_or("Missing 'prompt' in inputs or config")?;
        
        messages.push(LlmMessage::user(prompt.to_string())?);

        // 4. Execute Use Case
        let repository = LlmProviderFactory::create(provider_kind);
        let use_case = LlmCallUseCase::new(repository);

        let response = use_case.execute(messages, llm_config).await?;

        // 5. Return Output
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
                "max_tokens": "integer (optional)"
            },
            "inputs": {
                "provider": "string (optional)",
                "api_key": "string (optional)",
                "model": "string (optional)",
                "system_message": "string (optional)",
                "prompt": "string (optional)",
                "temperature": "number (optional)",
                "max_tokens": "integer (optional)"
            },
            "outputs": {
                "content": "string",
                "usage": "object"
            }
        })
    }
}
