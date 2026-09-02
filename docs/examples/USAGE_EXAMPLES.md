# Colmena — Catálogo de Ejemplos

Este documento es un índice curado de los grafos de ejemplo que viven bajo
`tests/graphs/`. Todos los grafos listados aquí están verificados y son
ejecutables con el binario `dag_engine`.

Si vas a aprender Colmena leyendo código, este es tu punto de partida: cada
sección apunta a un grafo real y muestra el comando exacto para correrlo.

> **Lenguaje y forma de uso**
> Colmena es una librería Rust. Los grafos son JSON ejecutados por el motor
> `dag_engine`. No hay (todavía) un paquete `pip install colmena` ni un SDK
> Python público; las integraciones se hacen vía el binario, vía la NAPI
> exportada (`index.js`) o vía la librería Rust.

---

## Prerrequisitos

### Toolchain

- Rust (ver `rust-toolchain.toml`).
- Para los grafos con `python_script` / `python_sandbox_tool`, compila con la
  feature `python`.

### Variables de entorno

Cárgalas desde tu `.env` antes de ejecutar:

```sh
set -a; source .env; set +a
```

Las API keys más usadas:

- `OPENAI_API_KEY` — proveedor `openai`.
- `GOOGLE_API_KEY` o `GEMINI_API_KEY` — proveedor `google` (ambos nombres
  mapean al mismo `ProviderKind::Google`).
- `ANTHROPIC_API_KEY` — proveedor `anthropic`.
- `DATABASE_URL` — Postgres, requerido por todos los grafos con memoria,
  secure values, sesiones o `agent_session_id`. Default recomendado para
  tests locales en colmena puro: `colmena_llm_memory`. ADP usa
  `adp_db_develop` (gestionado por Prisma).
- `TAVILY_API_KEY` — para los grafos en `tests/graphs/web/tavily_*`.
- `AMADEUS_CLIENT_ID`, `AMADEUS_CLIENT_SECRET` — grafos `amadeus_*`.

> **Modelo Google por defecto**: usa siempre `gemini-2.5-flash`.
> Los modelos `gemini-1.5-*` están deprecated.

### Comando canónico

```sh
cargo run --bin dag_engine -- run <ruta/al/grafo.json>
```

Flags útiles:

```sh
# Verbose con todos los logs del engine
COLMENA_VERBOSE=1 cargo run --bin dag_engine -- run <grafo.json>

# Continuar una sesión de agente (memoria persistente)
cargo run --bin dag_engine -- run <grafo.json> \
  --agent-session-id agent_demo_001

# Resumir un grafo suspendido pasando la respuesta del usuario
cargo run --bin dag_engine -- run <grafo.json> \
  --agent-session-id agent_demo_001 \
  --answer "París, 3 días"

# Modo servidor (expone los `trigger_webhook` como endpoints HTTP)
cargo run --bin dag_engine -- serve <grafo.json> --port 3000
```

Detalles completos en `CLAUDE.md` (sección *DAG Engine CLI*).

---

## 1. Básicos — motor puro, sin red

Ideal para entender la mecánica de nodos, edges y triggers.

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/basic/trigger.json` | Webhook trigger con `test_payload`. |
| `tests/graphs/basic/input_example.json` | Pasar input y loggear. |
| `tests/graphs/basic/power.json` | Operaciones matemáticas (`exponential`). |
| `tests/graphs/basic/power_webhook.json` | Lo mismo, expuesto por webhook. |
| `tests/graphs/basic/python_simple_graph.json` | `python_script` inline. Requiere feature `python`. |
| `tests/graphs/basic/test_cyclic_graph.json` | Grafo con ciclos. |
| `tests/graphs/basic/test_cyclic_early_stop.json` | Ciclo con parada temprana. |
| `tests/graphs/basic/test_loop.json` / `test_loop_direct.json` | Loops iterativos. |
| `tests/graphs/basic/test_suspend_manual.json` | Nodo `suspend` nativo (HITL). |
| `tests/graphs/basic/suspend_in_subgraph.json` | Suspend dentro de subgrafo. |
| `tests/graphs/basic/secure_suspend_smoke.json` | Secure values + suspend smoke. |
| `tests/graphs/basic/secure_value_in_config_smoke.json` | Secure value usado desde `config`. |

```sh
cargo run --bin dag_engine -- run tests/graphs/basic/power.json
```

## 2. Edge resolution — cómo se conectan los puertos

Tests pedagógicos del sistema de resolución de edges (extracción inteligente,
auto-flatten, puertos default, etc.).

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/edge_resolution/test_case_1_1_implicit_with_defaults.json` | Edge implícita con puertos default. |
| `tests/graphs/edge_resolution/test_case_1_4_fully_explicit.json` | Edge totalmente explícita. |
| `tests/graphs/edge_resolution/test_case_2_2_explicit_required_add.json` | Required fields. |
| `tests/graphs/edge_resolution/test_case_4_1_smart_extraction.json` | Smart field extraction. |
| `tests/graphs/edge_resolution/test_case_4_2_no_field_match.json` | Caso sin match. |
| `tests/graphs/edge_resolution/test_case_5_1_auto_flatten_fallback.json` | Auto-flatten fallback. |
| `tests/graphs/edge_resolution/default_output_ports_named.json` | Puertos de salida nombrados. |
| `tests/graphs/edge_resolution/default_ports_chain.json` | Encadenamiento por puertos default. |
| `tests/graphs/edge_resolution/smart_extraction_complex.json` | Extracción compleja anidada. |

## 3. LLM y agentes

Llamadas LLM, tool calling, streaming, thinking y patrones de extracción.

### Llamada básica y streaming

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/agents/llm_call.json` | Llamada simple a OpenAI. |
| `tests/graphs/agents/llm_local_test.json` | Llamada local con `provider = mock` (sin red). |
| `tests/graphs/agents/llm_chain_futbol.json` | Cadena LLM → LLM. |
| `tests/graphs/agents/llm_stream_dag.json` | Streaming chunks via SSE. |
| `tests/graphs/agents/llm_stream_tool.json` | Streaming con tool calls. |
| `tests/graphs/agents/llm_gemini_stream_tool.json` | Streaming + tools en Google. |
| `tests/graphs/agents/llm_usage_all_providers_test.json` | Compara los 3 providers (usage/tokens). |
| `tests/graphs/agents/llm_temporal_context_test.json` | Inyección de contexto temporal. |
| `tests/graphs/agents/extraction_example.json` | Extracción estructurada. |
| `tests/graphs/examples/llm_chain_birthday.json` | Cadena simple (Gemini). |

### Thinking / razonamiento extendido

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/agents/llm_thinking_anthropic.json` | Extended thinking en Claude. |
| `tests/graphs/agents/llm_thinking_gemini.json` | Thinking budget en Gemini. |
| `tests/graphs/agents/llm_thinking_openai.json` | Reasoning effort en OpenAI. |

### Tools dinámicas

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/agents/agent_with_tools.json` | OpenAI + tool HTTP. |
| `tests/graphs/agents/agent_with_tools_anthropic.json` | Mismo patrón en Anthropic. |
| `tests/graphs/agents/agent_with_tools_gemini.json` | Mismo patrón en Google. |
| `tests/graphs/agents/agent_with_tools_stream.json` | Tools + streaming. |
| `tests/graphs/agents/agent_http_tool_create_post.json` | HTTP tool con POST + body dinámico. |
| `tests/graphs/agents/agent_http_tool_recall.json` | HTTP tool encadenada con memoria. |
| `tests/graphs/agents/http_tool_dynamic_placeholder_test.json` | Placeholders `$DYNAMIC` en config. |
| `tests/graphs/agents/http_tool_node_schema_test.json` | Tool con JSON schema. |
| `tests/graphs/agents/tools_lazy_basic.json` | Lazy tool loading. |
| `tests/graphs/agents/forward_generated_artifact.json` | Forwarding de artefactos generados. ⚠️ **No carga** — ver nota abajo. |

```sh
cargo run --bin dag_engine -- run tests/graphs/agents/agent_with_tools.json
```

### Planner

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/agents/planner_test.json` | Nodo `planner` standalone. |

## 4. Skills

Skills son playbooks Markdown auto-cargables. La forma del config:
`{ "builtin": ["name"], "paths": ["./dir"] }`. También se pueden cargar
desde `llm_call` vía `skills_path` (directorio padre) o `skills_paths`
(lista). El único synthetic tool es `load_skill({ name, reference? })`.
Las referencias dentro de un `SKILL.md` son recursivas (depth 5, con
detección de ciclos).

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/agents/skills_basic.json` | Skills builtin (`python-expert`, `sql-optimizer`). |
| `tests/graphs/agents/roleplay_inventory_skills.json` | Skills cargadas por `paths`, con references. |
| `tests/graphs/skills/inventory_roleplay/` | Tres skills de ejemplo (analyst, writer, monitor) usadas por el grafo anterior. |
| `tests/graphs/advanced/hubspot/agent.json` + `tests/graphs/advanced/hubspot/skills/` | Skill realista (HubSpot CRM) con tools HTTP. |

## 5. Memoria (sesiones de agente)

Todos los grafos de memoria persisten conversación en `DATABASE_URL` por
`agent_session_id`. Usa el flag CLI `--agent-session-id` para encadenar
invocaciones.

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/memory/memory_sqlite_example.json` | Persistencia en SQLite (Gemini). |
| `tests/graphs/memory/memory_postgres_example.json` | Persistencia en Postgres. |
| `tests/graphs/memory/agent_chat_say.json` | Turno "say" (agente escribe). |
| `tests/graphs/memory/agent_chat_ask.json` | Turno "ask" (agente espera respuesta). |
| `tests/graphs/agents/agent_with_tools_postgres.json` | Tools + memoria Postgres (sesión inicial). |
| `tests/graphs/agents/agent_with_tools_postgres_recall.json` | Recall (continuación de sesión). |
| `tests/graphs/advanced/llm_tools_memory_test.json` | Memoria + tools combinadas. |
| `tests/graphs/advanced/llm_tools_memory_continuation.json` | Continuación de la anterior. |

```sh
# Crear sesión
cargo run --bin dag_engine -- run \
  tests/graphs/agents/agent_with_tools_postgres.json \
  --agent-session-id agent_demo_001

# Continuar la misma sesión
cargo run --bin dag_engine -- run \
  tests/graphs/agents/agent_with_tools_postgres_recall.json \
  --agent-session-id agent_demo_001
```

## 6. Documents (HTML)

Se agregaron en PR #79: variant `ArtifactKind::Html`, use cases
`upload_asset` / `list_assets` / `delete_asset`, `AssetStore` port con
implementaciones `LocalFsAssetStore` y `GcsAssetStore`.

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/documents/smoke_create_edit_read.json` | Crear → editar → leer un documento HTML. |
| `tests/graphs/documents/llm_tool_integration.json` | Documento HTML expuesto al LLM como herramienta. |

## 7. Multimedia (image generation / edit / tts)

Nodos: `image_generation`, `image_edit`, `tts`. Para el detalle de modelos
y parámetros por proveedor, ver el doc canónico de configuración del
canvas multimedia en ADP.

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/media/image_generation_basic.json` | Generación de imagen básica. |
| `tests/graphs/media/image_gemini.json` | Generación con Google. |
| `tests/graphs/media/image_anthropic.json` | Generación con Anthropic. |
| `tests/graphs/media/image_edit_basic.json` | Edición de imagen. |
| `tests/graphs/media/image_gen_then_edit.json` | Pipeline generate → edit. |
| `tests/graphs/media/tts_basic.json` | Text-to-speech. |
| `tests/graphs/agents/multimedia_agent.json` | Agente que decide qué tool multimedia usar. |
| `tests/graphs/agents/multimedia_agent_with_load.json` | Multimedia + load_attachment. |

### Visión / PDF

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/media/image_path.json` | Imagen desde archivo local. |
| `tests/graphs/media/image_url_openai.json` / `_anthropic` / `_gemini` | Imágenes vía signed URL. |
| `tests/graphs/media/pdf_path.json` / `pdf_base64.json` | PDF local / base64. |
| `tests/graphs/media/pdf_url_openai.json` / `_anthropic` / `_gemini` | PDF grande vía signed URL. |
| `tests/graphs/media/pdf_anthropic.json` / `pdf_gemini.json` | PDF inline por proveedor. |

> Para regenerar las signed URLs de los grafos `*_url_*`, ver
> `tests/graphs/media/README.md`.

### Adjuntos (load_attachment)

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/agents/load_attachment_basic.json` | Carga de adjunto básica. |
| `tests/graphs/agents/load_attachment_auto_summary.json` | Resumen automático del adjunto. |
| `tests/graphs/agents/load_attachment_inline_summary.json` | Resumen inline. |
| `tests/graphs/agents/load_attachment_opt_out.json` | Opt-out del resumen. |
| `tests/graphs/agents/load_attachment_subgraph.json` | Adjunto dentro de un subgrafo. |
| `tests/graphs/agents/load_attachment_two_agents_step1_upload.json` | Paso 1: upload (agente A). |
| `tests/graphs/agents/load_attachment_two_agents_step2_read.json` | Paso 2: read (agente B, mismo session). |
| `tests/graphs/agents/load_attachment_two_agents_step3_isolated.json` | Paso 3: aislamiento entre sesiones. |
| `tests/graphs/agents/agent_multipart_upload.json` | Upload multipart desde un agente. |
| `tests/graphs/agents/upload_inline_to_endpoint.json` | Upload inline a endpoint. ⚠️ **No carga** — ver nota abajo. |
| `tests/graphs/agents/upload_signed_url_to_endpoint.json` | Upload vía signed URL. ⚠️ **No carga** — ver nota abajo. |


> ⚠️ **Tres de los grafos listados arriba no se pueden ejecutar hoy.**
> `forward_generated_artifact.json`, `upload_inline_to_endpoint.json` y
> `upload_signed_url_to_endpoint.json` declaran `nodes` como un **array**, y el motor
> espera un mapa de `id → nodo`. Ninguno deserializa:
>
> ```
> Error: "… is not a graph: invalid type: sequence, expected a map"
> ```
>
> Se documentan aquí como ejemplos pendientes de reparación, no como ejemplos
> funcionales. `cargo run --bin dag_engine -- lint <archivo>` reporta el problema sin
> ejecutar nada. Anotado en [`docs/BACKLOG.md`](../BACKLOG.md).

## 8. SQL (tool de base de datos)

Tool de SQL con permisos finos sobre Postgres.

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/agents/sql_provision_schema_tool.json` | Provisión de schema. |
| `tests/graphs/agents/sql_create_schema_blocked.json` | Bloqueo de operaciones DDL no permitidas. |
| `tests/graphs/agents/sql_query_readonly_test.json` | Query read-only. |
| `tests/graphs/agents/sql_insert_decimal_regression.json` | Regression test (insert decimal). |
| `tests/graphs/agents/sql_rls_todo_test.json` | RLS / row-level security. |

## 9. Python (sandbox)

Requieren build con la feature `python`.

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/basic/python_simple_graph.json` | `python_script` inline. |
| `tests/graphs/agents/python_llm_graph.json` | Python + LLM. |
| `tests/graphs/agents/python_sandbox_tool_test.json` | Tool `python_sandbox` desde un agente. |
| `tests/graphs/agents/python_sandbox_tool_thinking_test.json` | Sandbox + thinking. |

## 10. HTTP / Web / APIs externas

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/external/http_request.json` | GET simple a API pública. |
| `tests/graphs/external/dynamic_http.json` | Endpoint resuelto en runtime. |
| `tests/graphs/external/http_headers_dynamic.json` | Headers dinámicos. |
| `tests/graphs/external/http_tool_configured.json` | HTTP como tool del agente. |
| `tests/graphs/external/multipart_upload.json` | Upload multipart. |
| `tests/graphs/external/multipart_gen_chain.json` | Generación + multipart encadenados. |

### Tavily (búsqueda web)

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/web/tavily_search_basic.json` | Search básico. |
| `tests/graphs/web/tavily_direct_search.json` | Search sin LLM. |
| `tests/graphs/web/tavily_fetch_article.json` | Fetch de artículo completo. |
| `tests/graphs/web/tavily_llm_openai.json` / `_gemini` / `_anthropic` | Tavily como tool de un LLM. |

### API explorer (OpenAPI)

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/web/api_explorer_petstore.json` | Petstore (Swagger 2). |
| `tests/graphs/web/api_explorer_petstore_flag_only.json` | Mismo, flag-only. |
| `tests/graphs/web/api_explorer_amadeus_swagger2.json` | Amadeus Swagger 2. |
| `tests/graphs/web/api_explorer_hubspot_conversation.json` | HubSpot. |

### Amadeus (flight search)

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/external/amadeus_flight_search_dynamic.json` | Búsqueda de vuelos. |
| `tests/graphs/external/debug_amadeus_token_only.json` | Sólo obtener token. |
| `tests/graphs/external/debug_amadeus_auth_flight.json` | Auth + búsqueda. |
| `tests/graphs/external/debug_amadeus_flight_no_llm.json` | Búsqueda sin LLM. |
| `tests/graphs/advanced/travel_agent_amadeus.json` | Agente completo. |

### Otros (ventas, canvas, socketio)

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/external/product_sales_assistant.json` | Asistente de ventas. |
| `tests/graphs/external/product_sales_assistant_cards.json` | Versión con cards. |
| `tests/graphs/external/product_sales_dummyjson.json` | Sobre dummyjson. |
| `tests/graphs/external/adp_canvas_load_test.json` | Carga grafo desde ADP. |
| `tests/graphs/external/canvas_builder_autonomous.json` | Canvas builder autónomo. |
| `tests/graphs/external/canvas_builder_controlled.json` | Canvas builder controlado. |
| `tests/graphs/external/socketio_canvas_builder.json` | Vía Socket.IO. |
| `tests/graphs/external/socketio_canvas_test.json` | Smoke Socket.IO. |
| `tests/graphs/external/socketio_pre_events.json` | Pre-events Socket.IO. |
| `tests/graphs/external/socketio_debug_test.json` | Debug Socket.IO. |

## 11. Secure values

Mecanismo para hashear secretos (`<value_1>`, …) que sólo se rehidratan
durante la inyección a nodos no-LLM. TTL del mapping: **24 h** (cambio
2026-05-11). Ver `tests/graphs/security/README.md` para el detalle de
cada test.

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/security/http_secure_basic.json` | HTTP node con `secure: true`. |
| `tests/graphs/security/http_secure_debug.json` | Variante con logs extra. |
| `tests/graphs/security/http_secure_to_http_inject.json` | Inyección de token entre nodos HTTP. |
| `tests/graphs/security/http_secure_to_llm_demo.json` | Secure HTTP → LLM (LLM ve hashes). |
| `tests/graphs/security/http_secure_to_llm_test.json` | Test de la integración anterior. |
| `tests/graphs/security/amadeus_secure_simple_test.json` | Amadeus + secure. |
| `tests/graphs/security/amadeus_secure_gemini_test.json` | Amadeus secure + Gemini. |
| `tests/graphs/security/amadeus_secure_gemini_agent_test.json` | Amadeus secure dentro de agente. |
| `tests/graphs/advanced/secure_python_echo_masking_e2e.json` | E2E: masking en `python_script`. |
| `tests/graphs/advanced/secure_suspend_login_direct.json` | Login flow con suspend (directo). |
| `tests/graphs/advanced/secure_suspend_login_e2e.json` | Login flow E2E. |

## 12. Suspend / HITL / loops

Pausa el grafo, espera input humano, continúa.

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/basic/test_suspend_manual.json` | Suspend nativo. |
| `tests/graphs/basic/suspend_in_subgraph.json` | Suspend dentro de subgrafo. |
| `tests/graphs/advanced/test_suspend.json` | Variante con LLM. |
| `tests/graphs/advanced/llm_tool_suspend_smoke.json` | Tool que dispara suspend. |
| `tests/graphs/advanced/llm_tool_suspend_flag_smoke.json` | Tool suspend gated por flag. |
| `tests/graphs/advanced/hitl_allow_suspend_false_test.json` | `allow_suspend = false`. |
| `tests/graphs/advanced/hitl_planner_suspend_test.json` | Planner que suspende. |
| `tests/graphs/advanced/hitl_critic_answer_rerun_test.json` | Critic + re-run con respuesta. |
| `tests/graphs/advanced/hitl_critic_max_retries_test.json` | Critic con max retries. |
| `tests/graphs/basic/test_loop.json` / `test_loop_direct.json` | Loops básicos. |

```sh
# Lanzar (se suspenderá esperando respuesta)
cargo run --bin dag_engine -- run \
  tests/graphs/advanced/llm_tool_suspend_smoke.json \
  --agent-session-id hitl_demo_001

# Resumir pasando la respuesta del humano
cargo run --bin dag_engine -- run \
  tests/graphs/advanced/llm_tool_suspend_smoke.json \
  --agent-session-id hitl_demo_001 \
  --answer "OK, procede"
```

## 13. Orquestadores (planner / critic / phase reactor)

El orquestador acepta config **anidada**:
`{ planner: {...}, critic: {...}, phase_reactor: {...}, final_reactor: {...} }`
(NO una config plana). Las cabeceras de prompts del critic se emiten en
inglés (e.g. `=== PREVIOUS ATTEMPT — WHY IT FAILED ===`).

### Canónico

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/advanced/trip_planner_v2.json` | **Ejemplo canónico** del orquestador (config anidada completa). |

### Variantes

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/advanced/test_orchestrator.json` | Orquestador básico. |
| `tests/graphs/advanced/trip_planner.json` | Versión inicial del trip planner. |
| `tests/graphs/advanced/trip_assistant.json` | Asistente con orquestador. |
| `tests/graphs/advanced/trip_planner_replanning_test.json` | Replanificación. |
| `tests/graphs/advanced/bridge_tasks_test.json` | Bridge tasks entre fases. |
| `tests/graphs/advanced/final_reactor_text_delta_test.json` | Final reactor emitiendo deltas. |

### Critic feedback

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/advanced/critic_feedback_cleanup_test.json` | Cleanup de feedback. |
| `tests/graphs/advanced/critic_feedback_injection_test.json` | Inyección de feedback al planner. |
| `tests/graphs/advanced/critic_feedback_multiretry_test.json` | Multi-retry. |
| `tests/graphs/advanced/critic_feedback_with_suspend_test.json` | Critic combinado con suspend. |

### Orquestadores por proveedor

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/advanced/travel_orchestrator_openai.json` | Variante OpenAI. |
| `tests/graphs/advanced/travel_orchestrator_anthropic.json` | Variante Anthropic. |
| `tests/graphs/advanced/travel_orchestrator_anthropic_suspend_test.json` | Anthropic + suspend. |
| `tests/graphs/advanced/travel_orchestrator_gemini.json` | Variante Gemini. |
| `tests/graphs/advanced/travel_orchestrator_gemini_suspend_test.json` | Gemini + suspend. |
| `tests/graphs/advanced/travel_orchestrator_gemini_pro_suspend_test.json` | Gemini Pro + suspend. |

## 14. Sub-grafos / agentes anidados

| Grafo | Qué muestra |
|-------|-------------|
| `tests/graphs/advanced/nested_orchestrators.json` | Orquestador dentro de orquestador. |
| `tests/graphs/advanced/nested_orchestrators_suspend.json` | Anidados + suspend. |
| `tests/graphs/advanced/nested_orchestrators_with_tools.json` | Anidados con tools. |
| `tests/graphs/advanced/nested_agents/weather_manager.json` | Manager (carga al child). |
| `tests/graphs/advanced/nested_agents/weather_child_agent.json` | Child invocado. |

---

## Ejecutar todo / auditoría

El archivo `tests/graphs/AUDIT_RESULTS.md` contiene la última auditoría de
ejecución de todos los grafos del directorio (estado, notas, regresiones).
Útil cuando un grafo "no corre" — primero revisar si ya está documentado.

## Cómo añadir un nuevo ejemplo

1. Coloca el JSON bajo la subcarpeta de `tests/graphs/` que mejor encaje
   por categoría (no inventes nuevas categorías sin necesidad).
2. Verifica que arranca con `cargo run --bin dag_engine -- run <ruta>`.
3. Si depende de un servicio externo, documenta cualquier setup adicional
   en un `README.md` local a la carpeta (ver
   `tests/graphs/media/README.md` y `tests/graphs/security/README.md` como
   referencia).
4. Añade una fila en la tabla de este documento, en la sección
   correspondiente.

## Referencias

- `CLAUDE.md` — comandos canónicos del binario, flags de CLI.
- `docs/developer_guide/12_dag_engine_guide.md` — guía del motor.
- `tests/graphs/AUDIT_RESULTS.md` — última auditoría de ejecución.
- `src/libs/colmena/src/skills/domain/skill_config.rs` — esquema real del
  config de skills.
