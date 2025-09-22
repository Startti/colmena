# 🔌 Añadir Nuevos Proveedores

### 1. Definir Proveedor en el Dominio

```rust
// src/llm/domain/llm_provider.rs
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LlmProvider {
    OpenAi,
    Gemini,
    Anthropic,
    Cohere,        // ← Nuevo proveedor
    Huggingface,   // ← Otro nuevo proveedor
}

impl LlmProvider {
    pub fn from_str(s: &str) -> Result<Self, LlmError> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(Self::OpenAi),
            "gemini" => Ok(Self::Gemini),
            "anthropic" => Ok(Self::Anthropic),
            "cohere" => Ok(Self::Cohere),        // ← Añadir aquí
            "huggingface" => Ok(Self::Huggingface), // ← Y aquí
            _ => Err(LlmError::invalid_provider(s)),
        }
    }
}
```

### 2. Crear Adapter

```rust
// src/llm/infrastructure/cohere_adapter.rs
use crate::llm::domain::{
    LlmRepository, LlmRequest, LlmResponse, LlmStreamChunk, LlmError, LlmStream,
    LlmUsage, MessageRole, LlmProvider,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

pub struct CohereAdapter {
    client: Client,
    base_url: String,
}

impl CohereAdapter {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: "https://api.cohere.ai/v1".to_string(),
        }
    }

    fn convert_messages(&self, request: &LlmRequest) -> String {
        // Cohere usa un formato diferente - implementar conversión
        request.messages()
            .iter()
            .map(|msg| msg.content())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[async_trait]
impl LlmRepository for CohereAdapter {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let url = format!("{}/generate", self.base_url);

        // Preparar request específico de Cohere
        let body = serde_json::json!({
            "model": request.config().model(),
            "prompt": self.convert_messages(&request),
            "max_tokens": request.config().max_tokens().unwrap_or(1000),
            "temperature": request.config().temperature().unwrap_or(0.7),
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", request.config().api_key()))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::network_error(e.to_string()))?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(LlmError::request_failed(format!(
                "Cohere API error: {}",
                error_text
            )));
        }

        let cohere_response: CohereResponse = response
            .json()
            .await
            .map_err(|e| LlmError::parsing_error(e.to_string()))?;

        // Convertir respuesta de Cohere a formato interno
        let content = cohere_response.generations
            .first()
            .map(|gen| gen.text.clone())
            .ok_or_else(|| LlmError::parsing_error("No generation in response"))?;

        Ok(LlmResponse::new(
            request.id().clone(),
            content,
            LlmProvider::Cohere,
            request.config().model().to_string(),
        ))
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        // Implementar streaming si Cohere lo soporta
        todo!("Implementar streaming para Cohere")
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        // Test de conectividad simple
        let url = format!("{}/models", self.base_url);
        let response = self
            .client
            .get(&url)
            .header("Authorization", "Bearer dummy")
            .send()
            .await
            .map_err(|e| LlmError::network_error(e.to_string()))?;

        if response.status().is_success() || response.status().is_client_error() {
            Ok(())
        } else {
            Err(LlmError::request_failed("Cohere endpoint not available"))
        }
    }

    fn provider_name(&self) -> &'static str {
        "cohere"
    }
}

// Estructuras para deserialización de respuestas de Cohere
#[derive(Debug, Deserialize)]
struct CohereResponse {
    generations: Vec<CohereGeneration>,
}

#[derive(Debug, Deserialize)]
struct CohereGeneration {
    text: String,
}
```

### 3. Registrar en Factory

```rust
// src/llm/infrastructure/llm_provider_factory.rs
impl LlmProviderFactory {
    pub fn create_repository(provider: LlmProvider) -> Box<dyn LlmRepository> {
        match provider {
            LlmProvider::OpenAi => Box::new(OpenAiAdapter::new()),
            LlmProvider::Gemini => Box::new(GeminiAdapter::new()),
            LlmProvider::Anthropic => Box::new(AnthropicAdapter::new()),
            LlmProvider::Cohere => Box::new(CohereAdapter::new()),        // ← Añadir
            LlmProvider::Huggingface => Box::new(HuggingfaceAdapter::new()), // ← Y aquí
        }
    }
}
```

### 4. Añadir a Python Bindings

```rust
// src/python_bindings/mod.rs
impl ColmenaLlm {
    pub fn call(
        // ... parámetros existentes
    ) -> PyResult<String> {
        // Validar provider
        let provider = match provider {
            "openai" => LlmProvider::OpenAi,
            "gemini" => LlmProvider::Gemini,
            "anthropic" => LlmProvider::Anthropic,
            "cohere" => LlmProvider::Cohere,           // ← Añadir
            "huggingface" => LlmProvider::Huggingface, // ← Añadir
            _ => return Err(LlmException::new_err(format!("Unknown provider: {}", provider))),
        };

        // Resto de la implementación...
    }
}
```

### 5. Crear Tests

```rust
// tests/integration/cohere_adapter_test.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::*;
    use crate::llm::infrastructure::CohereAdapter;

    #[tokio::test]
    async fn test_cohere_call() {
        let adapter = CohereAdapter::new();

        let config = LlmConfig::new()
            .with_model("command")
            .with_api_key("test-key");

        let request = LlmRequest::new(
            vec![LlmMessage::user("Test message")],
            config,
        );

        // Este test requiere API key válida o mock
        if let Ok(response) = adapter.call(request).await {
            assert!(!response.content().is_empty());
            assert_eq!(response.provider(), &LlmProvider::Cohere);
        }
    }

    #[tokio::test]
    async fn test_cohere_health_check() {
        let adapter = CohereAdapter::new();

        // Health check no debería requerir API key válida
        let result = adapter.health_check().await;
        assert!(result.is_ok());
    }
}
```
