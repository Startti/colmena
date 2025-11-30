# Documento de Diseño y Desarrollo (DDS) - Módulo LLM

## 1. Resumen Ejecutivo

### 1.1 Propósito
Diseñar e implementar el primer módulo base de la librería **Colmena** para la orquestación de agentes de IA. Este módulo proporcionará una abstracción unificada para realizar llamadas a diferentes proveedores de Large Language Models (LLMs) como OpenAI, Gemini y Anthropic.

### 1.2 Objetivos
- Implementar una interfaz unificada para múltiples proveedores de LLM
- Soportar tanto llamadas normales como streaming
- Gestión flexible de API keys (variables de entorno o valores fijos)
- Arquitectura extensible siguiendo principios hexagonales
- Exposición a Python mediante PyO3

### 1.3 Alcance
- **Incluye**: Abstracción de LLMs, configuración de API keys, llamadas síncronas y streaming
- **Excluye**: Funcionalidades avanzadas de agentes, persistencia, UI

## 2. Arquitectura del Sistema

### 2.1 Principios Arquitectónicos
Siguiendo la **Arquitectura Hexagonal**:
- **Dominio**: Lógica de negocio pura para LLMs
- **Aplicación**: Casos de uso para llamadas a LLMs
- **Infraestructura**: Adaptadores para cada proveedor específico

### 2.2 Diagrama de Arquitectura

```
┌─────────────────────────────────────────┐
│              PYTHON LAYER               │
│         (PyO3 Bindings)                │
└─────────────────┬───────────────────────┘
                  │
┌─────────────────▼───────────────────────┐
│           APPLICATION LAYER             │
│  ┌─────────────────┐ ┌─────────────────┐│
│  │  LlmCallUseCase │ │LlmStreamUseCase ││
│  └─────────────────┘ └─────────────────┘│
└─────────────────┬───────────────────────┘
                  │
┌─────────────────▼───────────────────────┐
│             DOMAIN LAYER                │
│  ┌─────────────────┐ ┌─────────────────┐│
│  │   LlmRequest    │ │   LlmResponse   ││
│  │   LlmConfig     │ │   LlmProvider   ││
│  │  LlmRepository  │ │   (trait)       ││
│  └─────────────────┘ └─────────────────┘│
└─────────────────┬───────────────────────┘
                  │
┌─────────────────▼───────────────────────┐
│         INFRASTRUCTURE LAYER            │
│  ┌─────────────┐┌─────────────┐┌───────┐│
│  │OpenAiAdapter││GeminiAdapter││Claude ││
│  │             ││             ││Adapter││
│  └─────────────┘└─────────────┘└───────┘│
└─────────────────────────────────────────┘
```

## 3. Diseño Detallado

### 3.1 Capa de Dominio

#### 3.1.1 Value Objects

##### LlmProvider
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmProvider {
    OpenAi,
    Gemini,
    Anthropic,
}
```

##### LlmConfig
```rust
#[derive(Debug, Clone)]
pub struct LlmConfig {
    provider: LlmProvider,
    api_key: String,
    model: String,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
}
```

##### LlmRequest
```rust
#[derive(Debug, Clone)]
pub struct LlmRequest {
    id: LlmRequestId,
    messages: Vec<LlmMessage>,
    config: LlmConfig,
    stream: bool,
}
```

##### LlmResponse
```rust
#[derive(Debug, Clone)]
pub struct LlmResponse {
    id: LlmResponseId,
    request_id: LlmRequestId,
    content: String,
    usage: Option<LlmUsage>,
    provider: LlmProvider,
}
```

#### 3.1.2 Entidades

##### LlmMessage
```rust
#[derive(Debug, Clone)]
pub struct LlmMessage {
    role: MessageRole,
    content: String,
    timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
}
```

#### 3.1.3 Puerto (Trait)

##### LlmRepository
```rust
#[async_trait]
pub trait LlmRepository: Send + Sync {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;
    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError>;
}
```

#### 3.1.4 Errores de Dominio

```rust
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("Invalid API key")]
    InvalidApiKey,
    #[error("Provider not supported: {provider}")]
    UnsupportedProvider { provider: String },
    #[error("Request failed: {message}")]
    RequestFailed { message: String },
    #[error("Configuration error: {message}")]
    ConfigurationError { message: String },
}
```

### 3.2 Capa de Aplicación

#### 3.2.1 Casos de Uso

##### LlmCallUseCase
```rust
pub struct LlmCallUseCase {
    repository: Arc<dyn LlmRepository>,
}

impl LlmCallUseCase {
    pub async fn execute(
        &self,
        messages: Vec<String>,
        provider: LlmProvider,
        api_key: Option<String>,
        config: Option<LlmCallConfig>,
    ) -> Result<LlmResponse, LlmError> {
        // 1. Validar y crear objetos de dominio
        // 2. Resolver API key (env var o parámetro)
        // 3. Ejecutar llamada
        // 4. Retornar respuesta
    }
}
```

##### LlmStreamUseCase
```rust
pub struct LlmStreamUseCase {
    repository: Arc<dyn LlmRepository>,
}

impl LlmStreamUseCase {
    pub async fn execute(
        &self,
        messages: Vec<String>,
        provider: LlmProvider,
        api_key: Option<String>,
        config: Option<LlmCallConfig>,
    ) -> Result<LlmStream, LlmError> {
        // Similar a LlmCallUseCase pero retorna stream
    }
}
```

### 3.3 Capa de Infraestructura

#### 3.3.1 Adaptadores por Proveedor

##### OpenAiAdapter
```rust
pub struct OpenAiAdapter {
    client: reqwest::Client,
}

#[async_trait]
impl LlmRepository for OpenAiAdapter {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        // Implementación específica para OpenAI API
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        // Implementación de streaming para OpenAI
    }
}
```

##### GeminiAdapter
```rust
pub struct GeminiAdapter {
    client: reqwest::Client,
}

#[async_trait]
impl LlmRepository for GeminiAdapter {
    // Implementación específica para Gemini API
}
```

##### AnthropicAdapter
```rust
pub struct AnthropicAdapter {
    client: reqwest::Client,
}

#[async_trait]
impl LlmRepository for AnthropicAdapter {
    // Implementación específica para Anthropic API
}
```

#### 3.3.2 Factory Pattern

```rust
pub struct LlmProviderFactory;

impl LlmProviderFactory {
    pub fn create(provider: LlmProvider) -> Arc<dyn LlmRepository> {
        match provider {
            LlmProvider::OpenAi => Arc::new(OpenAiAdapter::new()),
            LlmProvider::Gemini => Arc::new(GeminiAdapter::new()),
            LlmProvider::Anthropic => Arc::new(AnthropicAdapter::new()),
        }
    }
}
```

## 4. Integración con Python

### 4.1 PyO3 Bindings

```rust
use pyo3::prelude::*;

#[pyclass]
pub struct ColmenaLlm {
    call_use_case: LlmCallUseCase,
    stream_use_case: LlmStreamUseCase,
}

#[pymethods]
impl ColmenaLlm {
    #[new]
    pub fn new() -> Self {
        // Inicialización
    }

    #[pyo3(signature = (messages, provider, api_key=None, **kwargs))]
    pub fn call(
        &self,
        messages: Vec<String>,
        provider: &str,
        api_key: Option<String>,
        kwargs: Option<&PyDict>,
    ) -> PyResult<String> {
        // Wrapper para llamada normal
    }

    #[pyo3(signature = (messages, provider, api_key=None, **kwargs))]
    pub fn stream(
        &self,
        messages: Vec<String>,
        provider: &str,
        api_key: Option<String>,
        kwargs: Option<&PyDict>,
    ) -> PyResult<PyObject> {
        // Wrapper para streaming
    }
}

#[pymodule]
fn colmena(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<ColmenaLlm>()?;
    Ok(())
}
```

### 4.2 Uso desde Python

```python
import colmena

# Inicializar
llm = colmena.ColmenaLlm()

# Llamada normal
response = llm.call(
    messages=["Hello, how are you?"],
    provider="openai",
    api_key="sk-...",  # Opcional, puede usar OPENAI_API_KEY
    model="gpt-4",
    temperature=0.7
)

# Llamada con streaming
for chunk in llm.stream(
    messages=["Tell me a story"],
    provider="anthropic",
    model="claude-3-sonnet"
):
    print(chunk, end="")
```

## 5. Gestión de Configuración

### 5.1 Variables de Entorno

```rust
pub struct ConfigResolver;

impl ConfigResolver {
    pub fn resolve_api_key(provider: LlmProvider, explicit_key: Option<String>) -> Result<String, LlmError> {
        if let Some(key) = explicit_key {
            return Ok(key);
        }

        let env_var = match provider {
            LlmProvider::OpenAi => "OPENAI_API_KEY",
            LlmProvider::Gemini => "GEMINI_API_KEY",
            LlmProvider::Anthropic => "ANTHROPIC_API_KEY",
        };

        std::env::var(env_var)
            .map_err(|_| LlmError::ConfigurationError {
                message: format!("API key not found in environment variable {}", env_var)
            })
    }
}
```

## 6. Estructura de Directorios

```
src/
├── lib.rs
├── python_bindings/
│   └── mod.rs
├── shared/
│   ├── mod.rs
│   └── infrastructure/
│       ├── mod.rs
│       ├── service_container.rs
│       └── config_resolver.rs
└── llm/
    ├── mod.rs
    ├── domain/
    │   ├── mod.rs
    │   ├── llm_request.rs
    │   ├── llm_response.rs
    │   ├── llm_config.rs
    │   ├── llm_message.rs
    │   ├── llm_provider.rs
    │   ├── llm_repository.rs
    │   ├── llm_error.rs
    │   └── value_objects/
    │       ├── mod.rs
    │       ├── llm_request_id.rs
    │       └── llm_response_id.rs
    ├── application/
    │   ├── mod.rs
    │   ├── llm_call_use_case.rs
    │   └── llm_stream_use_case.rs
    └── infrastructure/
        ├── mod.rs
        ├── openai_adapter.rs
        ├── gemini_adapter.rs
        ├── anthropic_adapter.rs
        └── llm_provider_factory.rs
```

## 7. Plan de Implementación

### Fase 1: Fundación (Semana 1)
1. Configurar estructura de proyecto y dependencias
2. Implementar capa de dominio base
3. Crear value objects y entidades principales
4. Definir traits y errores

### Fase 2: Aplicación (Semana 2)
1. Implementar casos de uso básicos
2. Crear lógica de resolución de configuración
3. Implementar factory pattern
4. Testing de capa de aplicación

### Fase 3: Infraestructura (Semana 3)
1. Implementar OpenAiAdapter
2. Implementar GeminiAdapter
3. Implementar AnthropicAdapter
4. Testing de adaptadores

### Fase 4: Integración Python (Semana 4)
1. Configurar PyO3 bindings
2. Crear wrapper de Python
3. Testing de integración
4. Documentación y ejemplos

## 8. Dependencias Requeridas

```toml
[dependencies]
# Async runtime
tokio = { version = "1.0", features = ["full"] }

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# HTTP client
reqwest = { version = "0.11", features = ["json", "stream"] }

# Error handling
thiserror = "1.0"
anyhow = "1.0"

# Async traits
async-trait = "0.1"

# Date/time
chrono = { version = "0.4", features = ["serde"] }

# UUID generation
uuid = { version = "1.0", features = ["v4", "serde"] }

# Python bindings
pyo3 = { version = "0.20", features = ["extension-module"] }

# Streaming
futures = "0.3"
tokio-stream = "0.1"

# Environment variables
dotenvy = "0.15"

[lib]
name = "colmena"
crate-type = ["cdylib", "rlib"]
```

## 9. Criterios de Aceptación

- [ ] Llamadas exitosas a OpenAI, Gemini y Anthropic
- [ ] Soporte para streaming en todos los proveedores
- [ ] Gestión flexible de API keys
- [ ] Interfaz Python funcional
- [ ] Cobertura de tests > 80%
- [ ] Documentación completa con ejemplos
- [ ] Manejo robusto de errores
- [ ] Performance aceptable (< 2s por llamada)

## 10. Riesgos y Mitigaciones

| Riesgo | Probabilidad | Impacto | Mitigación |
|--------|--------------|---------|------------|
| Cambios en APIs de proveedores | Media | Alto | Abstracciones robustas y testing continuo |
| Complejidad de streaming | Alta | Medio | Prototipos tempranos y documentación |
| Performance de PyO3 | Baja | Medio | Benchmarking y optimización |
| Gestión de errores async | Media | Medio | Testing exhaustivo y logging |

## 11. Integración con DAG Engine

### 11.1 Propósito

El módulo LLM se integra con el DAG Engine permitiendo orquestar llamadas a LLMs como parte de workflows complejos. Esto permite:
- Encadenar múltiples llamadas a LLMs
- Combinar LLMs con otras operaciones (HTTP, matemáticas, etc.)
- Configurar LLMs dinámicamente basado en outputs de otros nodos
- Orquestar pipelines de procesamiento de datos con IA

### 11.2 LlmNode

El `LlmNode` es un adaptador que implementa `ExecutableNode` y expone el módulo LLM al DAG Engine.

```rust
// src/dag_engine/infrastructure/nodes/llm.rs
pub struct LlmNode;

#[async_trait::async_trait]
impl ExecutableNode for LlmNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
    ) -> Result<Value, Box<dyn StdError>> {
        // 1. Resolver configuración (inputs > config)
        let provider_kind = resolve_provider(inputs, config)?;
        let api_key = resolve_api_key(inputs, config)?;
        let model = resolve_model(inputs, config);
        
        // 2. Crear provider y configuración LLM
        let provider = LlmProvider::new(provider_kind, api_key, model)?;
        let mut llm_config = LlmConfig::new(provider);
        
        // Aplicar parámetros opcionales
        if let Some(temp) = resolve_temperature(inputs, config) {
            llm_config = llm_config.with_temperature(temp)?;
        }
        
        // 3. Construir mensajes
        let mut messages = Vec::new();
        if let Some(system) = resolve_system_message(inputs, config) {
            messages.push(LlmMessage::system(system)?);
        }
        let prompt = resolve_prompt(inputs, config)?;
        messages.push(LlmMessage::user(prompt)?);
        
        // 4. Ejecutar llamada LLM
        let repository = LlmProviderFactory::create(provider_kind);
        let use_case = LlmCallUseCase::new(repository);
        let response = use_case.execute(messages, llm_config).await?;
        
        // 5. Retornar output
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
```

### 11.3 Caso de Uso: Pipeline HTTP → LLM

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/analyze-article",
        "method": "POST",
        "test_payload": {
          "url": "https://api.example.com/article"
        }
      }
    },
    "fetch_article": {
      "type": "http_request",
      "config": {
        "method": "GET"
      }
    },
    "analyze": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-4",
        "system_message": "You are an expert content analyst.",
        "max_tokens": 500
      }
    },
    "log_analysis": {
      "type": "log"
    }
  },
  "edges": [
    {
      "from": "trigger.output.url",
      "to": "fetch_article.base_url"
    },
    {
      "from": "fetch_article.output.body.content",
      "to": "analyze.prompt"
    },
    {
      "from": "analyze.output",
      "to": "log_analysis.input"
    }
  ]
}
```

### 11.4 Beneficios de la Integración

1. **Composabilidad**: LLMs se combinan fácilmente con otros nodos
2. **Configuración Dinámica**: Prompts y parámetros desde datos upstream
3. **Reutilización**: Mismo módulo LLM en Python y DAG Engine
4. **Testability**: Workflows completos testeables con `test_payload`
5. **Observabilidad**: Logging unificado de todo el pipeline

### 11.5 Referencias

- [DAG Engine Developer Guide](../developer_guide/12_dag_engine_guide.md)
- [DAG Engine Design](DAG_ENGINE_DISEÑO.md)
- [Usage Examples](../examples/USAGE_EXAMPLES.md)

Este diseño proporciona una base sólida para el módulo LLM de Colmena, siguiendo principios de arquitectura hexagonal y permitiendo fácil extensión para futuros proveedores y funcionalidades.