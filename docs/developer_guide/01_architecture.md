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
│   │   └── value_objects/         # Value Objects del dominio
│   │       ├── mod.rs
│   │       ├── llm_request_id.rs  # ID único de requests
│   │       └── llm_message.rs     # Mensaje individual
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
