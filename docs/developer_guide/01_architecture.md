# 🏗️ Arquitectura del Proyecto

### Principios de Diseño

Colmena sigue **Arquitectura Hexagonal** (Ports and Adapters) con estos principios:

1. **Separación de Responsabilidades**: Dominio, Aplicación e Infraestructura claramente separados
2. **Inversión de Dependencias**: El dominio no depende de infraestructura
3. **Testabilidad**: Cada capa es testeable independientemente
4. **Extensibilidad**: Fácil agregar nuevos proveedores sin cambiar el core

### Estructura Detallada

```
src/
├── lib.rs                          # Entry point, expone módulos públicos
├── llm/                           # Módulo LLM (core del proyecto)
│   ├── mod.rs                     # Configuración del módulo
│   ├── domain/                    # 🏛️ CAPA DE DOMINIO
│   │   ├── mod.rs                 # Exports del dominio
│   │   ├── llm_provider.rs        # Enum de proveedores y su configuración
│   │   ├── llm_config.rs          # Configuración de requests (incluye LlmUsage)
│   │   ├── llm_request.rs         # Entidad: Request de LLM
│   │   ├── llm_response.rs        # Entidad: Response de LLM
│   │   ├── llm_repository.rs      # Port: Interfaz principal
│   │   ├── llm_error.rs           # Tipos de error del dominio
│   │   ├── llm_message.rs         # Entidad: Mensaje individual
│   │   └── value_objects/         # Value Objects del dominio
│   │       ├── mod.rs
│   │       ├── llm_request_id.rs  # ID único de requests
│   │       └── llm_response_id.rs # ID único de responses
│   ├── application/               # 🎯 CAPA DE APLICACIÓN
│   │   ├── mod.rs                 # Exports de aplicación
│   │   ├── llm_call_use_case.rs   # Caso de uso: llamada síncrona
│   │   ├── llm_stream_use_case.rs # Caso de uso: streaming
│   │   └── llm_health_check_use_case.rs # Caso de uso: health check
│   └── infrastructure/            # 🔧 CAPA DE INFRAESTRUCTURA
│       ├── mod.rs                 # Exports de infraestructura
│       ├── openai_adapter.rs      # Adapter: OpenAI API
│       ├── gemini_adapter.rs      # Adapter: Gemini API
│       ├── anthropic_adapter.rs   # Adapter: Anthropic API
│       └── llm_provider_factory.rs # Factory para crear adapters
├── dag_engine/                    # 🧠 MOTOR DE EJECUCIÓN DE GRAFOS (DAG)
│   ├── mod.rs                     # (No presente, pero conceptualmente aquí)
│   ├── main.rs                    # Entry point del binario del DAG
│   ├── domain/                    # 🏛️ CAPA DE DOMINIO DEL DAG
│   │   ├── mod.rs
│   │   ├── graph.rs               # Entidades: Graph, NodeConfig, Edge
│   │   ├── node.rs                # Port: Trait ExecutableNode
│   │   └── error.rs               # Errores de dominio del DAG
│   ├── application/               # 🎯 CAPA DE APLICACIÓN DEL DAG
│   │   ├── mod.rs
│   │   ├── ports.rs               # Port: Trait NodeRegistryPort
│   │   └── run_use_case.rs        # Caso de uso: ejecutar un grafo
│   └── infrastructure/            # 🔧 CAPA DE INFRAESTRUCTURA DEL DAG
│       ├── mod.rs
│       ├── registry.rs            # Adapter: HashMapNodeRegistry
│       └── nodes/                 # Adapters: Implementaciones de nodos
│           ├── mod.rs
│           ├── math.rs
│           └── debug.rs
├── shared/                        # 🤝 FUNCIONALIDADES COMPARTIDAS
│   ├── mod.rs
│   └── infrastructure/
│       ├── mod.rs
│       ├── config_resolver.rs     # Resolución de configuración
│       └── service_container.rs   # Contenedor de servicios
└── python_bindings/              # 🐍 BINDINGS PARA PYTHON
    └── mod.rs                     # Wrappers PyO3
```

### Flujo de Datos

```
Python Call → PyO3 Bindings → Use Case → Repository → Adapter → HTTP API
     ↓                                                              ↓
Python Response ← PyO3 Bindings ← Domain Response ← Adapter ← HTTP Response
```

### Relación entre Rust y Python

Este proyecto no es una aplicación de Rust pura, sino una **librería de Python acelerada con Rust**.

- **Python es el director de orquesta**: La aplicación final es de Python. Se beneficia de su ecosistema y facilidad de uso para la lógica de alto nivel.
- **Rust es el motor de alto rendimiento**: Las operaciones computacionalmente intensivas y la lógica de negocio principal se implementan en Rust para obtener la máxima velocidad y seguridad.
- **PyO3 es el puente**: La librería `pyo3` permite exponer las funciones de Rust a Python de una manera idiomática y eficiente.

El objetivo es combinar la flexibilidad de Python con el rendimiento de Rust, delegando las tareas pesadas al código nativo compilado.
