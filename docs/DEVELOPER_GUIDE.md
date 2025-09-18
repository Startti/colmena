# 👩‍💻 Guía del Desarrollador - Colmena

Esta guía está dirigida a desarrolladores que quieren contribuir, extender o entender en profundidad el funcionamiento de Colmena.

## 📋 Tabla de Contenidos

- [Arquitectura del Proyecto](#arquitectura-del-proyecto)
- [Configuración del Entorno de Desarrollo](#configuración-del-entorno-de-desarrollo)
- [Convenciones de Código](#convenciones-de-código)
- [Añadir Nuevos Proveedores](#añadir-nuevos-proveedores)
- [Extender Funcionalidad](#extender-funcionalidad)
- [Testing](#testing)
- [Performance y Optimización](#performance-y-optimización)
- [Deployment y Distribución](#deployment-y-distribución)

## 🏗️ Arquitectura del Proyecto

### Principios de Diseño

Colmena sigue **Arquitectura Hexagonal** (Ports and Adapters) con estos principios:

1. **Separación de Responsabilidades**: Dominio, Aplicación e Infraestructura claramente separados
2. **Inversión de Dependencias**: El dominio no depende de infraestructura
3. **Testabilidad**: Cada capa es testeable independientemente
4. **Extensibilidad**: Fácil agregar nuevos proveedores sin cambiar el core

### Estructura Detallada

```
src/
├── lib.rs                          # Entry point, expone módulos públicos
├── llm/                           # Módulo LLM (core del proyecto)
│   ├── mod.rs                     # Configuración del módulo
│   ├── domain/                    # 🏛️ CAPA DE DOMINIO
│   │   ├── mod.rs                 # Exports del dominio
│   │   ├── llm_provider.rs        # Enum de proveedores
│   │   ├── llm_config.rs          # Configuración de requests
│   │   ├── llm_request.rs         # Entidad: Request de LLM
│   │   ├── llm_response.rs        # Entidad: Response de LLM
│   │   ├── llm_repository.rs      # Port: Interfaz principal
│   │   ├── llm_error.rs           # Tipos de error del dominio
│   │   └── value_objects/         # Value Objects del dominio
│   │       ├── mod.rs
│   │       ├── llm_request_id.rs  # ID único de requests
│   │       ├── llm_message.rs     # Mensaje individual
│   │       ├── llm_usage.rs       # Métricas de uso (tokens)
│   │       └── llm_stream.rs      # Tipos para streaming
│   ├── application/               # 🎯 CAPA DE APLICACIÓN
│   │   ├── mod.rs                 # Exports de aplicación
│   │   ├── llm_call_use_case.rs   # Caso de uso: llamada síncrona
│   │   ├── llm_stream_use_case.rs # Caso de uso: streaming
│   │   └── llm_health_check_use_case.rs # Caso de uso: health check
│   └── infrastructure/            # 🔧 CAPA DE INFRAESTRUCTURA
│       ├── mod.rs                 # Exports de infraestructura
│       ├── openai_adapter.rs      # Adapter: OpenAI API
│       ├── gemini_adapter.rs      # Adapter: Gemini API
│       ├── anthropic_adapter.rs   # Adapter: Anthropic API
│       └── llm_provider_factory.rs # Factory para crear adapters
├── shared/                        # 🤝 FUNCIONALIDADES COMPARTIDAS
│   ├── mod.rs
│   └── infrastructure/
│       ├── mod.rs
│       ├── config_resolver.rs     # Resolución de configuración
│       └── service_container.rs   # Contenedor de servicios
└── python_bindings/              # 🐍 BINDINGS PARA PYTHON
    └── mod.rs                     # Wrappers PyO3
```

### Flujo de Datos

```
Python Call → PyO3 Bindings → Use Case → Repository → Adapter → HTTP API
     ↓                                                              ↓
Python Response ← PyO3 Bindings ← Domain Response ← Adapter ← HTTP Response
```

## ⚙️ Configuración del Entorno de Desarrollo

### Setup Inicial

```bash
# 1. Clonar repositorio
git clone https://github.com/tu-org/colmena.git
cd colmena

# 2. Instalar herramientas de desarrollo
cargo install cargo-watch    # Auto-recompilación
cargo install cargo-expand   # Expansión de macros
cargo install clippy         # Linter avanzado

# 3. Configurar pre-commit hooks
cargo install pre-commit
pre-commit install

# 4. Setup Python
python3 -m venv venv
source venv/bin/activate
pip install maturin pytest black isort mypy
```

### Scripts de Desarrollo

```bash
# scripts/dev.sh
#!/bin/bash

# Auto-recompilación en desarrollo
cargo watch -x "check" -x "test" -x "run"

# Compilación y test rápido
cargo check && cargo test && maturin develop

# Test completo con coverage
cargo test -- --nocapture
```

### Configuración del Editor

**VS Code (settings.json):**
```json
{
    "rust-analyzer.cargo.features": "all",
    "rust-analyzer.checkOnSave.command": "clippy",
    "python.defaultInterpreterPath": "./venv/bin/python",
    "python.linting.enabled": true,
    "python.linting.mypyEnabled": true
}
```

**Vim/Neovim:**
```lua
-- rust-tools.nvim setup
require('rust-tools').setup({
    server = {
        settings = {
            ["rust-analyzer"] = {
                cargo = { features = "all" },
                checkOnSave = { command = "clippy" }
            }
        }
    }
})
```

## 📝 Convenciones de Código

### Rust

**Nombrado:**
```rust
// Structs: PascalCase
pub struct LlmRequest { }

// Enums: PascalCase
pub enum LlmProvider { OpenAi, Gemini }

// Functions: snake_case
pub fn create_request() -> LlmRequest { }

// Constants: SCREAMING_SNAKE_CASE
const DEFAULT_TIMEOUT: u64 = 30;

// Traits: PascalCase con sufijo descriptivo
pub trait LlmRepository { }
```

**Documentación:**
```rust
/// Representa una configuración para llamadas a LLM.
///
/// # Ejemplos
///
/// ```rust
/// use colmena::llm::domain::LlmConfig;
///
/// let config = LlmConfig::new("gpt-4", "sk-...");
/// assert_eq!(config.model(), "gpt-4");
/// ```
///
/// # Errores
///
/// Esta función puede fallar si la API key está malformada.
pub struct LlmConfig {
    model: String,
    api_key: String,
}
```

**Error Handling:**
```rust
// ✅ Usar Result para operaciones que pueden fallar
pub fn call_llm() -> Result<LlmResponse, LlmError> {
    // ...
}

// ✅ Crear errores específicos del dominio
#[derive(Debug, Clone)]
pub enum LlmError {
    NetworkError(String),
    ParseError(String),
    AuthenticationError(String),
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

// ✅ Implementar Send + Sync para concurrencia
pub struct OpenAiAdapter {
    client: Client,  // reqwest::Client es Send + Sync
}

unsafe impl Send for OpenAiAdapter {}
unsafe impl Sync for OpenAiAdapter {}
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

## 🔌 Añadir Nuevos Proveedores

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

## 🧪 Testing

### Estructura de Tests

```
tests/
├── unit/                          # Tests unitarios
│   ├── domain/
│   │   ├── llm_request_test.rs
│   │   ├── llm_response_test.rs
│   │   └── llm_config_test.rs
│   ├── application/
│   │   └── use_cases_test.rs
│   └── infrastructure/
│       ├── openai_adapter_test.rs
│       ├── gemini_adapter_test.rs
│       └── anthropic_adapter_test.rs
├── integration/                   # Tests de integración
│   ├── api_integration_test.rs
│   ├── python_bindings_test.rs
│   └── end_to_end_test.rs
└── mocks/                        # Mocks y utilities
    ├── mock_http_client.rs
    ├── mock_llm_adapter.rs
    └── test_utilities.rs
```

### Test Patterns

**Mock HTTP Client:**
```rust
// tests/mocks/mock_http_client.rs
use mockito::{Mock, Server};
use serde_json::json;

pub struct MockLlmServer {
    server: Server,
}

impl MockLlmServer {
    pub async fn new() -> Self {
        Self {
            server: Server::new_async().await,
        }
    }

    pub fn mock_openai_success(&mut self) -> Mock {
        self.server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({
                "choices": [{
                    "message": {
                        "content": "Test response"
                    }
                }],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 5
                }
            }).to_string())
    }

    pub fn url(&self) -> String {
        self.server.url()
    }
}
```

**Unit Test Example:**
```rust
// tests/unit/domain/llm_request_test.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::*;

    #[test]
    fn test_llm_request_creation() {
        let config = LlmConfig::new()
            .with_model("gpt-4")
            .with_api_key("test-key");

        let messages = vec![
            LlmMessage::user("Hello"),
            LlmMessage::assistant("Hi there!"),
        ];

        let request = LlmRequest::new(messages.clone(), config.clone());

        assert_eq!(request.messages(), &messages);
        assert_eq!(request.config(), &config);
        assert!(!request.id().value().is_empty());
    }

    #[test]
    fn test_llm_request_validation() {
        let config = LlmConfig::new()
            .with_model("")  // ← Modelo vacío debería fallar
            .with_api_key("test-key");

        let messages = vec![LlmMessage::user("Hello")];

        let result = LlmRequest::try_new(messages, config);
        assert!(result.is_err());
    }
}
```

**Integration Test:**
```rust
// tests/integration/api_integration_test.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mocks::MockLlmServer;

    #[tokio::test]
    async fn test_openai_adapter_integration() {
        let mut mock_server = MockLlmServer::new().await;
        let _mock = mock_server.mock_openai_success().create_async().await;

        let adapter = OpenAiAdapter::with_base_url(mock_server.url());

        let config = LlmConfig::new()
            .with_model("gpt-4")
            .with_api_key("test-key");

        let request = LlmRequest::new(
            vec![LlmMessage::user("Test")],
            config,
        );

        let response = adapter.call(request).await.unwrap();
        assert_eq!(response.content(), "Test response");
    }
}
```

### Test Commands

```bash
# Tests unitarios únicamente
cargo test --lib

# Tests de integración únicamente
cargo test --test integration

# Tests con output detallado
cargo test -- --nocapture

# Tests específicos
cargo test test_openai_adapter

# Tests con coverage (requiere cargo-tarpaulin)
cargo tarpaulin --verbose --all-features --workspace --timeout 120
```

## 🚀 Performance y Optimización

### Profiling

```rust
// Añadir profiling markers
use std::time::Instant;

impl OpenAiAdapter {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let start = Instant::now();

        // Llamada HTTP
        let http_start = Instant::now();
        let response = self.client.post(&url).send().await?;
        let http_duration = http_start.elapsed();

        // Parsing
        let parse_start = Instant::now();
        let parsed: OpenAiResponse = response.json().await?;
        let parse_duration = parse_start.elapsed();

        let total_duration = start.elapsed();

        // Log de métricas
        log::debug!(
            "OpenAI call completed: total={}ms, http={}ms, parse={}ms",
            total_duration.as_millis(),
            http_duration.as_millis(),
            parse_duration.as_millis()
        );

        // Convertir respuesta...
        Ok(response)
    }
}
```

### Benchmark Tests

```rust
// benches/llm_benchmarks.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use colmena::llm::domain::*;

fn benchmark_request_creation(c: &mut Criterion) {
    c.bench_function("create_llm_request", |b| {
        b.iter(|| {
            let config = LlmConfig::new()
                .with_model(black_box("gpt-4"))
                .with_api_key(black_box("test-key"));

            let messages = vec![
                LlmMessage::user(black_box("Test message")),
            ];

            LlmRequest::new(black_box(messages), black_box(config))
        })
    });
}

fn benchmark_message_parsing(c: &mut Criterion) {
    let json_data = r#"
    {
        "choices": [{
            "message": {"content": "This is a test response"}
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    }
    "#;

    c.bench_function("parse_openai_response", |b| {
        b.iter(|| {
            serde_json::from_str::<OpenAiResponse>(black_box(json_data))
        })
    });
}

criterion_group!(benches, benchmark_request_creation, benchmark_message_parsing);
criterion_main!(benches);
```

### Optimizaciones Comunes

**1. Connection Pooling:**
```rust
// Reutilizar cliente HTTP
lazy_static! {
    static ref HTTP_CLIENT: Client = Client::builder()
        .pool_max_idle_per_host(10)
        .pool_idle_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client");
}

impl OpenAiAdapter {
    pub fn new() -> Self {
        Self {
            client: HTTP_CLIENT.clone(),  // ← Reutilizar cliente
            base_url: "https://api.openai.com/v1".to_string(),
        }
    }
}
```

**2. String Optimization:**
```rust
// ✅ Usar &str cuando sea posible
fn process_message(content: &str) -> String {
    content.to_uppercase()
}

// ✅ Usar Cow para evitar clones innecesarios
use std::borrow::Cow;

fn maybe_modify(input: &str, should_modify: bool) -> Cow<str> {
    if should_modify {
        Cow::Owned(input.to_uppercase())
    } else {
        Cow::Borrowed(input)
    }
}
```

**3. Async Optimization:**
```rust
// ✅ Procesar streams eficientemente
use futures::StreamExt;

async fn process_stream(stream: LlmStream) -> Result<String, LlmError> {
    let mut buffer = String::with_capacity(1024); // Pre-allocar

    tokio::pin!(stream);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(chunk.content());
    }

    Ok(buffer)
}
```

## 📦 Deployment y Distribución

### Building Wheels

```bash
# Build para múltiples plataformas
maturin build --release --target x86_64-unknown-linux-gnu
maturin build --release --target x86_64-pc-windows-msvc
maturin build --release --target x86_64-apple-darwin
maturin build --release --target aarch64-apple-darwin

# Wheels se crean en target/wheels/
ls target/wheels/
```

### GitHub Actions CI/CD

```yaml
# .github/workflows/ci.yml
name: CI

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        python-version: [3.8, 3.9, "3.10", "3.11"]

    steps:
    - uses: actions/checkout@v3

    - name: Set up Python ${{ matrix.python-version }}
      uses: actions/setup-python@v4
      with:
        python-version: ${{ matrix.python-version }}

    - name: Set up Rust
      uses: actions-rs/toolchain@v1
      with:
        toolchain: stable
        override: true

    - name: Install dependencies
      run: |
        python -m pip install --upgrade pip
        pip install maturin pytest

    - name: Build
      run: maturin develop --release

    - name: Test
      run: pytest tests/

  build:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest, macos-latest]

    steps:
    - uses: actions/checkout@v3

    - name: Set up Python
      uses: actions/setup-python@v4
      with:
        python-version: "3.11"

    - name: Set up Rust
      uses: actions-rs/toolchain@v1
      with:
        toolchain: stable
        override: true

    - name: Build wheels
      run: |
        pip install maturin
        maturin build --release

    - name: Upload wheels
      uses: actions/upload-artifact@v3
      with:
        name: wheels
        path: target/wheels/
```

### Release Process

```bash
# scripts/release.sh
#!/bin/bash

set -e

VERSION=$1
if [ -z "$VERSION" ]; then
    echo "Usage: $0 <version>"
    exit 1
fi

echo "🚀 Releasing version $VERSION"

# 1. Update version in Cargo.toml
sed -i "s/^version = .*/version = \"$VERSION\"/" Cargo.toml

# 2. Update version in pyproject.toml
sed -i "s/^version = .*/version = \"$VERSION\"/" pyproject.toml

# 3. Run tests
cargo test
python -m pytest

# 4. Build wheels for all platforms
maturin build --release

# 5. Create git tag
git add .
git commit -m "Release v$VERSION"
git tag "v$VERSION"

# 6. Push to repository
git push origin main
git push origin "v$VERSION"

# 7. Upload to PyPI (manual step)
echo "✅ Release prepared. Run 'maturin publish' to upload to PyPI"
```

### Versioning Strategy

```toml
# Cargo.toml
[package]
version = "0.1.0"    # SemVer: MAJOR.MINOR.PATCH

# 0.x.y = Pre-1.0, breaking changes allowed
# 1.x.y = Stable API, breaking changes require major version bump
# x.y.Z = Bug fixes only
# x.Y.z = New features, backward compatible
# X.y.z = Breaking changes
```

## 🤝 Contribuir al Proyecto

### Pull Request Process

1. **Fork y Clone**
2. **Crear Feature Branch**: `git checkout -b feature/nueva-funcionalidad`
3. **Implementar y Testear**: Seguir convenciones de código
4. **Documentation**: Actualizar docs si es necesario
5. **PR**: Crear pull request con descripción detallada

### Code Review Checklist

- [ ] Código sigue convenciones del proyecto
- [ ] Tests añadidos para nueva funcionalidad
- [ ] Documentación actualizada
- [ ] No hay warnings de clippy
- [ ] Performance considerada
- [ ] Manejo de errores apropiado
- [ ] API backward compatible (si aplica)

---

**🐝 Colmena** - *Construyendo el futuro de la orquestación de IA*

> 💡 **Para Desarrolladores**: Siempre prioriza la claridad del código sobre la optimización prematura. Rust ya es rápido por defecto.