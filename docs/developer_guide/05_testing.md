# 🧪 Testing

### Estrategia de Tests

En Colmena, seguimos la estrategia de testing idiomática de Rust:

1.  **Tests Unitarios (`#[cfg(test)]`)**:
    *   **Ubicación**: Se encuentran en un módulo `mod tests { ... }` dentro del mismo fichero que el código que prueban.
    *   **Propósito**: Probar la lógica interna de una función o un módulo de forma aislada. Tienen acceso a funciones y tipos privados.
    *   **Ejemplo**: Testear la lógica de validación de `LlmConfig` sin depender de nada más.

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

    let use_case = LlmCallUseCase::new(Box::new(mock_repo));
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
