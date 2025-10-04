# 🔌 Añadir Nuevos Proveedores

### 1. Definir Proveedor en el Dominio

```rust
// src/llm/domain/llm_provider.rs
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderKind {
    OpenAi,
    Gemini,
    Anthropic,
    Mistral,        // ← Nuevo proveedor
}

impl ProviderKind {
    pub fn from_str(s: &str) -> Result<Self, LlmError> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(Self::OpenAi),
            "gemini" => Ok(Self::Gemini),
            "anthropic" => Ok(Self::Anthropic),
            "mistral" => Ok(Self::Mistral),        // ← Añadir aquí
            _ => Err(LlmError::UnsupportedProvider { provider: s.to_string() }),
        }
    }
}
```

### 2. Crear Adapter

Crea un nuevo fichero, por ejemplo `src/llm/infrastructure/mistral_adapter.rs`. Este adaptador debe implementar el trait `LlmRepository`.

```rust
// src/llm/infrastructure/mistral_adapter.rs
use crate::llm::domain::{
    LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmError,
};
use async_trait::async_trait;
use reqwest::Client;

pub struct MistralAdapter {
    client: Client,
    base_url: String,
}

impl MistralAdapter {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: "https://api.mistral.ai/v1".to_string(),
        }
    }
}

#[async_trait]
impl LlmRepository for MistralAdapter {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        // 1. Construir el cuerpo de la petición (body) específico para Mistral
        // 2. Realizar la llamada HTTP con reqwest
        // 3. Parsear la respuesta del API
        // 4. Convertir la respuesta a la entidad LlmResponse del dominio
        todo!("Implementar la llamada para Mistral")
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        // Implementar streaming si el API de Mistral lo soporta
        todo!("Implementar streaming para Mistral")
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        // Implementar una comprobación simple de conectividad
        todo!("Implementar health check para Mistral")
    }

    fn provider_name(&self) -> &'static str {
        "mistral"
    }
}
```

### 3. Registrar en Factory

```rust
// src/llm/infrastructure/llm_provider_factory.rs
use crate::llm::infrastructure::MistralAdapter; // ← Importar nuevo adapter

impl LlmProviderFactory {
    pub fn create(provider: ProviderKind) -> Arc<dyn LlmRepository> {
        match provider {
            ProviderKind::OpenAi => Arc::new(OpenAiAdapter::new()),
            ProviderKind::Gemini => Arc::new(GeminiAdapter::new()),
            ProviderKind::Anthropic => Arc::new(AnthropicAdapter::new()),
            ProviderKind::Mistral => Arc::new(MistralAdapter::new()), // ← Añadir
        }
    }
}
```

### 4. Añadir a Python Bindings

```rust
// src/python_bindings/mod.rs

// No es necesario modificar los bindings si se usa el ServiceContainerFactory,
// ya que este puede registrar todos los providers disponibles dinámicamente.
// Si la lógica es manual, se añadiría aquí:

// let provider_kind = ProviderKind::from_str(provider)?;
// match provider_kind { ... }
```

### 5. Crear Tests

Añade tests de integración para tu nuevo adaptador en el directorio `tests/`.

```rust
// tests/mistral_adapter_test.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::*;
    use crate::llm::infrastructure::MistralAdapter;
    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path};

    #[tokio::test]
    async fn test_mistral_adapter_call_success() {
        // Iniciar servidor mock con wiremock
        let server = MockServer::start().await;

        // Configurar una respuesta mock
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions")) // Ajustar al endpoint correcto de Mistral
            .respond_with(ResponseTemplate::new(200).set_body_json(/* ... */))
            .mount(&server)
            .await;

        // Crear adaptador apuntando al servidor mock
        let adapter = MistralAdapter::with_base_url(server.uri());

        // Ejecutar la llamada y verificar el resultado
        // ...
    }
}
```
