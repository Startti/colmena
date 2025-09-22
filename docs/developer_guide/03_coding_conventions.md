# 📝 Convenciones de Código

### Rust

**Nombrado:**
```rust
// Structs: PascalCase
pub struct LlmRequest { }

// Enums: PascalCase
pub enum ProviderKind { OpenAi, Gemini }

// Functions: snake_case
pub fn create_request() -> LlmRequest { }

// Constants: SCREAMING_SNAKE_CASE
pub const INVALID_TEMPERATURE: &str = "Temperature must be between 0.0 and 2.0";

// Traits: PascalCase con sufijo descriptivo
pub trait LlmRepository { }
```

**Documentación:**
```rust
/// Representa una configuración para llamadas a LLM.
///
/// Se construye utilizando un patrón builder para una configuración fluida.
///
/// # Ejemplos
///
/// ```rust
/// use colmena::llm::domain::{LlmConfig, LlmProvider, ProviderKind};
///
/// // El provider se crea primero, gestionando la API key y el modelo.
/// let provider = LlmProvider::new(
///     ProviderKind::OpenAi,
///     "test_api_key".to_string(),
///     Some("gpt-4".to_string())
/// ).unwrap();
///
/// // LlmConfig usa el provider y se configura con el patrón builder.
/// let config = LlmConfig::new(provider)
///     .with_temperature(0.8)
///     .unwrap()
///     .with_max_tokens(1024)
///     .unwrap();
///
/// assert_eq!(config.temperature(), Some(0.8));
/// ```
pub struct LlmConfig {
    // ... campos privados
}
```

**Error Handling:**
```rust
// ✅ Usar Result para operaciones que pueden fallar
pub fn call_llm() -> Result<LlmResponse, LlmError> {
    // ...
}

// ✅ Crear errores específicos del dominio con `thiserror`
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("Configuration error: {message}")]
    ConfigurationError { message: String },

    #[error("Network error: {message}")]
    NetworkError { message: String },
}

// ✅ Usar ? operator para propagación de errores
pub fn complex_operation() -> Result<String, LlmError> {
    let response = call_api()?;
    let parsed = parse_response(response)?;
    Ok(parsed.content)
}
```

**Async/Await:**
```rust
// ✅ Usar async/await consistentemente
#[async_trait]
pub trait LlmRepository {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;
}

// ✅ reqwest::Client ya es Send + Sync, por lo que los adaptadores
// que lo contienen también lo son de forma segura sin `unsafe`.
pub struct OpenAiAdapter {
    client: Client,
}
```

### Python Bindings

**PyO3 Patterns:**
```rust
#[pyclass]
pub struct ColmenaLlm {
    container: ServiceContainer,
}

#[pymethods]
impl ColmenaLlm {
    #[new]
    pub fn new() -> Self {
        Self {
            container: ServiceContainer::new(),
        }
    }

    /// Realizar llamada síncrona a LLM
    ///
    /// Args:
    ///     messages: Lista de mensajes de conversación
    ///     provider: Proveedor a usar ('openai', 'gemini', 'anthropic')
    ///     api_key: API key del proveedor (opcional si está en env)
    ///     model: Modelo específico a usar (opcional)
    ///     temperature: Creatividad de la respuesta [0.0-1.0] (opcional)
    ///     max_tokens: Máximo tokens de respuesta (opcional)
    ///
    /// Returns:
    ///     str: Respuesta del LLM
    ///
    /// Raises:
    ///     LlmException: Si hay error en la llamada
    pub fn call(
        &self,
        py: Python,
        messages: Vec<String>,
        provider: &str,
        api_key: Option<String>,
        model: Option<String>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        top_p: Option<f32>,
    ) -> PyResult<String> {
        // Implementación...
    }
}
```
