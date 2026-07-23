# 00. Visión General de la Arquitectura

> Documento de entrada para nuevos colaboradores. Ofrece el mapa del sistema y dirige a guías especializadas para cada área. Para principios de diseño en detalle, ver [`01_architecture.md`](./01_architecture.md).

---

## ¿Qué es Colmena?

Colmena es una librería Rust-nativa de orquestación de agentes AI. Su núcleo es un **motor de ejecución de grafos DAG** (grafos dirigidos acíclicos) que corre secuencias de nodos —llamadas LLM, consultas SQL, scripts Python, requests HTTP, y más de 25 tipos adicionales— de forma asíncrona sobre Tokio. Soporta tres proveedores LLM (OpenAI, Anthropic, Gemini) a través de una abstracción unificada. Expone bindings nativos para Python (PyO3) y Node.js (napi-rs) y puede consumirse directamente como CLI o servidor HTTP.

La arquitectura sigue el patrón **Hexagonal (Ports & Adapters)**: el dominio no depende de infraestructura, y cada integración externa se encapsula detrás de un trait.

- Crate: `colmena_dag_engine` v0.4.0 (ver `src/libs/colmena/Cargo.toml` línea 3)
- Módulo Python: `colmena` (ver `pyproject.toml` → `tool.maturin.module-name`)
- Repositorio: https://github.com/Startti/colmena

---

## Diagrama maestro

```
┌──────────────────────────────────────────────────────────────────────────┐
│                     PUNTOS DE ENTRADA / CONSUMIDORES                      │
│                                                                            │
│  CLI binary          HTTP binary       Python (PyO3)      Node.js (napi)  │
│  dag_engine run      dag_engine serve  import colmena      require(...)    │
│  dag_engine/main.rs  dag_engine/api.rs python_bindings/    node_bindings/  │
│  (attachment_gc      — standalone GC binary, attachment_gc/main.rs)        │
└──────────┬───────────────────┬─────────────────┬──────────────────────────┘
           │                   │                 │
           ▼                   ▼                 ▼
┌──────────────────────────────────────────────────────────────────────────┐
│                         ColmenaEngine / DagRunUseCase                     │
│                (dag_engine/engine.rs + application/run_use_case.rs)       │
│                                                                            │
│  graph.json ──parse──► Graph { nodes, edges }                             │
│                         │                                                  │
│                         ▼                                                  │
│  execute_stream()  ── traverses nodes in dependency order ─────────────► │
│                         │         ▲                                        │
│                         │         │ outputs flow back via all_outputs map │
│                         ▼         │                                        │
│                   ┌─────────────────────────────────────────────────────┐ │
│                   │        HashMapNodeRegistry (infrastructure)          │ │
│                   │  25+ node types, each implements ExecutableNode      │ │
│                   │  infrastructure/nodes/  +  infrastructure/registry   │ │
│                   └─────────────────────────────────────────────────────┘ │
│                                                                            │
│  Observer events ──────────────────────────────────────────► SSE stream  │
│  (NodeEvent enum, domain/observer.rs)                                     │
└──────────────────────────────────────────────────────────────────────────┘
           │
           ▼
┌──────────────────────────────────────────────────────────────────────────┐
│                 MÓDULOS HORIZONTALES (hexagonal layers)                   │
│                                                                            │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────────────┐  │
│  │      llm/        │  │   dag_engine/   │  │       shared/           │  │
│  │  domain/         │  │  domain/        │  │  infrastructure/        │  │
│  │  application/    │  │  application/   │  │  config_resolver.rs     │  │
│  │  infrastructure/ │  │  infrastructure/│  │  service_container.rs   │  │
│  │  OpenAI · Gemini │  │  nodes/ (25+)   │  └─────────────────────────┘  │
│  │  Anthropic       │  │  registry.rs    │                                │
│  └─────────────────┘  └─────────────────┘                                │
│                                                                            │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────────────┐  │
│  │    skills/       │  │    storage/     │  │       documents/        │  │
│  │  Built-in +      │  │  OutputStorage  │  │  Word/Excel artifacts   │  │
│  │  filesystem      │  │  Port + adapters│  │  DocumentRuntime IR     │  │
│  └─────────────────┘  └─────────────────┘  └─────────────────────────┘  │
│                                                                            │
│  ┌─────────────────┐                                                      │
│  │      web/        │                                                      │
│  │  tavily_client   │                                                      │
│  │  api_explorer    │                                                      │
│  └─────────────────┘                                                      │
└──────────────────────────────────────────────────────────────────────────┘
```

El diagrama muestra el flujo principal: los consumidores delegan en `ColmenaEngine`, que parsea el JSON en un `Graph`, ejecuta los nodos en orden topológico vía `execute_stream()`, y publica eventos `NodeEvent` al observer (SSE hacia afuera).

---

## Tabla de módulos

| Módulo | Responsabilidad | Guía de referencia |
|--------|----------------|-------------------|
| `dag_engine/` | Motor de ejecución de grafos DAG. Dominio: `Graph`, `ExecutableNode`, `NodeEvent`. Aplicación: `DagRunUseCase::execute_stream`. Infra: 25+ implementaciones de nodos, `HashMapNodeRegistry`, servidor HTTP Axum. | [12_dag_engine_guide.md](./12_dag_engine_guide.md) |
| `llm/` | Abstracción multi-proveedor de LLM (OpenAI, Anthropic, Gemini). Dominio: `LlmRepository` trait, `LlmRequest/Response`, historial de conversaciones. Aplicación: `AgentService` (loop ReAct), `LlmCallUseCase`, `LlmStreamUseCase`. Infra: adapters por proveedor, fábrica de providers, persistencia SQLite/Postgres. | [14_llm_deep_dive.md](./14_llm_deep_dive.md) |
| `skills/` | Paquetes de conocimiento Markdown cargados bajo demanda por el nodo LLM vía el tool sintético `load_skill`. Dominio: `SkillRepository` trait, `SkillCatalogEntry`. Infra: `BuiltinSkillRepository` (compilada con `include_dir!`) y `FilesystemSkillRepository` (paths del operador). | [24_skills.md](./24_skills.md) |
| `storage/` | Persistencia de artefactos generados (imágenes, audio). Dominio: `OutputStorageRepository` trait. Infra: `LocalCacheStorageAdapter` (tests/CLI), `LocalHttpStorageAdapter` (dev), `HttpCallbackStorageAdapter` (producción). | [32_multimedia_generation.md](./32_multimedia_generation.md) |
| `documents/` | Generación y edición de documentos Word/Excel. Dominio: IR JSON como source of truth, patches atómicos, versionado. Aplicación: `DocumentRuntime`. Infra: renderizado con `rust_xlsxwriter`/`docx-rs`, storage local o GCS. | [27_documents_library.md](./27_documents_library.md) |
| `web/` | Nodos toolkit web: `tavily_client` (búsqueda), `api_explorer` (OpenAPI), `browser` (próximamente). Dominio: `SessionRegistry`, errores. Infra: expansión de sub-tools y despacho `__sub_tool`. | [25_web_nodes.md](./25_web_nodes.md) |
| `shared/` | Utilidades de proceso: `ConfigResolver` (carga `.env`, construye `LlmConfig`), `ServiceContainerFactory` (inyecta dependencias por proveedor). No tiene capas domain/application; solo `infrastructure/`. | — |
| `python_bindings/` | Bindings PyO3: `ColmenaLlm` (call/stream/health_check), `run_dag`, `serve_dag`, `validate_graph`, `default_registry`. Python package `colmena` compilado con `maturin`. | [docs/examples/python_usage.md](../examples/python_usage.md) |
| `node_bindings/` | Bindings napi-rs: `ColmenaLlm` (call/health_check), `run_dag` async, `serve_dag`. Compilado con `npm run build --features node`. | — |
| `attachment_gc/` | Binario standalone de garbage collection para artefactos TTL-expirados en `conversation_attachments`. Diseñado para Cloud Scheduler → Cloud Run Job. | [36_attachment_gc.md](./36_attachment_gc.md) |

---

## Puntos de entrada

### CLI — ejecutar un grafo

```bash
# Cargar API keys
source .env

# Correr un grafo (termina cuando el grafo completa)
cargo run --bin dag_engine -- run tests/graphs/agents/llm_call.json

# Con sesión estable para flujos con estado (suspend/resume, memoria)
cargo run --bin dag_engine -- run graph.json --agent-session-id demo_001

# Levantar como servidor HTTP (SSE en POST /run, eventos en GET /events)
cargo run --bin dag_engine -- serve tests/graphs/agents/llm_call.json
```

Los binarios están declarados en `src/libs/colmena/Cargo.toml` líneas 12-26: `colmena` (main.rs), `dag_engine` (dag_engine/main.rs), `attachment_gc` (attachment_gc/main.rs), `colmena_oauth_setup` (src/bin/colmena_oauth_setup.rs).

### Python

```python
import colmena  # módulo compilado con maturin develop

llm = colmena.ColmenaLlm()
response = llm.call([{"role": "user", "content": "Hola"}], "openai")

# Ejecutar un grafo completo
result = colmena.run_dag("tests/graphs/basic/trigger.json")
```

El módulo Python se llama `colmena` (`pyproject.toml` → `tool.maturin.module-name = "colmena"`). Los bindings están en `src/libs/colmena/src/python_bindings/mod.rs`.

### Node.js

```typescript
// Compilar primero: npm run build  (requiere feature flag "node")
const { ColmenaLlm, runDag } = require('./index.node');

const llm = new ColmenaLlm();
const result = await llm.call([{ role: 'user', content: 'Hello' }], 'openai');
```

Bindings en `src/libs/colmena/src/node_bindings/mod.rs`.

---

## Ciclo de vida de una ejecución

```
graph.json
    │
    ▼
serde_json::from_str  →  Graph { nodes: HashMap<id, NodeConfig>, edges: Vec<Edge> }
    │                    (dag_engine/domain/graph.rs)
    ▼
graph.validate()  →  rechaza node_ids con '/' (reservado para subgrafos)
    │
    ▼
DagRunUseCase::execute_stream(graph, resume_id, answer, ...)
    │
    │  VecDeque con todos los node_ids
    │  Cada iteración:
    │
    ├─ ¿están listos todos los inputs del nodo? (edges resueltos en all_outputs)
    │     NO → re-encola si upstream todavía activo
    │     SÍ ↓
    │
    ├─ registry.get_node(node_type)  →  Arc<dyn ExecutableNode>
    │
    ├─ build_inputs_for(node_id, edges, all_outputs)
    │     resuelve JSON pointer paths (edge.from = "node_id.field.subfield")
    │
    ├─ node.execute(&inputs, &config, &mut state, observer)
    │     │
    │     │ Para nodos llm:
    │     │   AgentService::run()  →  loop ReAct
    │     │     LLM call → tool_calls? → DagToolExecutor (impl ToolExecutor::execute())
    │     │                              → next LLM call with tool result
    │     │                              → repeat until no tool calls
    │     │
    │     └─ Emite NodeEvent vía observer.on_event(...)
    │           LlmToken, LlmToolCallStart/Finish, LlmUsage, SkillLoaded, ...
    │           (dag_engine/domain/observer.rs)
    │
    ├─ all_outputs.insert(node_id, output_value)
    │
    └─ encola nodos sucesores con edges desde este nodo
          ↑ repite hasta que VecDeque vacío
    │
    ▼
Stream<DagExecutionEvent>  →  serializado como SSE (o JSON final en CLI)
```

El trait central es `ExecutableNode::execute` en `src/libs/colmena/src/dag_engine/domain/node.rs`:

```rust
async fn execute(
    &self,
    inputs: &NodeInputs,           // HashMap<String, Value> — outputs de nodos upstream
    config: &Value,                // config estática del nodo desde graph.json
    state: &mut Value,             // estado global mutable del grafo
    observer: Option<Arc<dyn ExecutionObserver>>,
) -> Result<Value, Box<dyn StdError + Send + Sync>>;
```

Los eventos que el observer puede recibir están enumerados en `dag_engine/domain/observer.rs` (`NodeEvent` enum): `LlmToken`, `LlmToolCall`, `LlmUsage`, `SkillLoaded`, `ToolDescribed`, `ThinkingToken`, `ReasoningStart/Delta/End`, `SubgraphChildEvent`, y más. Ver la referencia completa en [`docs/sse_events_reference.md`](../sse_events_reference.md).

---

## Sigue tu interés

> **Para un recorrido archivo por archivo del árbol Rust**, ver [`CODEBASE_TOUR.md`](../CODEBASE_TOUR.md) — desglose módulo por módulo (domain/application/infrastructure) con archivos clave verificados y patrones de contribución comunes.

| Quiero... | Lee |
|-----------|-----|
| Escribir mi primer grafo JSON | [12_dag_engine_guide.md](./12_dag_engine_guide.md) + [16_data_flow_guide.md](./16_data_flow_guide.md) |
| Usar un nodo LLM con herramientas | [14_llm_deep_dive.md](./14_llm_deep_dive.md) + [09_tool_calling.md](./09_tool_calling.md) |
| Añadir un tipo de nodo nuevo | [12_dag_engine_guide.md](./12_dag_engine_guide.md) (sección "Implementar un nodo") + [23_sql_node.md](./23_sql_node.md) como ejemplo real |
| Añadir un proveedor LLM nuevo | [04_adding_providers.md](./04_adding_providers.md) |
| Usar skills (conocimiento modular) | [24_skills.md](./24_skills.md) |
| Carga progresiva de tools (lazy loading) | [29_lazy_tool_loading.md](./29_lazy_tool_loading.md) |
| Agentes anidados / subgrafos | [19_nested_agents_and_subgraphs.md](./19_nested_agents_and_subgraphs.md) |
| El nodo orchestrator (planificador + critic) | [20_orchestrator_architecture.md](./20_orchestrator_architecture.md) |
| Generar imágenes o audio | [32_multimedia_generation.md](./32_multimedia_generation.md) |
| Documentos Word/Excel como herramienta LLM | [27_documents_library.md](./27_documents_library.md) |
| Adjuntos y archivos grandes | [31_load_attachment.md](./31_load_attachment.md) + [28_large_files_api.md](./28_large_files_api.md) |
| Nodo SQL con permisos y sandbox | [23_sql_node.md](./23_sql_node.md) |
| Nodo Python script | [26_python_node.md](./26_python_node.md) |
| Nodo Socket.IO | [21_socketio_node.md](./21_socketio_node.md) |
| Nodos web (Tavily, API Explorer) | [25_web_nodes.md](./25_web_nodes.md) |
| Contexto temporal/geográfico automático | [35_temporal_geographic_context.md](./35_temporal_geographic_context.md) |

---

## Aspectos transversales

| Tema | Guía |
|------|------|
| Principios hexagonales, flujo de datos, estructura de directorios | [01_architecture.md](./01_architecture.md) |
| Persistencia de memoria (SQLite/Postgres, historial de conversaciones) | [15_memory_guide.md](./15_memory_guide.md) |
| Schema de tablas Postgres (`llm_node_history`, `dag_runs`, `secure_value_mappings`) | [30_database_schema.md](./30_database_schema.md) |
| Seguridad: Secure Values, pgcrypto (`pgp_sym_encrypt`), `secure_suspend` | [13_security_strategy.md](./13_security_strategy.md) |
| Estrategia de testing, mocking, `#[ignore]`, deny-warnings | [05_testing.md](./05_testing.md) |
| Flujo completo de tool calls (node_schema → merge → ejecución) | [22_tool_execution_flow.md](./22_tool_execution_flow.md) |
| Referencia de todos los eventos SSE emitidos por el motor | [../sse_events_reference.md](../sse_events_reference.md) |
| GC de artefactos (attachment_gc binary) | [36_attachment_gc.md](./36_attachment_gc.md) |
