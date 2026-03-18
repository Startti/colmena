use crate::llm::domain::{MessageRole, ProviderKind};
use crate::shared::infrastructure::{ConfigResolver, ServiceContainerFactory};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

// ==================== LLM Bindings ====================

#[napi(object)]
#[derive(Clone, Default)]
pub struct NodeLlmConfigOptions {
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f64>,
    pub frequency_penalty: Option<f64>,
    pub presence_penalty: Option<f64>,
}

#[napi(object)]
pub struct NodeLlmMessage {
    pub role: String,
    pub content: String,
}

#[napi]
pub struct ColmenaLlm {
    containers: HashMap<String, Arc<crate::shared::infrastructure::ServiceContainer>>,
}

#[napi]
impl ColmenaLlm {
    #[napi(constructor)]
    pub fn new() -> Result<Self> {
        ConfigResolver::load_env()
            .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
        let mut containers = HashMap::new();
        for (provider, container) in ServiceContainerFactory::create_all() {
            containers.insert(provider.to_string(), Arc::new(container));
        }
        Ok(Self { containers })
    }

    #[napi]
    pub async fn call(
        &self,
        messages: Vec<NodeLlmMessage>,
        provider: String,
        options: Option<NodeLlmConfigOptions>,
    ) -> Result<String> {
        let provider_kind = ProviderKind::from_str(&provider)
            .map_err(|e| Error::new(Status::InvalidArg, e.to_string()))?;
        let container = self
            .containers
            .get(&provider)
            .ok_or_else(|| {
                Error::new(
                    Status::InvalidArg,
                    format!("Provider {} not found", provider),
                )
            })?
            .clone();

        let llm_messages: Result<Vec<crate::llm::domain::LlmMessage>> = messages
            .into_iter()
            .map(|msg| {
                let role = MessageRole::from_str(&msg.role)
                    .map_err(|e| Error::new(Status::InvalidArg, e.to_string()))?;
                crate::llm::domain::LlmMessage::new(role, msg.content)
                    .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))
            })
            .collect();

        let options = options.unwrap_or_default();
        let config = ConfigResolver::create_config(
            provider_kind,
            options.api_key,
            options.model,
            options.temperature.map(|v| v as f32),
            options.max_tokens,
            options.top_p.map(|v| v as f32),
            options.frequency_penalty.map(|v| v as f32),
            options.presence_penalty.map(|v| v as f32),
        )
        .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;

        container
            .llm_call
            .execute(llm_messages?, config)
            .await
            .map(|res| res.content().to_string())
            .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))
    }

    #[napi]
    pub async fn health_check(&self, provider: String) -> Result<bool> {
        let container = self
            .containers
            .get(&provider)
            .ok_or_else(|| {
                Error::new(
                    Status::InvalidArg,
                    format!("Provider {} not found", provider),
                )
            })?
            .clone();

        container
            .llm_health_check
            .execute()
            .await
            .map(|status| status.is_healthy())
            .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))
    }

    #[napi]
    pub fn get_providers(&self) -> Result<Vec<String>> {
        Ok(self.containers.keys().cloned().collect())
    }
}

// ==================== DAG Engine Bindings ====================

#[napi]
pub async fn run_dag(
    file_path: String,
    resume_id: Option<String>,
    resume_answer: Option<String>,
    inject_payload: Option<Value>,
    include_extra_info: Option<bool>,
) -> Result<Value> {
    let result = crate::dag_engine::api::run_dag(
        file_path,
        resume_id,
        resume_answer,
        inject_payload,
        include_extra_info.unwrap_or(false),
    )
    .await
    .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;

    Ok(result)
}

#[napi]
pub async fn serve_dag(file_path: String, host: Option<String>, port: Option<u16>) -> Result<()> {
    let host = host.unwrap_or_else(|| "0.0.0.0".to_string());
    let port = port.unwrap_or(8080);

    crate::dag_engine::api::serve_dag(file_path, host, port)
        .await
        .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))
}
