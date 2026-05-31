# 🏗️ Arquitectura del Proyecto

### Principios de Diseño

Colmena sigue **Arquitectura Hexagonal** (Ports and Adapters) con estos principios:

1. **Separación de Responsabilidades**: Dominio, Aplicación e Infraestructura claramente separados
2. **Inversión de Dependencias**: El dominio no depende de infraestructura
3. **Testabilidad**: Cada capa es testeable independientemente
4. **Extensibilidad**: Fácil agregar nuevos proveedores sin cambiar el core

### Estructura Detallada (Crate de Rust)

El código fuente principal se encuentra en `src/libs/colmena/src/`.

```
src/libs/colmena/src/
├── lib.rs                          # Entry point del crate (exports principales)
├── main.rs                         # Punto de entrada para tests o demos rápidas
├── llm/                            # 🤖 MÓDULO LLM (Multi-provider)
│   ├── mod.rs                      # Registro de submódulos
│   ├── domain/                     # 🏛️ CAPA DE DOMINIO
│   │   ├── mod.rs                  # Exports del dominio
│   │   ├── llm_provider.rs         # Enum ProviderKind { OpenAi, Google, Anthropic, Mock, Generated }
│   │   ├── llm_config.rs           # Configuración: model, temp, tokens, etc.
│   │   ├── llm_request.rs          # Entidad: Request (mensajes, tools, streaming)
│   │   ├── llm_response.rs         # Entidad: Response (contenido, usage, tool_calls)
│   │   ├── llm_repository.rs       # Port: Interfaz del adaptador de LLM
│   │   ├── llm_error.rs            # Errores técnicos del dominio
│   │   ├── llm_message.rs          # Entidades de mensajes y contenidos (texto/media)
│   │   ├── tools.rs                # Definición de herramientas (Tool/Function definition)
│   │   ├── tool_executor.rs        # Entidad para representar la ejecución de tools
│   │   ├── memory.rs               # Entidades para persistencia de conversaciones
│   │   └── value_objects/          # Objetos sin identidad propia (ids)
│   ├── application/                # 🎯 CAPA DE APLICACIÓN
│   │   ├── agent_service.rs        # Servicio de alto nivel (Agentes ReAct)
│   │   ├── llm_call_use_case.rs    # Orquestador: llamada síncrona
│   │   ├── llm_stream_use_case.rs  # Orquestador: streaming SSE/mensajes
│   │   └── llm_health_check_use_case.rs # Comprobación de estado de proveedores
│   └── infrastructure/             # 🔧 CAPA DE INFRAESTRUCTURA
│       ├── openai_adapter.rs       # Adapter: OpenAI con Hybrid routing
│       ├── gemini_adapter.rs       # Adapter: Google Gemini API (ProviderKind::Google)
│       ├── anthropic_adapter.rs    # Adapter: Anthropic Claude API
│       ├── mock_adapter.rs         # Adapter mockeado para testing sin API
│       ├── scripted_adapter.rs     # Adapter con respuestas pre-programadas (tests)
│       ├── llm_provider_factory.rs # Factory para resolución de adapters
│       ├── openai_tts_adapter.rs   # Adapter TTS: OpenAI
│       ├── google_tts_adapter.rs   # Adapter TTS: Google
│       ├── elevenlabs_tts_adapter.rs # Adapter TTS: ElevenLabs
│       ├── tts_provider_factory.rs # Factory para proveedores TTS
│       ├── files/                  # Files API por proveedor
│       ├── attachments/            # Registro de adjuntos por proveedor
│       ├── attachment_summary/     # Resúmenes precomputados de adjuntos
│       └── persistence/            # DB adapters (SQLite, Postgres)
├── dag_engine/                     # 🧠 MOTOR DE EJECUCIÓN DE GRAFOS (DAG)
│   ├── main.rs                     # Entry point del binario CLI (dag_engine)
│   ├── api.rs                      # Lógica compartida para REST/Internal API
│   ├── mod.rs                      # Registro de módulos
│   ├── domain/                     # 🏛️ CAPA DE DOMINIO DEL DAG
│   │   ├── graph.rs                # Estructuras Graph, NodeConfig, Edge
│   │   ├── node.rs                 # Port: Trait ExecutableNode
│   │   ├── state.rs                # Gestión de estado mutable en ejecución
│   │   ├── tool_configuration.rs   # Configuración de herramientas dinámicas
│   │   └── secure_value_repository.rs # Port: Interfaz para secretos cifrados
│   ├── application/                # 🎯 CAPA DE APLICACIÓN DEL DAG
│   │   ├── run_use_case.rs         # Motor: ordenamiento topológico y ejecución
│   │   └── secure_value_service.rs # Orquestación de valores sensibles (SecureValue, TTL 24h)
│   └── infrastructure/             # 🔧 CAPA DE INFRAESTRUCTURA DEL DAG
│       ├── registry.rs             # Mapping de tipos de nodo a implementaciones
│       ├── dag_tool_executor.rs    # Ejecutor de herramientas dentro del grafo
│       └── nodes/                  # Implementaciones de nodos específicas (ver tabla abajo)
├── skills/                         # 🛠️ SKILLS (capacidades cargables on-demand)
│   ├── domain/                     # SkillConfig { builtin: [..], paths: [..] }, recursive references (depth 5)
│   ├── application/                # load_skill use case, recursive reference resolver
│   └── infrastructure/             # Loader de SKILL.md + frontmatter (YAML)
├── documents/                      # 📄 ARTIFACTS / DOCUMENTOS (Markdown, HTML, etc.)
│   ├── domain/                     # ArtifactKind { Markdown, Html, ... }, HtmlIR, PatchOp
│   ├── application/                # upload_asset, list_assets, delete_asset, render, validate
│   └── infrastructure/             # HtmlRenderer, HtmlValidator, HtmlOpApplier
├── storage/                        # 💾 ALMACENAMIENTO DE BLOBS
│   ├── domain/                     # Port: AssetStore, OutputStorageRepository
│   └── infrastructure/             # LocalFsAssetStore, GcsAssetStore (feature `gcs`)
├── web/                            # 🌐 CLIENTES WEB (search / scraping)
│   └── infrastructure/             # Tavily, fetchers HTTP
├── attachment_gc/                  # 🧹 GARBAGE COLLECTOR de adjuntos huérfanos
│   ├── application/                # Use case de barrido programado
│   └── infrastructure/             # Scheduler + adapters de proveedores
├── shared/                         # 🤝 FUNCIONALIDADES COMPARTIDAS
│   └── infrastructure/
│       ├── config_resolver.rs      # Resolución de variables ${ENV}
│       └── service_container.rs    # DI Container para servicios críticos
├── python_bindings/                # 🐍 BINDINGS PyO3 (Python Integration)
└── node_bindings/                  # 📦 BINDINGS Napi-RS (TypeScript/Node)
```

### Nodos del DAG (`dag_engine/infrastructure/nodes/`)

| Módulo | Propósito |
|---|---|
| `llm.rs` | Llamada LLM (soporta `skills_path` / `skills_paths` para cargar skills) |
| `llm_synthetic_tools/` | Tools sintéticas: `load_skill({name, reference?})`, `load_attachment`, `describe_tool`, document tools |
| `http.rs` | Llamadas HTTP arbitrarias |
| `math.rs` | Aritmética / expresiones |
| `debug.rs` | Log de inspección |
| `input.rs` / `output.rs` | Entrada/salida del grafo |
| `orchestrator.rs` | Orquestador anidado (planner + critic + phase_reactor + final_reactor) |
| `planner.rs` | Generación de plan multi-paso |
| `critic.rs` | Crítica adversarial (prompts en inglés: `=== PREVIOUS ATTEMPT — WHY IT FAILED ===`) |
| `reactor.rs` | Ejecutor reactivo de pasos |
| `extraction.rs` | Extracción estructurada (JSON / schema) |
| `qa_response_parser.rs` | Parseo de respuestas Q&A |
| `subgraph.rs` | Ejecución de grafos anidados |
| `loop_controller.rs` | Bucles condicionales |
| `suspend.rs` / `secure_suspend.rs` | Pausa el grafo (segundo cifrado con SecureValue, TTL 24h) |
| `trigger.rs` | Disparadores de eventos |
| `task_memory_writer.rs` | Escritura en memoria de tarea / sesión |
| `current_time.rs` | Reloj/timezone |
| `python_node.rs` | Ejecución de Python embebido |
| `socketio.rs` | Emisión Socket.IO |
| `sql.rs` | Consultas SQL |
| `tavily_client.rs` | Búsqueda web vía Tavily |
| `api_explorer.rs` | Toolkit de exploración de APIs (5 sub-tools) |
| `echo_toolkit.rs` | Toolkit de prueba (echo) |
| `document_nodes.rs` | Creación/edición de documentos (Markdown/HTML) |
| `image_generation.rs` | Generación de imágenes |
| `image_edit.rs` | Edición de imágenes |
| `tts.rs` | Text-to-speech |
| `util/`, `prompts/` | Helpers internos y plantillas de system prompts |

### Flujo de Datos

```
Python Call → PyO3 Bindings → Use Case → Repository → Adapter → HTTP API
     ↓                                                              ↓
Python Response ← PyO3 Bindings ← Domain Response ← Adapter ← HTTP Response
```

### Relación entre Rust y Python

Colmena es una **librería de Python acelerada con Rust**. Rust implementa el motor de ejecución, la seguridad y el rendimiento, mientras que Python proporciona la flexibilidad necesaria para orquestar la lógica de negocio de alto nivel a través de los bindings en `python_bindings`.

### Hybrid API Routing (OpenAI)

Para maximizar la compatibilidad y funcionalidad, algunos adaptadores (como OpenAI) implementan un **enrutamiento híbrido**. 
- Las peticiones estándar de texto e imágenes se enrutan a través del endpoint `/v1/chat/completions`.
- Las peticiones que contienen documentos complejos (como PDFs) se transforman automáticamente y se enrutan a través del nuevo endpoint `/v1/responses` (OpenAI Responses API), permitiendo un procesamiento nativo de archivos sin romper la interfaz del dominio.
