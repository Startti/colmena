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
