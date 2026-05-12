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
pub fn with_temperature(mut self, temperature: f32) -> Result<Self, LlmError> {
    // ...
}

// ✅ Crear errores específicos y descriptivos del dominio con `thiserror`.
// El mensaje de error está acoplado al tipo de error.
#[derive(Debug, Error, PartialEq)]
pub enum LlmError {
    #[error("Invalid API key")]
    InvalidApiKey,

    #[error("Provider not supported: {provider}")]
    UnsupportedProvider { provider: String },

    #[error("Temperature must be between 0.0 and 2.0")]
    InvalidTemperature,

    #[error("Network error: {message}")]
    NetworkError { message: String },
}

// ✅ Usar ? operator para propagación de errores.
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

### Manejo de Errores: Domain vs Infrastructure

1. **Domain (`thiserror`)**: Los errores que representan casos de negocio o fallos esperados del dominio deben usar `thiserror`. Deben ser un `enum` con variantes claras y descriptivas.
2. **Infrastructure (`anyhow`)**: Para fallos técnicos impredecibles en la capa de infraestructura (errores de red, fallos de IO, errores de bases de datos de terceros), se prefiere el uso de `anyhow::Result` o `Box<dyn std::error::Error>` para facilitar la propagación hacia arriba cuando no se requiere un manejo específico de la variante del error.

### DAG Engine: Implementación de Nodos

Al implementar el trait `ExecutableNode`, se deben seguir estas reglas:

1. **Precedencia de Configuración**: La configuración dinámica (`inputs`) siempre tiene prioridad sobre la estática (`config`).
   ```rust
   // ✅ Patrón correcto
   let model = inputs.get("model")
       .and_then(|v| v.as_str())
       .or_else(|| config.get("model").and_then(|v| v.as_str()))
       .map(|s| s.to_string());
   ```
2. **Resultados Estructurados**: Todos los nodos deben devolver su resultado principal envuelto en un objeto JSON bajo la clave `"output"`.
   ```rust
   // ✅ Convención de salida
   Ok(json!({ "output": result_value }))
   ```
3. **Estado Inmutable**: Los nodos no deben mantener estado interno persistente entre ejecuciones del DAG a menos que utilicen explícitamente el parámetro `state: &mut Value` proporcionado en el método `execute`.

### Python Bindings

**PyO3 Patterns:**
```rust
#[pyclass]
pub struct ColmenaLlm {
    // ...
}

#[pymethods]
impl ColmenaLlm {
    #[new]
    pub fn new() -> PyResult<Self> {
        // ...
    }

    /// Realizar llamada síncrona a LLM
    ///
    /// Args:
    ///     messages (list[dict]): Lista de mensajes. Cada mensaje es un diccionario
    ///         con claves "role" (str) y "content" (str).
    ///     provider (str): Proveedor a usar ('openai', 'google', 'anthropic').
    ///     api_key (str, optional): API key del proveedor.
    ///     model (str, optional): Modelo específico a usar.
    ///     temperature (float, optional): Creatividad de la respuesta [0.0-2.0].
    ///     max_tokens (int, optional): Máximo de tokens en la respuesta.
    ///     top_p (float, optional): Nucleus sampling.
    ///
    /// Returns:
    ///     str: Respuesta del LLM
    ///
    /// Raises:
    ///     LlmException: Si hay un error en la llamada.
    pub fn call(
        &self,
        py: Python,
        messages: Vec<&PyDict>,
        provider: &str,
        api_key: Option<String>,
        model: Option<String>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        top_p: Option<f32>,
        frequency_penalty: Option<f32>,
        presence_penalty: Option<f32>,
    ) -> PyResult<String> {
        // Implementación...
    }
}
```
