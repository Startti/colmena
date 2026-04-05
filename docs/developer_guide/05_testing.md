# 🧪 Testing

### Estrategia de Tests

En Colmena, seguimos la estrategia de testing idiomática de Rust:

1.  **Tests Unitarios (`#[cfg(test)]`)**:
    *   **Ubicación**: Se encuentran en un módulo `mod tests { ... }` dentro del mismo fichero que el código que prueban.
    *   **Propósito**: Probar la lógica interna de una función o un módulo de forma aislada. Tienen acceso a funciones y tipos privados.
    *   **Ejemplo**: Testear la lógica de validación de `LlmConfig` sin depender de nada más.

    ```rust
    // src/llm/domain/llm_config.rs
    #[test]
    fn test_with_temperature_invalid() {
        let provider = create_test_provider();
        let config = LlmConfig::new(provider);

        // Se comprueba que un valor inválido devuelve la variante de error correcta.
        let result = config.with_temperature(2.5);
        assert_eq!(result.unwrap_err(), LlmError::InvalidTemperature);
    }
    ```

2.  **Tests de Integración (`tests/`)**:
    *   **Ubicación**: Cada fichero `.rs` en el directorio `tests/` en la raíz del proyecto es un test de integración.
    *   **Propósito**: Probar la API pública de la librería. Simulan cómo un usuario externo interactuaría con Colmena, asegurando que las diferentes partes del sistema funcionan bien juntas.
    *   **Ejemplo**: Testear un `LlmCallUseCase` completo, usando un `LlmRepository` mockeado para simular la capa de infraestructura.

### Test Patterns

**Mocking con `mockall`**:
Para los tests de aplicación, usamos `mockall` para crear mocks de nuestras dependencias (traits).

```rust
// src/llm/domain/llm_repository.rs
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait LlmRepository {
    // ...
}

// En el test de un caso de uso:
#[tokio::test]
async fn test_llm_call_use_case_success() {
    let mut mock_repo = MockLlmRepository::new();

    // Esperamos que se llame a `call` una vez y devolvemos un Ok.
    mock_repo.expect_call()
        .times(1)
        .returning(|_| Ok(LlmResponse::new(/* ... */)));

    let use_case = LlmCallUseCase::new(std::sync::Arc::new(mock_repo));
    let result = use_case.execute(/* ... */).await;

    assert!(result.is_ok());
}
```

**Servidor HTTP Mock con `wiremock`**:
Para los tests de los adaptadores de infraestructura, usamos `wiremock` para simular las APIs externas.

```rust
// tests/gemini_adapter_test.rs
use wiremock::{MockServer, Mock, ResponseTemplate};
use wiremock::matchers::{method, path};

#[tokio::test]
async fn test_gemini_adapter_call_success() {
    // 1. Iniciar servidor mock
    let server = MockServer::start().await;

    // 2. Configurar una respuesta mock
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-pro:generateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(/* ... */))
        .mount(&server)
        .await;

    // 3. Crear adaptador apuntando al servidor mock
    let adapter = GeminiAdapter::with_base_url(server.uri());

    // 4. Ejecutar la llamada y verificar el resultado
    let response = adapter.call(/* ... */).await.unwrap();
    assert_eq!(response.content(), "Respuesta mockeada");
}
```

### Test Commands

```bash
# Ejecutar todos los tests (unitarios y de integración)
cargo test

# Ejecutar tests de un módulo específico
cargo test llm::domain::llm_config

# Ejecutar un test de integración específico
cargo test --test cohere_adapter_test

# Tests con output detallado
cargo test -- --nocapture

# Tests con coverage (requiere cargo-tarpaulin)
cargo tarpaulin --all-features --workspace
```

### System Tests (DAGs / JSON Graphs)

El motor DAG (`dag_engine`) utiliza ficheros JSON para definir escenarios de prueba completos. Los grafos se encuentran en `tests/graphs/` organizados por categoría:

| Categoría | Ruta | Contenido |
|-----------|------|-----------|
| **basic** | `tests/graphs/basic/` | Nodos simples: math, log, trigger, loop, suspend |
| **agents** | `tests/graphs/agents/` | llm_call, tool calling, streaming, extraction, planner |
| **advanced** | `tests/graphs/advanced/` | Orchestrators, multi-agent, trip planner |
| **memory** | `tests/graphs/memory/` | Persistencia con SQLite y PostgreSQL |
| **external** | `tests/graphs/external/` | HTTP requests, Amadeus API |
| **media** | `tests/graphs/media/` | Archivo multimedia para tests de visión/documentos |
| **security** | `tests/graphs/security/` | Tests de Secure Values, cifrado y auto-inyección de secretos |

#### Comando para ejecutar un grafo JSON

```bash
# Sintaxis base (modo local con test_payload)
cargo run --bin dag_engine -- run <path/to/graph.json>

# Opciones adicionales
cargo run --bin dag_engine -- run <file> [--session-id <id>] [--answer <text>] [--include-extra-info]

# Modo servidor (producción)
cargo run --bin dag_engine -- serve <path/to/graph.json>
```

#### Ejemplos concretos por categoría

```bash
# Básicos
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json
cargo run --bin dag_engine -- run tests/graphs/basic/power.json

# Agentes con LLM
cargo run --bin dag_engine -- run tests/graphs/agents/llm_call.json
cargo run --bin dag_engine -- run tests/graphs/agents/agent_with_tools.json
cargo run --bin dag_engine -- run tests/graphs/agents/http_tool_dynamic_placeholder_test.json
cargo run --bin dag_engine -- run tests/graphs/agents/extraction_example.json

# Memoria
cargo run --bin dag_engine -- run tests/graphs/memory/memory_sqlite_example.json
cargo run --bin dag_engine -- run tests/graphs/memory/memory_postgres_example.json

# HTTP externo
cargo run --bin dag_engine -- run tests/graphs/external/http_request.json
```

> [!NOTE]
> Los grafos que usan `trigger_webhook` pueden ejecutarse en modo `run` gracias al campo `test_payload` en su configuración, que simula el payload de entrada sin necesidad de un servidor HTTP real.
