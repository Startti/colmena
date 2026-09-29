# 🔌 Añadir Nuevos Proveedores

### 1. Definir Proveedor en el Dominio

```rust
// src/libs/colmena/src/llm/domain/llm_provider.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    OpenAi,
    Google,
    Anthropic,
    Mock,
    Generated,
    Mistral,        // ← Nuevo proveedor
}

impl FromStr for ProviderKind {
    type Err = LlmError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(ProviderKind::OpenAi),
            "google" => Ok(ProviderKind::Google),
            "anthropic" => Ok(ProviderKind::Anthropic),
            "mock" => Ok(ProviderKind::Mock),
            "generated" => Ok(ProviderKind::Generated),
            "mistral" => Ok(ProviderKind::Mistral),        // ← Añadir aquí
            _ => Err(LlmError::UnsupportedProvider { provider: s.to_string() }),
        }
    }
}
```

**Provider naming:** The provider identifier for Google's LLM is `"google"` (not `"gemini"`) — Google is the company; Gemini is the product family Google ships. However, the env var name `GEMINI_API_KEY` and model identifiers like `gemini-2.5-flash` remain unchanged because those are the official names Google uses in its Gemini SDK and docs. Internally, the Rust adapter struct is still named `GeminiAdapter` for the same reason.

### 2. Crear Adapter

Crea un nuevo fichero, por ejemplo `src/libs/colmena/src/llm/infrastructure/mistral_adapter.rs`. Este adaptador debe implementar el trait `LlmRepository`.

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

Los envíos de `call` y `stream` pasan por `transient::send_with_transient_retry`
(reenvío ante un estado transitorio; ver [18_troubleshooting.md](18_troubleshooting.md)).

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


### 5. Capacidades que no son chat: modelos de decisión tipada

No todo modelo es un proveedor de chat. `LlmRepository` promete mensajes de entrada y texto (más
tool calls) de salida; un modelo que no genera texto no cumple ese contrato y no se registra ahí.
Para esos casos hay un puerto propio, igual que TTS tiene `TtsRepository`.

`DecisionModelRepository` (`llm/domain/decision_model_repository.rs`) recibe un `state` (texto, objeto
JSON o arreglo, nunca `null`) y preguntas tipadas, y devuelve respuestas tipadas con probabilidades:

| Pregunta (`QuestionKind`) | Qué pide | Respuesta (`Answer`) |
|---|---|---|
| `Noul { criteria }` | sí/no; `criteria` opcional con `when_true` / `when_false` | `Noul { probability }` (0 a 1) |
| `Choice { options }` | elegir una opción de un conjunto cerrado; cada opción con descripción opcional | `Choice { choice, probabilities, confidence }` |
| `Score { levels }` | ubicar en una escala ordenada (`levels[0]` es el nivel más bajo) | `Score { score, probabilities, confidence }` (`score` fraccional) |

El dominio (`llm/domain/decision_model.rs`) valida solo reglas neutrales, antes de cualquier llamada:
`state` no nulo, al menos una pregunta, ids de pregunta únicos, al menos una opción con claves únicas y
al menos un nivel. Los límites de cada proveedor (por ejemplo, máximo de opciones o de niveles) los
valida su adapter, también antes de la red. `DecisionUsage { input_tokens, output_tokens }` se reporta
como `NodeEvent::LlmUsage` (prompt y completion).

Este modelo no escribe texto: solo elige entre lo que se le ofrece. Para extraer un valor del texto,
el código propone los candidatos y el modelo elige uno. Si el valor correcto puede no estar entre los
candidatos, incluí una opción de escape: sin ella, el modelo elige igual la opción más parecida.
El primer adapter es TypeSafe Jev (`llm/infrastructure/typesafe_jev_adapter.rs`), que se construye
con `build_decision_model_repository("typesafe", api_key)`. Hace `POST https://api.typesafe.ai/v1/systemone`
con `Authorization: Bearer <api_key>`, timeout de 10 s y sin reintentos: un error del proveedor falla
la llamada. Antes de la red valida los límites de Jev (máximo 255 opciones por `choice` y 10 niveles
por `score`). La `api_key` es obligatoria y explícita (por ejemplo `"${TYPESAFE_API_KEY}"`); vacía, es
un error de configuración que nombra `TYPESAFE_API_KEY`. Los errores se mapean así: 401/403 →
`Auth`; 400/422 → `InvalidRequest` (con `error_type` cuando viene); 429 → `RateLimited`; 5xx, incluido
529 → `Upstream`; timeout → `Timeout`; un 200 que no se entiende, o que elige una opción que no se
ofreció → `MalformedResponse`.
