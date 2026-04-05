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
src/
├── lib.rs                          # Entry point del crate (exports principales)
├── main.rs                         # Punto de entrada para tests o demos rápidas
├── llm/                           # 🤖 MÓDULO LLM (Multi-provider)
│   ├── mod.rs                     # Registro de submódulos
│   ├── domain/                    # 🏛️ CAPA DE DOMINIO
│   │   ├── mod.rs                 # Exports del dominio
│   │   ├── llm_provider.rs        # Enum de proveedores (OpenAI, Anthropic, Gemini)
│   │   ├── llm_config.rs          # Configuración: model, temp, tokens, etc.
│   │   ├── llm_request.rs         # Entidad: Request (mensajes, tools, streaming)
│   │   ├── llm_response.rs        # Entidad: Response (contenido, usage, tool_calls)
│   │   ├── llm_repository.rs      # Port: Interfaz del adaptador de LLM
│   │   ├── llm_error.rs           # Errores técnicos del dominio
│   │   ├── llm_message.rs         # Entidades de mensajes y contenidos (texto/media)
│   │   ├── tools.rs               # Definición de herramientas (Tool/Function definition)
│   │   ├── tool_executor.rs       # Entidad para representar la ejecución de tools
│   │   ├── memory.rs              # Entidades para persistencia de conversaciones
│   │   └── value_objects/         # Objetos sin identidad propia (ids)
│   ├── application/               # 🎯 CAPA DE APLICACIÓN
│   │   ├── agent_service.rs       # Servicio de alto nivel (Agentes ReAct)
│   │   ├── llm_call_use_case.rs   # Orquestador: llamada síncrona
│   │   ├── llm_stream_use_case.rs # Orquestador: streaming SSE/mensajes
│   │   └── llm_health_check_use_case.rs # Comprobación de estado de proveedores
│   └── infrastructure/            # 🔧 CAPA DE INFRAESTRUCTURA
│       ├── openai_adapter.rs      # Adapter: OpenAI con Hybrid routing
│       ├── gemini_adapter.rs      # Adapter: Google Gemini API
│       ├── anthropic_adapter.rs   # Adapter: Anthropic Claude API
│       ├── llm_provider_factory.rs # Factory para resolución de adapters
│       ├── mock_adapter.rs        # Adapter mockeado para testing sin API
│       └── persistence/           # DB adapters (SQLite, Postgres)
├── dag_engine/                    # 🧠 MOTOR DE EJECUCIÓN DE GRAFOS (DAG)
│   ├── main.rs                    # Entry point del binario CLI (dag_engine)
│   ├── api.rs                     # Lógica compartida para REST/Internal API
│   ├── mod.rs                     # Registro de módulos
│   ├── domain/                    # 🏛️ CAPA DE DOMINIO DEL DAG
│   │   ├── graph.rs               # Estructuras Graph, NodeConfig, Edge
│   │   ├── node.rs                # Port: Trait ExecutableNode
│   │   ├── state.rs               # Gestión de estado mutable en ejecución
│   │   ├── tool_configuration.rs  # Configuración de herramientas dinámicas
│   │   └── secure_value_repository.rs # Port: Interfaz para secretos cifrados
│   ├── application/               # 🎯 CAPA DE APLICACIÓN DEL DAG
│   │   ├── run_use_case.rs        # Motor: ordenamiento topológico y ejecución
│   │   └── secure_value_service.rs # Orquestación de valores sensibles (SecureValue)
│   └── infrastructure/            # 🔧 CAPA DE INFRAESTRUCTURA DEL DAG
│       ├── registry.rs            # Mapping de tipos de nodo a implementaciones
│       ├── dag_tool_executor.rs   # Ejecutor de herramientas dentro del grafo
│       └── nodes/                 # Implementaciones de nodos específicas
│           ├── llm.rs, http.rs    # Nodos inteligentes/externos
│           ├── math.rs, debug.rs  # Nodos básicos y de log
│           └── orchestrator.rs    # Nodos de flujo avanzado
├── shared/                        # 🤝 FUNCIONALIDADES COMPARTIDAS
│   └── infrastructure/
│       ├── config_resolver.rs     # Resolución de variables ${ENV}
│       └── service_container.rs   # DI Container para servicios críticos
├── python_bindings/              # 🐍 BINDINGS PyO3 (Python Integration)
└── node_bindings/                # 📦 BINDINGS Napi-RS (TypeScript/Node)
```

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
