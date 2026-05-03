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

---

## 🦀 Toolchain de Rust — local vs CI

La versión de Rust está pineada en [`rust-toolchain.toml`](../../rust-toolchain.toml) en la raíz:

```toml
[toolchain]
channel = "1.95.0"
components = ["rustfmt", "clippy"]
```

**Cómo funciona:**
- Cuando ejecutas `cargo` o `rustc`, `rustup` detecta el toml y usa esa versión exacta. Si no la tienes instalada, la auto-instala.
- En CI, los workflows (`ci-develop.yml`, `ci-staging.yml`, `cd-main.yml`) usan `actions-rust-lang/setup-rust-toolchain@v1`, que **lee el toml automáticamente** — local y CI siempre están alineados.

**Para subir la versión de Rust:**
1. Edita `rust-toolchain.toml` → `channel = "1.96.0"` (o lo que sea)
2. Ejecuta `cargo clippy -- -D warnings` local — arregla los lints nuevos si los hay
3. PR. CI tomará la versión nueva del toml automáticamente. **No hay que tocar los workflows.**

**Verificación en CI:** los workflows tienen un step `Show Rust version` que imprime `rustc --version` antes de correr clippy/tests. Útil para confirmar de un vistazo qué versión se está usando.

---

## 🧪 Comandos de test: local vs CI

CI ejecuta `cargo test --verbose`, que corre **todo**: unit tests + integration tests + doctests. Es importante reproducir esto en local antes de hacer push.

| Comando | Qué incluye | Cuándo usarlo |
|---------|------------|---------------|
| `cargo test --lib` | Solo unit tests (`#[cfg(test)]` inline) | Iteración rápida durante desarrollo |
| `cargo test --doc` | Solo doctests (ejemplos en `///` comments) | Cuando tocas docs en items públicos |
| `cargo test --test <nombre>` | Un binario de integration test específico | Cuando trabajas en un test concreto en `tests/` |
| **`cargo test --verbose`** | **Todo (igual que CI)** | **Antes de push o de abrir PR** |

> [!IMPORTANT]
> Si `cargo test --lib` pasa pero `cargo test --verbose` falla, es muy probable que sea un **doctest** desactualizado. Los doctests se compilan contra la API pública, así que cualquier cambio de campo en una struct pública puede romperlos sin que lo notes en los unit tests.

---

## 🚫 Tests con `#[ignore]` — integración con servicios reales

Los tests que requieren conectividad real (Postgres, APIs externas) están marcados con `#[ignore]` para que **no corran en CI por default**, pero sí se puedan correr en local cuando tengas el ambiente listo.

### Inventario actual

| Categoría | Tests | Variable requerida | Por qué |
|-----------|-------|--------------------|---------|
| `agent_session_id_lifecycle` | 3 | `DATABASE_URL` (Postgres) | Persistencia de runs en `dag_runs` |
| `orchestrator_agent_suspend` | 3 | `DATABASE_URL` | Suspend/resume de orchestrators con HITL |
| `find_resume_entry` | 2 | `DATABASE_URL` | Búsqueda de entry point para resume |
| `engine_pool_sharing` | 2 | `TEST_DATABASE_URL` | Verificación del `PgPoolRegistry` |
| `postgres_file_cache` (lib) | 5 | `TEST_DATABASE_URL` | Cache persistido de archivos del LLM |
| `tavily_live` | 4 | `TAVILY_API_KEY` | Llamadas reales al search API |

**Total: 19 tests `#[ignore]`.**

### Cómo correrlos en local

Asegúrate de tener `.env` con las variables necesarias (`DATABASE_URL`, `TAVILY_API_KEY`, etc.) y luego:

```bash
source .env

# Correr SOLO los ignorados (los normales no corren)
cargo test -- --ignored

# Correr ignorados + normales (todo el suite real)
cargo test -- --include-ignored

# Correr ignorados de un binario específico
cargo test --test orchestrator_agent_suspend -- --ignored
```

### Cómo agregar un test que requiere ambiente real

Marca el test con `#[ignore]` con un mensaje explicando qué requiere:

```rust
#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn my_db_test() {
    let url = std::env::var("DATABASE_URL").unwrap();
    // ...
}
```

> [!WARNING]
> **NO uses** `std::env::var("X").unwrap()` o `.expect(...)` sin `#[ignore]`. Si la var no está set, el test panica en CI y rompe el build. La regla es: **si tu test lee una env var requerida, debe llevar `#[ignore]`**.

---

## 🔐 Variables de entorno

### Variables que solo viven en `.env` local (no en CI)

Estas variables están en el `.env` del repo (gitignoreado, no llega al runner) y se usan para tests/scripts que **NO corren en CI**:

| Variable | Usada por | Tests afectados |
|----------|-----------|-----------------|
| `DATABASE_URL` | Engine + tests de persistencia | 8 tests `#[ignore]` |
| `TEST_DATABASE_URL` | Tests de `PgPoolRegistry` y `postgres_file_cache` | 7 tests `#[ignore]` |
| `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY` | `ConfigResolver::resolve_api_key` (devuelve `Err` si falta, no panica) | Solo grafos JSON manuales |
| `TAVILY_API_KEY` | `tavily_client` node + tests live | 4 tests `#[ignore]` |
| `AMADEUS_CLIENT_ID`, `AMADEUS_CLIENT_SECRET` | Solo grafos JSON en `tests/graphs/external/` | Ninguno (manual) |
| `ADP_*`, `DATABASE_URL_GRAPHS` | No referenciadas en código Rust | Ninguno |

### Verificar que tu cambio no rompe CI por env vars

Antes de hacer push, simula el ambiente de CI moviendo `.env` aside:

```bash
mv .env .env.bak
env -u DATABASE_URL cargo test --verbose 2>&1 | grep -E "test result:|FAILED"
mv .env.bak .env
```

Todos los `test result:` deben decir `ok`. Si ves algún `FAILED`, hay un test que lee una env var sin `#[ignore]` y va a romper CI.

---

## 🐍 Tests de Python

Los tests de Python están en `python/tests/` y se ejecutan con:

```bash
.venv/bin/pytest python/ -v
```

CI los corre en el matrix de versiones Python 3.8 → 3.12 después de los tests de Rust. Ver [`06_estructura_testing_python.md`](./06_estructura_testing_python.md) para más detalle.
