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
#[async_trait]
impl LlmRepository for MistralAdapter {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        // 1. Mapear LlmRequest (incluyendo messages y tools) al formato de Mistral
        // 2. Realizar POST con reqwest
        // 3. Convertir respuesta JSON a LlmResponse (mapeando contenido y usage)
        // 4. SI SOPORTA TOOLS: Mapear tool_calls del API a domain::ToolCall
        todo!()
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        // Es obligatorio implementar streaming. Recomendamos usar `async_stream::try_stream!`
        // para emitir `LlmStreamChunk` (Content, Usage o ToolCallChunk)
        todo!()
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        // GET simple a /models o similar para verificar API Key/Conexión
        todo!()
    }

    fn provider_name(&self) -> &'static str { "mistral" }
}
```

### 🧠 Consideraciones Avanzadas (v0.3.0)

#### 1. Soporte Multimedia (Vision & Documents)
Si el proveedor soporta imágenes o PDFs, debes verificar los archivos en `request.messages()`.
- Iterar sobre `msg.files()`.
- Convertir `file.bytes` (Base64) según el esquema del API.
- Si solo soporta formatos específicos (ej. OpenAI solo imágenes en chat completions), maneja el error o usa "Hybrid Routing" hacia otro endpoint.

#### 2. Tool Calling
Para proveedores con soporte de funciones:
- **Request**: Enviar `request.tools()` convertido al formato JSON del proveedor.
- **Response**: Si el modelo decide usar una herramienta, el adaptador debe devolver un `LlmResponse` donde `tool_calls()` sea `Some(Vec<ToolCall>)`.
- **Streaming**: Emitir `LlmStreamPart::ToolCallChunk` para cada fragmento de argumentos recibido.

#### 3. Error Mapping
No devuelvas errores genéricos de `reqwest`. Usa el helper `LlmError` para categorizar:
- `LlmError::network_error(e)`
- `LlmError::parsing_error(e)`
- `LlmError::request_failed(msg)` (para errores 4xx/5xx del API)

### 3. Registrar en Factory

```rust
// src/libs/colmena/src/llm/infrastructure/llm_provider_factory.rs
impl LlmProviderFactory {
    pub fn create(kind: ProviderKind) -> Arc<dyn LlmRepository> {
        match kind {
            // ...
            ProviderKind::Mistral => Arc::new(MistralAdapter::new()),
        }
    }
}
```

### 4. Tests de Integración

Es crítico testear tanto `call` como `stream`. Se recomienda usar el crate `wiremock` para simular las respuestas del API y verificar que el mapeo de `ToolCall` y `Usage` sea correcto.

