# 🚀 Performance y Optimización

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

> **Nota**: este ejemplo es ilustrativo, no ejecutable tal cual. El repo no tiene un directorio `benches/` ni `criterion` como dependencia/target `[[bench]]` en `Cargo.toml` — habría que añadirlos primero. `LlmConfig::new` tampoco toma builders `with_model`/`with_api_key`; su firma real es `LlmConfig::new(provider: LlmProvider)` (`src/libs/colmena/src/llm/domain/llm_config.rs:72`).

```rust
// benches/llm_benchmarks.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use colmena::llm::domain::*;

fn benchmark_request_creation(c: &mut Criterion) {
    c.bench_function("create_llm_request", |b| {
        b.iter(|| {
            let config = LlmConfig::new(black_box(LlmProvider::OpenAi));

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

> **Nota**: patrón de optimización *propuesto*, no el estado actual del código. Hoy `OpenAiAdapter::new()` crea un `Client::new()` nuevo por instancia, sin cliente estático compartido ni tuning de pool (`src/libs/colmena/src/llm/infrastructure/openai_adapter.rs:24-29`), y `lazy_static` no es una dependencia del crate.

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
### Rendimiento del Motor DAG

El motor de grafos está diseñado para la confiabilidad y la trazabilidad, lo que introduce ciertas consideraciones de rendimiento:

**1. Ejecución de Nodos:**
Actualmente, el `DagRunUseCase` ejecuta los nodos de forma **secuencial** siguiendo la cola de activación (`active_queue`).
- **Optimización**: Los nodos se activan por eventos; un nodo se encola tan pronto como sus dependencias upstream han emitido datos.
- **Overhead**: La resolución de entradas dinámica mediante JSON Pointers tiene un coste $O(1)$ por arista, minimizado por el uso de `serde_json::Value`.

**2. Impacto de Secure Values:**
El uso de `secure: true` en la configuración de un nodo añade dos pasos adicionales:
- **Pre-ejecución**: Inyección de secretos desde PostgreSQL (latencia de DB) — único backend implementado hoy (`PostgresSecureValueRepository`); "Vault" solo aparece en un comentario como caso de uso futuro hipotético (`postgres_secure_value_repository.rs:49`).
- **Post-ejecución**: Cifrado vía `pgp_sym_encrypt` de pgcrypto (cifrado simétrico PGP, sin override de cipher-algo → modo por defecto de pgcrypto, no AES-256-GCM) y hashing del resultado antes de guardarlo en memoria o enviarlo al stream (`postgres_secure_value_repository.rs:87`).
- **Recomendación**: Usa `secure: true` solo en campos estrictamente sensibles para evitar el coste de cifrado en datos públicos.

**3. Persistencia de Estado:**
Si se utiliza un `state_repository` (SQLite/Postgres), el motor guardará el estado completo del grafo en cada paso de suspensión (`SUSPENDED`) o finalización.
- El tamaño del estado crece linealmente con el número de nodos y el tamaño de sus outputs.
- Para grafos masivos, considera usar `strip_extra_info: true` para reducir el payload guardado.

### Mejores Prácticas de Rendimiento

1.  **Pre-allocación**: En nodos personalizados de Rust, pre-alloca buffers si conoces el tamaño aproximado de la respuesta (ej. `String::with_capacity`).
2.  **Streaming**: Prefiere `execute_stream` sobre `execute` para procesar resultados parciales y reducir la percepción de latencia del usuario final.
3.  **Lazy Client**: Reutiliza el `reqwest::Client` entre nodos mediante un contenedor de servicios compartido para aprovechar el pooling de conexiones TCP.
