# 🐝 Colmena - AI Agent Orchestration Library

[![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)](https://www.rust-lang.org)
[![Python](https://img.shields.io/badge/python-3.8+-blue.svg)](https://www.python.org)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)
[![Status](https://img.shields.io/badge/status-alpha-red.svg)](https://github.com/your-org/colmena)

Una librería **nativa** de Rust para la orquestación de agentes de IA, diseñada siguiendo principios de **Arquitectura Hexagonal** y expuesta a Python mediante PyO3. Proporciona una interfaz unificada para múltiples proveedores de LLM con llamadas síncronas y streaming.

## 🎯 Características

### 🤖 Módulo LLM
- **🔌 Multi-Proveedor**: Soporte nativo para OpenAI, Gemini y Anthropic
- **⚡ Streaming**: Respuestas en tiempo real con chunks de texto
- **🐍 Python Ready**: Bindings nativos compilados con PyO3 (no wrappers)
- **🧠 Memoria Persistente**: SQLite y PostgreSQL para historial de conversaciones
- **🛠️ Tool Calling**: Soporte nativo para herramientas y agentes

### ⚙️ Motor DAG (DAG Engine)
- **📊 Ejecución de Grafos**: Motor para ejecutar flujos de trabajo complejos
- **🔗 Composición de Nodos**: Conecta LLMs, HTTP, Python y más
- **🐍 Python Node**: Ejecuta código Python arbitrario con datos JSON
- **🤖 Integración LLM**: Los LLMs pueden generar y ejecutar código Python
- **📝 Configuración JSON**: Define grafos con archivos JSON simple

### 🏗️ Arquitectura
- **🏗️ Arquitectura Limpia**: Implementación hexagonal para máxima extensibilidad
- **🔧 Configuración Flexible**: API keys desde variables de entorno o valores directos
- **🛡️ Manejo de Errores**: Gestión robusta con tipos específicos y recuperación
- **🚀 Performance**: Código nativo Rust, sin overhead de interpretación
- **🔒 Type Safety**: Garantías de tipos en tiempo de compilación

## ✅ Estado del Proyecto - FUNCIONAL

**Módulos Completamente Funcionales:**

### 🤖 Módulo LLM Base
- ✅ **Arquitectura hexagonal completa** y probada
- ✅ **Soporte Multi-LLM**: OpenAI, Gemini, Anthropic funcionando
- ✅ **Llamadas síncronas y streaming** implementadas
- ✅ **Bindings de Python nativos** compilados y probados
- ✅ **Gestión de configuración** flexible y robusta
- ✅ **Tests completos**: 8/8 tests pasando con Gemini
- ✅ **Documentación técnica** y ejemplos de uso

### ⚙️ Motor DAG (DAG Engine)
- ✅ **Ejecución de grafos** dirigidos acíclicos (DAGs)
- ✅ **Nodos disponibles**: Debug, Math, HTTP, LLM, Python, Trigger
- ✅ **Python Node**: Ejecuta código Python con integración LLM
- ✅ **Tool Calling**: Los LLMs pueden usar otros nodos como herramientas
- ✅ **Memoria persistente**: SQLite y PostgreSQL
- ✅ **Servidor HTTP**: API REST para ejecución de DAGs

## 📁 Estructura del Proyecto

```
src/
├── lib.rs                          # Entry point de la librería
├── llm/                           # 🤖 Módulo LLM
│   ├── domain/                    # 🏛️ Capa de Dominio
│   │   ├── llm_provider.rs       # Enums de proveedores
│   │   ├── llm_config.rs         # Configuraciones
│   │   ├── llm_request.rs        # Requests
│   │   ├── llm_response.rs       # Responses
│   │   ├── llm_repository.rs     # Trait principal
│   │   └── value_objects/        # Value Objects
│   ├── application/               # 🎯 Capa de Aplicación
│   │   ├── llm_call_use_case.rs  # Caso de uso: llamada normal
│   │   ├── llm_stream_use_case.rs # Caso de uso: streaming
│   │   └── agent_service.rs      # Servicio de agentes con tools
│   └── infrastructure/            # 🔧 Capa de Infraestructura
│       ├── openai_adapter.rs     # Adaptador OpenAI
│       ├── gemini_adapter.rs     # Adaptador Gemini
│       ├── anthropic_adapter.rs  # Adaptador Anthropic
│       ├── llm_provider_factory.rs # Factory
│       └── persistence/          # Repositorios de memoria
│           ├── sqlite_conversation_repository.rs
│           └── postgres_conversation_repository.rs
├── dag_engine/                    # ⚙️ Motor DAG
│   ├── main.rs                   # CLI para ejecutar DAGs
│   ├── domain/                   # 🏛️ Capa de Dominio
│   │   ├── node.rs              # Trait ExecutableNode
│   │   └── dag.rs               # Estructura del grafo
│   ├── application/              # 🎯 Capa de Aplicación
│   │   ├── dag_executor.rs      # Ejecutor de grafos
│   │   └── ports/               # Puertos/interfaces
│   ├── infrastructure/           # 🔧 Capa de Infraestructura
│   │   ├── nodes/               # Implementaciones de nodos
│   │   │   ├── debug.rs        # Nodos de depuración
│   │   │   ├── math.rs         # Nodos matemáticos
│   │   │   ├── http.rs         # Nodo HTTP
│   │   │   ├── llm.rs          # Nodo LLM
│   │   │   └── python_node.rs  # 🐍 Nodo Python (NUEVO)
│   │   ├── registry.rs          # Registro de nodos
│   │   └── dag_tool_executor.rs # Herramientas para LLM
│   └── api/                     # 🌐 API REST
│       └── server.rs            # Servidor HTTP
├── shared/                        # 🤝 Funcionalidades compartidas
│   └── infrastructure/
│       ├── config_resolver.rs    # Resolución de configuración
│       └── service_container.rs  # Contenedor de servicios
└── python_bindings/              # 🐍 Bindings para Python
    └── mod.rs                    # Wrappers PyO3
```

## 🛠️ Tecnologías

- **Rust**: Lenguaje principal, performance y seguridad
- **PyO3**: Bindings nativos para Python
- **Tokio**: Runtime asíncrono
- **Reqwest**: Cliente HTTP
- **Serde**: Serialización/deserialización
- **Arquitectura Hexagonal**: Separación limpia de responsabilidades

## 📖 Documentación

### 🚀 Para Usuarios
- [📦 **Guía de Instalación**](docs/INSTALLATION_GUIDE.md) - Instalación paso a paso en cualquier sistema operativo
- [🐍 **Ejemplos de Uso en Python**](docs/PYTHON_USAGE_EXAMPLES.md) - Casos de uso prácticos y ejemplos completos
- [🔧 **Guía de Solución de Problemas**](docs/TROUBLESHOOTING.md) - Soluciones a problemas comunes

### 👩‍💻 Para Desarrolladores
- [📋 **Documento de Diseño y Desarrollo (DDS)**](docs/dds/MODULO_LLM_DISEÑO.md) - Arquitectura detallada del módulo LLM
- [🏗️ **Guía de Arquitectura Hexagonal**](docs/dds/ARQUITECTURA_HEXAGONAL_GUIA.md) - Principios arquitectónicos aplicados
- [👩‍💻 **Guía del Desarrollador**](docs/DEVELOPER_GUIDE.md) - Contribuir, extender y entender el código
- [⚙️ **CLAUDE.md**](CLAUDE.md) - Guía para desarrollo con Claude Code

## 🚀 Instalación y Compilación

### Prerrequisitos del Sistema

**En Linux (Ubuntu/Debian):**
```bash
# Instalar dependencias del sistema
sudo apt update
sudo apt install curl build-essential python3-dev python3-pip

# Instalar Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.bashrc
```

**En macOS:**
```bash
# Instalar Xcode command line tools
xcode-select --install

# Instalar Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.bashrc
```

**En Windows:**
1. Descarga e instala Rust desde [rustup.rs](https://rustup.rs/)
2. Instala Visual Studio Build Tools con C++ support
3. Instala Python 3.8+ desde [python.org](https://python.org)

### Compilación Paso a Paso

#### 1. Clonar y Preparar el Proyecto
```bash
# Clonar el repositorio
git clone https://github.com/tu-org/colmena.git
cd colmena

# Verificar que Rust está instalado correctamente
rustc --version
cargo --version
```

#### 2. Compilar la Librería Rust
```bash
# Verificar que el código compila
cargo check

# Ejecutar tests de Rust (opcional)
cargo test

# Compilar en modo release (opcional, para mejor performance)
cargo build --release
```

#### 3. Configurar Python y Maturin
```bash
# Crear entorno virtual de Python
python3 -m venv venv
source venv/bin/activate  # En Windows: venv\Scripts\activate

# Instalar maturin (herramienta para compilar extensiones Python en Rust)
pip install maturin

# Compilar e instalar la librería Python
maturin develop --release

# Verificar la instalación
python -c "import colmena; print('✅ Colmena instalado correctamente')"
```

### Verificación de la Instalación

Ejecuta este script para verificar que todo funciona:

```python
# test_installation.py
import colmena

# Verificar que el módulo está disponible
print(f"✅ Módulo colmena cargado desde: {colmena.__file__}")

# Verificar funcionalidad básica
llm = colmena.ColmenaLlm()
print(f"✅ ColmenaLlm inicializado: {type(llm)}")

# Test con API key válida (reemplaza con tu key)
try:
    response = llm.call(
        messages=["Hola, ¿cómo estás?"],
        provider="gemini",
        api_key="TU_API_KEY_AQUI"
    )
    print(f"✅ Llamada exitosa: {response[:50]}...")
except Exception as e:
    print(f"⚠️  Necesitas una API key válida: {e}")
```

### Variables de Entorno (Opcional)

Puedes configurar las API keys como variables de entorno:

```bash
# .env o en tu shell
export OPENAI_API_KEY="tu-openai-key"
export GEMINI_API_KEY="tu-gemini-key"
export ANTHROPIC_API_KEY="tu-anthropic-key"
```

### Solución de Problemas Comunes

**Error: "Microsoft Visual C++ 14.0 is required" (Windows)**
```bash
# Instalar Visual Studio Build Tools
# Descargar desde: https://visualstudio.microsoft.com/visual-cpp-build-tools/
```

**Error: "python3-dev not found" (Linux)**
```bash
sudo apt install python3-dev python3-pip
```

**Error: "maturin not found"**
```bash
pip install --upgrade pip
pip install maturin
```

**Error de compilación con PyO3**
```bash
# Verificar versión de Python (debe ser 3.8+)
python --version

# Reinstalar con configuración específica
pip uninstall maturin
pip install maturin
maturin develop --release
```

## 🎮 Uso de la Librería

### Importar y Configurar

```python
import colmena

# Inicializar la librería
llm = colmena.ColmenaLlm()
```

### Llamadas Síncronas

```python
# Llamada simple con Gemini
response = llm.call(
    messages=["¿Qué es la arquitectura hexagonal?"],
    provider="gemini",
    model="gemini-1.5-flash",
    api_key="tu-gemini-api-key"
)
print(response)

# Llamada con OpenAI
response = llm.call(
    messages=["Explica qué es Rust"],
    provider="openai",
    model="gpt-4",
    api_key="tu-openai-api-key",
    temperature=0.7,
    max_tokens=500
)
print(response)

# Llamada con Anthropic
response = llm.call(
    messages=["¿Cómo funciona PyO3?"],
    provider="anthropic",
    model="claude-3-sonnet-20240229",
    api_key="tu-anthropic-api-key"
)
print(response)
```

### Llamadas con Streaming

```python
# Streaming con cualquier proveedor
chunks = llm.stream(
    messages=["Cuenta una historia corta"],
    provider="gemini",
    api_key="tu-api-key"
)

for chunk in chunks:
    print(chunk, end="", flush=True)
print()  # Nueva línea al final
```

### Conversaciones con Contexto

```python
# Mantener contexto en múltiples mensajes
messages = [
    "Hola, soy un desarrollador de Rust",
    "¿Puedes explicarme qué es PyO3?",
    "¿Y cómo se compila una extensión Python?"
]

response = llm.call(
    messages=messages,
    provider="gemini",
    api_key="tu-api-key"
)
print(response)
```

### Configuración Flexible

```python
# Usar variables de entorno (recomendado)
import os
os.environ['GEMINI_API_KEY'] = 'tu-api-key'

response = llm.call(
    messages=["Test con variable de entorno"],
    provider="gemini"
)

# Configuración manual con parámetros adicionales
response = llm.call(
    messages=["Respuesta creativa"],
    provider="openai",
    model="gpt-4",
    api_key="tu-openai-key",
    temperature=0.9,
    max_tokens=1000,
    top_p=0.95
)
```

### Manejo de Errores

```python
try:
    response = llm.call(
        messages=["Test"],
        provider="gemini",
        api_key="api-key-invalida"
    )
    print(response)
except colmena.LlmException as e:
    print(f"Error en la llamada LLM: {e}")
except Exception as e:
    print(f"Error inesperado: {e}")
```

## ⚙️ Uso del Motor DAG

El **DAG Engine** permite crear flujos de trabajo complejos conectando diferentes tipos de nodos en un grafo dirigido acíclico (DAG).

### Ejecutar un DAG

```bash
# Ejecutar un grafo desde un archivo JSON
cargo run --bin dag_engine run tests/python_simple_graph.json

# Servir un grafo como API HTTP
cargo run --bin dag_engine serve tests/python_llm_graph.json --port 3000
```

### Tipos de Nodos Disponibles

| `llm_call`                              | Ejecuta modelos LLM          | Generación de texto, análisis          |
| `python_script`                         | Ejecuta código Python        | Lógica personalizada, transformaciones |
| `trigger_webhook`                       | Dispara webhooks             | Integración con sistemas externos      |

### 🔗 Auto-Flattening (Conexión Simplificada)

El motor DAG soporta **auto-flattening**, lo que permite conectar nodos usando solo sus nombres, sin necesidad de especificar mapeos detallados de parámetros si la estructura de datos es compatible.

- **Mapeo Automático**: Si conectas un nodo `A` a un nodo `B` (ej. `{"from": "nodeA", "to": "nodeB"}`), el motor intentará inyectar todas las propiedades del objeto de salida de `A` directamente en el espacio de entrada de `B`.
- **Simplificación**: Evita verbosidad en los archivos JSON al no tener que mapear cada propiedad individualmente (ej. `nodeA.result -> nodeB.input`).
- **Compatibilidad**: Funciona especialmente bien con el **Python Node**, permitiendo que scripts generen múltiples variables que el siguiente nodo puede consumir de forma transparente.

### 🐍 Python Node

El **Python Node** ejecuta código Python arbitrario dentro del flujo del DAG, con integración completa con JSON.

#### Características

- ✅ **Ejecución Segura**: Código Python ejecutado en thread aislado
- ✅ **Integración JSON**: Inputs/outputs automáticos desde/hacia JSON
- ✅ **Variables Inyectadas**: Los inputs del nodo se inyectan como variables Python
- ✅ **Soporte para Funciones**: Define y usa funciones dentro del script
- ✅ **Compatible con LLMs**: Procesa código generado por LLMs (limpia markdown)
- ✅ **Librerías Estándar**: Acceso completo a la biblioteca estándar de Python

#### Ejemplo Básico

```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": {
        "x": 10,
        "y": 5
      }
    },
    "python_calc": {
      "type": "python_script",
      "config": {
        "code": "output = x * y + 2"
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    {"from": "start.x", "to": "python_calc.x"},
    {"from": "start.y", "to": "python_calc.y"},
    {"from": "python_calc.output", "to": "log_result.input"}
  ]
}
```

**Resultado**: `52` (10 × 5 + 2)

#### Ejemplo con LLM

El Python Node puede ejecutar código generado dinámicamente por un LLM:

```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": {
        "prompt": "Write a Python script that calculates the factorial of 5 and assigns it to 'output'"
      }
    },
    "llm_gen": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-4o"
      }
    },
    "python_exec": {
      "type": "python_script"
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    {"from": "start.prompt", "to": "llm_gen.prompt"},
    {"from": "llm_gen.output.content", "to": "python_exec.code"},
    {"from": "python_exec.output", "to": "log_result.input"}
  ]
}
```

**Resultado**: `120` (factorial de 5)

#### Convenciones del Python Node

1. **Inputs**: Todas las entradas del nodo se inyectan como variables globales
2. **Output**: El script debe asignar el resultado a una variable llamada `output`
3. **Code Source**: El código puede venir de:
   - Config: `config.code` (estático)
   - Input: `inputs.code` (dinámico, por ejemplo desde un LLM)
4. **Markdown Cleanup**: Automáticamente limpia bloques ```python ... ``` de código LLM
5. **Librerías**: Se puede usar `import` para cualquier librería estándar de Python

#### Ejemplo Avanzado con Funciones

```python
# Este código puede estar en config.code o ser generado por un LLM
def fibonacci(n):
    if n <= 1:
        return n
    return fibonacci(n-1) + fibonacci(n-2)

output = [fibonacci(i) for i in range(10)]
```

### Flujos Comunes

#### 1. LLM → Python → Resultado
LLM genera código → Python ejecuta → Log resultado

#### 2. HTTP → Python → LLM  
API externa → Python transforma datos → LLM analiza

#### 3. Input → Math → Python → Output
Datos iniciales → Operaciones → Lógica compleja → Resultado



## 🧪 Testing y Verificación

### Ejecutar Tests Completos

El proyecto incluye un script de testing completo:

```bash
# Activar entorno virtual
source venv/bin/activate

# Ejecutar tests de streaming (requiere API key válida)
python python/tests/test_streaming_scenarios.py
```

### Tests Incluidos

1. **Health Check**: Verificación de conectividad
2. **Llamada Simple**: Test básico de funcionalidad
3. **Llamada con Contexto**: Múltiples mensajes
4. **Conversación**: Interacción de ida y vuelta
5. **Streaming**: Respuestas en tiempo real
6. **Manejo de Errores**: API keys inválidas y errores de red
7. **Test de Performance**: Medición de tiempos de respuesta
8. **Configuración Personalizada**: Parámetros de temperatura y tokens

### Verificar Compilación Nativa

```python
# Verificar que usamos la librería Rust compilada
python prove_rust_library.py
```

Este script demuestra que:
- Los métodos son nativos (compilados desde Rust)
- No hay código Python interpretado
- La librería hace llamadas reales a APIs

## ⚡ Performance

### Ventajas de la Implementación en Rust

- **🚀 Velocidad Nativa**: Sin overhead de interpretación Python
- **🧠 Gestión de Memoria**: Control preciso con ownership de Rust
- **🔒 Thread Safety**: Garantías de concurrencia sin data races
- **⚡ HTTP Async**: Cliente HTTP nativo con tokio
- **📦 Zero-Copy**: Minimiza copias de datos entre Rust y Python

### Benchmarks (Aproximados)

| Operación       | Tiempo (ms) | Notas                     |
| --------------- | ----------- | ------------------------- |
| Inicialización  | <1          | Una sola vez por proceso  |
| Llamada Simple  | 500-2000    | Depende del proveedor LLM |
| Streaming Chunk | <10         | Por chunk individual      |
| Parsing JSON    | <5          | Nativo con serde          |

## 🏗️ Arquitectura

Colmena sigue los principios de **Arquitectura Hexagonal** (Ports and Adapters):

### 🏛️ Dominio (Core)
- **Entidades**: `LlmRequest`, `LlmResponse`, `LlmMessage`
- **Value Objects**: `LlmRequestId`, `LlmProvider`, `LlmConfig`
- **Puertos**: `LlmRepository` trait
- **Lógica de Negocio**: Validaciones y reglas de dominio

### 🎯 Aplicación (Use Cases)
- **LlmCallUseCase**: Orquesta llamadas síncronas
- **LlmStreamUseCase**: Maneja streaming
- **LlmHealthCheckUseCase**: Verifica salud de proveedores

### 🔧 Infraestructura (Adapters)
- **OpenAiAdapter**: Implementa API de OpenAI
- **GeminiAdapter**: Implementa API de Gemini
- **AnthropicAdapter**: Implementa API de Anthropic
- **ConfigResolver**: Gestiona configuración
- **Python Bindings**: Expone funcionalidad a Python

## 🤝 Contribuir

1. Fork el proyecto
2. Crea una rama feature (`git checkout -b feature/nueva-funcionalidad`)
3. Commit tus cambios (`git commit -am 'Añadir nueva funcionalidad'`)
4. Push a la rama (`git push origin feature/nueva-funcionalidad`)
5. Crear Pull Request

### Guías de Desarrollo

- Seguir principios de arquitectura hexagonal
- Mantener separación clara entre capas
- Agregar tests para nueva funcionalidad
- Documentar APIs públicas
- Seguir convenciones de Rust

## 📜 Licencia

[Definir licencia]

## 🙏 Agradecimientos

- Arquitectura hexagonal inspirada en los principios de Alistair Cockburn
- Patrón Ports and Adapters
- Comunidad Rust y PyO3

---

**🐝 Colmena** - *Orquestando el futuro de la IA, una llamada a la vez*