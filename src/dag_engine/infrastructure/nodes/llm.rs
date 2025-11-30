use crate::domain::node::{ExecutableNode, NodeInputs};
use colmena::llm::domain::{LlmConfig, LlmMessage, LlmRequest, ThreadId, ProviderKind, LlmProvider};
use colmena::llm::infrastructure::{ConversationRepositoryFactory, LlmProviderFactory};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

pub struct LlmNode {
    repository_factory: Arc<ConversationRepositoryFactory>,
}

impl LlmNode {
    pub fn new(repository_factory: Arc<ConversationRepositoryFactory>) -> Self {
        Self { repository_factory }
    }

    fn resolve_env_var(value: &str) -> Result<String, String> {
        if value.starts_with("${") && value.ends_with("}") {
            let var_name = &value[2..value.len() - 1];
            std::env::var(var_name).map_err(|_| format!("Environment variable {} not found", var_name))
        } else {
            Ok(value.to_string())
        }
    }
}

#[async_trait]
impl ExecutableNode for LlmNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
    ) -> Result<Value, Box<dyn Error>> {
        // --- 1. Resolve Configuration (Inputs > Config) ---
        
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

        let api_key = Self::resolve_env_var(api_key_raw)?;

        // Model
        let model = inputs.get("model").and_then(|v| v.as_str())
            .or_else(|| config.get("model").and_then(|v| v.as_str()))
            .map(|s| s.to_string());

        // Prompt
        let prompt = inputs.get("prompt").and_then(|v| v.as_str())
            .or_else(|| config.get("prompt").and_then(|v| v.as_str()))
            .ok_or("Missing 'prompt' in inputs or config")?;

        // System Message (Optional)
        let system_message = inputs.get("system_message").and_then(|v| v.as_str())
            .or_else(|| config.get("system_message").and_then(|v| v.as_str()));

        // Thread ID (Optional - for Memory)
        let thread_id = inputs.get("thread_id").and_then(|v| v.as_str())
            .or_else(|| config.get("thread_id").and_then(|v| v.as_str()));

        // Connection URL (Optional - for Memory Backend)
        let connection_url_raw = inputs.get("connection_url").and_then(|v| v.as_str())
            .or_else(|| config.get("connection_url").and_then(|v| v.as_str()));

        // --- 2. Prepare LLM Request ---
        
        let provider = LlmProvider::new(provider_kind.clone(), api_key, model)?;
        let mut llm_config = LlmConfig::new(provider); // Add extra config params here if needed

        // Optional Params
        if let Some(temp) = inputs.get("temperature").and_then(|v| v.as_f64())
            .or_else(|| config.get("temperature").and_then(|v| v.as_f64())) {
            llm_config = llm_config.with_temperature(temp as f32)?;
        }
        
        if let Some(max_tokens) = inputs.get("max_tokens").and_then(|v| v.as_u64())
            .or_else(|| config.get("max_tokens").and_then(|v| v.as_u64())) {
            llm_config = llm_config.with_max_tokens(max_tokens as u32)?;
        }

        let mut messages = Vec::new();

        // 2.1 Load History if Thread ID and Connection URL are present
        let mut repo_instance = None;
        if let (Some(tid), Some(url_raw)) = (thread_id, connection_url_raw) {
            let connection_url = Self::resolve_env_var(url_raw)?;
            let repo = self.repository_factory.get_repository(&connection_url).await?;
            repo_instance = Some(repo.clone());
            
            let tid = ThreadId(tid.to_string());
            let conversation = repo.get_by_id(&tid).await?;
            messages.extend(conversation.messages);
        }

        // 2.2 Add System Message if present (and not already in history? For now just add it if provided)
        // Note: Usually system message is first. If history exists, maybe we shouldn't add it again?
        // Or maybe the history loading should handle this. For now, let's prepend if messages is empty.
        if let Some(sys_msg) = system_message {
             if messages.is_empty() {
                 messages.push(LlmMessage::system(sys_msg.to_string())?);
             }
        }

        // 2.3 Add User Prompt
        let user_message = LlmMessage::user(prompt.to_string())?;
        messages.push(user_message.clone());

        let request = LlmRequest::new(messages, llm_config, false)?;

        // --- 3. Execute LLM Call ---
        let llm_repo = LlmProviderFactory::create(provider_kind);
        let response = llm_repo.call(request).await?;

        // --- 4. Save to Memory (if enabled) ---
        if let (Some(tid_str), Some(repo)) = (thread_id, repo_instance) {
            let tid = ThreadId(tid_str.to_string());
            
            // Save User Message (we need to save it because it wasn't in DB yet)
            repo.add_message(&tid, user_message).await?;
            
            // Save Assistant Response
            let assistant_message = LlmMessage::assistant(response.content().to_string())?;
            repo.add_message(&tid, assistant_message).await?;
        }

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
