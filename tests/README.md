# Colmena Tests

Este directorio contiene la suite de pruebas de integración, ejemplos de DAGs y scripts de verificación para Colmena.

## Estructura de Directorios

- **`rust/`**: Tests de integración escritos en Rust.
- **`python/`**: Scripts de verificación y ejemplos de cliente en Python.
- **`dags/`**: Definiciones de grafos (DAGs) en formato JSON.

---

## 🏗️ DAGs de Ejemplo (`tests/dags/`)

A continuación se listan todos los DAGs disponibles para probar diferentes funcionalidades del motor.

### Cómo ejecutar un DAG
Para todos los DAGs listados con "Webhook Path", se pueden ejecutar con:

```bash
# 1. Iniciar el servidor
cargo run --bin dag_engine -- serve tests/dags/<archivo>.json

# 2. Enviar el payload (usando curl o Postman)
curl -X POST -H "Content-Type: application/json" -d '<payload>' http://localhost:3000<path>
```

### 📡 Activación de Streaming
Para activar el streaming (SSE) en **cualquier DAG**, simplemente agrega los siguientes headers a tu petición:

- `Accept: text/event-stream`
- `x-vercel-ai-ui-message-stream: v1`

Ejemplo genérico con curl:

```bash
curl -N -X POST \
  -H "Content-Type: application/json" \
  -H "Accept: text/event-stream" \
  -H "x-vercel-ai-ui-message-stream: v1" \
  -d '<payload>' \
  http://localhost:3000<path>
```

### Tabla de Referencia

| Archivo | Descripción | Path (WebHook) | Payload de Prueba |
|---|---|---|---|
| **Agentes ReAct (OpenAI)** | | | |
| `agent_with_tools.json` | Asistente matemático (Sin Streaming). | `/agent-demo` | `{"question": "Calculate 25 + 17, then multiply the result by 2"}` |
| `agent_with_tools_stream.json` | Asistente matemático (**Con Streaming Enabled**). | `/agent-demo-stream` | `{"question": "Calculate 25 + 17, then multiply the result by 2"}` |
| `agent_with_tools_postgres.json` | Agente con memoria persistente (PostgreSQL). | `/agent-memory-demo` | `{"question": "What is 50 + 25? Then multiply by 3."}` |
| `agent_with_tools_postgres_recall.json` | Prueba de recuperación de memoria (Contexto previo). | `/agent-memory-demo-2` | `{"question": "What was the result of my previous calculation?"}` |
| `http_tool_configured.json` | Agente interactuando con API REST externa (JSONPlaceholder). | `/agent` | `{"request": "Get all active users"}` |
| | | | |
| **Agente de Viajes (Amadeus Integration)** | | | |
| `travel_agent_amadeus.json` | Agente completo. Autenticación + Búsqueda de Vuelos/Hoteles. | `/travel-agent` | `{"question": "I want to go from bogota to miami..."}` |
| `debug_amadeus_auth_flight.json` | Versión simplificada para debug de auth + búsqueda. | `/debug-travel` | `{"question": "Search for flights to Paris"}` |
| `debug_amadeus_flight_no_llm.json` | Llamada directa a APIs de Amadeus (sin LLM) para probar conectividad. | `/debug-flight-direct` | `{}` |
| `debug_amadeus_token_only.json` | Solo prueba de obtención de token OAuth2. | `/debug-token` | `{}` |
| | | | |
| **LLM & Streaming** | | | |
| `llm_stream_tool.json` | **Principal (Verificación)**. Streaming de LLM + Herramienta (Mock Weather). | `/execute` | `{"prompt": "What is the weather in Paris?"}` |
| `llm_call.json` | Llamada simple a LLM con Streaming activado. | `/test-llm` | `{"message": "Say hello in Spanish!"}` |
| `llm_local_test.json` | Llamada simple a LLM (sin streaming). | `/test-llm` | `{"message": "Say hello in Spanish!"}` |
| | | | |
| **Integración HTTP & Webhooks** | | | |
| `trigger.json` | Webhook simple (Echo). | `/hello` | `{"request": {"message": "Hello!"}}` |
| `dynamic_http.json` | Webhook que define dinámicamente el endpoint a llamar. | `/dynamic-test` | `{"endpoint": "/random_joke"}` |
| `http_request.json` | Llamada HTTP estática simple. | `/test-http` | `{}` |
| `power_webhook.json` | Webhook + Cálculo Exponencial. | `/power` | `{"base_num": 5}` |
| | | | |
| **Pruebas Internas / Sin Webhook** | *Estos DAGs usan `mock_input` o no tienen trigger directo.* | N/A | N/A |
| `power.json` | Cálculo exponencial simple. | N/A | N/A |
| `memory_postgres_example.json` | Test secuencial de memoria Postgres (sin trigger). | N/A | N/A |
| `memory_sqlite_example.json` | Test secuencial de memoria SQLite (sin trigger). | N/A | N/A |
| `python_llm_graph.json` | Generación y ejecución de código Python. | N/A | N/A |
| `python_simple_graph.json` | Ejecución simple de script Python. | N/A | N/A |
| `llm_stream_dag.json` | Test interno de streaming con proveedor Mock. | `/dag/execute` | N/A |

---

## 🦀 Tests de Rust (`tests/rust/`)

Estos tests verifican la lógica interna del motor y los adaptadores.

**Ejecución:**
```bash
cargo test
# O para correr uno específico:
cargo test --test agent_with_tools
```

| Archivo | Descripción |
|---|---|
| `agent_with_tools.rs` | Verifica un ciclo completo ReAct con herramientas mockeadas. |
| `openai_tool_test.rs` | Verifica específicamente el adaptador de OpenAI y la serialización de tools. |
| `gemini_tool_test.rs` | Verifica el adaptador de Google Gemini. |

---

## 🐍 Scripts de Python (`tests/python/`)

Utilidades para verificar el comportamiento del servidor externamente.

**Ejecución:**
```bash
.venv/bin/python tests/python/<script>.py
```

| Archivo | Descripción |
|---|---|
| `verify_tool_stream.py` | Verifica el protocolo de streaming SSE (Tool Calls + Usage + Text). Úsese con `llm_stream_tool.json`. |
| `verify_sse.py` | Cliente genérico de SSE para probar cualquier endpoint que sopporte streaming. |
| `run_server_python.py` | Ejemplo de cómo iniciar el servidor Colmena desde código Python (bindings). |
| `test_server_binding.py` | Test automatizado para verificar que los bindings de Python respetan host/puerto. |
