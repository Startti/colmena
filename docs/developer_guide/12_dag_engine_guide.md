# 🚀 Guía de Desarrollo: `dag_engine`

Este documento describe la arquitectura y el proceso de desarrollo para el `dag_engine`, un motor de ejecución de Grafos Acíclicos Dirigidos (DAG) extensible, implementado en Rust y basado en una arquitectura hexagonal limpia.

## 🚀 Conceptos Clave

El motor está diseñado para ejecutar un DAG definido en un fichero JSON.

### El Fichero `graph.json`

Este fichero JSON es el "código fuente" para el motor. Define tres elementos clave:

1.  **`nodes`**: Un mapa de todas las operaciones en el grafo. Cada nodo tiene un ID único (ej. `"start_data"`, `"add_step"`) y define:
    *   **`type`**: Un string (ej. `"add"`, `"log"`, `"http_request"`, `"llm_call"`) que se mapea a una implementación específica en Rust.
    *   **`config`**: Un objeto JSON para valores estáticos que el nodo necesita (ej. un exponente, un prompt, una URL, un API key).

2.  **`edges`**: Una lista de conexiones que definen el flujo de datos.
    *   **`from`**: El origen de los datos, usando una sintaxis similar a JSON-pointer (ej. `"node_id.field_a"` o `"node_id.output"`).
    *   **`to`**: El destino de los datos (ej. `"other_node.input_b"`).

### Flujo de Datos

- El motor ejecuta los nodos en un orden determinado por un **ordenamiento topológico**.
- La salida de un nodo se pasa a la entrada del siguiente, según lo definido en los `edges`.
- Todos los nodos estándar (matemáticos, de log, etc.) deben devolver su resultado envuelto en una clave `output`, por ejemplo: `{ "output": 75.0 }`.
- Los nodos raíz (como `mock_input` o `trigger_webhook`) son especiales y emiten su objeto de datos como salida.

### Configuración Dinámica

**Novedad**: Todos los nodos ahora soportan **configuración dinámica**, donde los valores de `inputs` tienen prioridad sobre los valores de `config`. Esto permite que los nodos se configuren dinámicamente en tiempo de ejecución basándose en las salidas de nodos anteriores.

**Ejemplo**: El `HttpNode` puede recibir el `endpoint` desde el nodo trigger en lugar de tenerlo codificado en la configuración.

## 🏛️ Arquitectura: Hexagonal (Puertos y Adaptadores)

El motor sigue una estricta arquitectura hexagonal, separando la lógica en tres capas distintas. Esto hace que el sistema sea altamente modular y fácil de testear y extender.

### 1. `domain` (El Núcleo)

Es el corazón de la aplicación. Es Rust puro y no tiene dependencias del "mundo exterior" (como bases de datos, APIs o nuestro `main.rs`).

-   **`domain/graph.rs`**: Define las estructuras de datos puras (`Graph`, `NodeConfig`, `Edge`).
-   **`domain/node.rs`**: Define el "Puerto" principal (el trait `ExecutableNode`). Este es el contrato central que todos los nodos deben firmar. Básicamente dice: "Debes ser capaz de ejecutar".
-   **`domain/error.rs`**: Define los errores puros del dominio (`DagError`, ej. `CycleDetected`).

### 2. `application` (El Orquestador)

Esta capa contiene la "lógica de negocio" de cómo ejecutar un grafo. Depende del `domain` pero no sabe nada sobre cómo se implementan los nodos.

-   **`application/ports.rs`**: Define los "Puertos" que la aplicación necesita del mundo exterior (ej. el trait `NodeRegistryPort`, que dice "Necesito una forma de encontrar un nodo a partir de su `type` string").
-   **`application/run_use_case.rs`**: Es el cerebro del motor.
    -   Recibe el `NodeRegistryPort` mediante inyección de dependencias.
    -   Realiza el ordenamiento topológico para obtener el orden de ejecución.
    -   Itera a través de los nodos.
    -   Construye los `NodeInputs` para cada nodo parseando los `edges`.
    -   Usa el `NodeRegistryPort` para obtener la implementación correcta del nodo.
    -   Llama a `node.execute()`.

### 3. `infrastructure` (El Mundo "Real")

Esta capa implementa todos los "Puertos" definidos en las capas `domain` y `application`. Aquí es donde ocurre todo el trabajo "sucio".

-   **`infrastructure/nodes/`**: Contiene todas nuestras implementaciones de nodos (ej. `AddNode`, `LogNode`, `HttpNode`, `LlmNode`). Cada uno de estos es un "Adaptador" que implementa el trait `ExecutableNode`.
-   **`infrastructure/registry.rs`**: Es el "Adaptador" que implementa el `NodeRegistryPort`. `HashMapNodeRegistry` usa un simple `HashMap` para conectar strings (ej. `"add"`, `"http_request"`) con la estructura concreta del nodo.
-   **`main.rs`**: Es el "Adaptador Primario" o "Ensamblador". Inicializa el `HashMapNodeRegistry`, lo inyecta en el `DagRunUseCase`, y luego le indica al caso de uso que se ejecute.

## 🔌 Sistema de Puertos por Defecto (Default Input/Output)

> **Novedad v0.3.0**: Cada nodo ahora declara sus puertos de entrada y salida por defecto, permitiendo definir edges sin especificar campos de forma explícita.

### ¿Por qué?

Anteriormente, las edges requerían especificar siempre los campos:
```json
{ "from": "llm1.result", "to": "llm2.prompt" }  // Verboso
```

Ahora, con defaults, puedes simplificar:
```json
{ "from": "llm1", "to": "llm2" }  // Limpio e intencional
```

El engine automáticamente resuelve `llm1.result → llm2.prompt` basándose en los defaults declarados por cada nodo.

### Cómo Funciona

Cada nodo implementa:
```rust
fn default_input(&self) -> Option<&str> { Some("prompt") }
fn default_output(&self) -> Option<&str> { Some("result") }
```

**Resolución de Edges:**

| Edge | Comportamiento |
|---|---|
| `{ from: "A", to: "B" }` | Usa `A.default_output → B.default_input` |
| `{ from: "A.field", to: "B" }` | Usa `A.field → B.default_input` |
| `{ from: "A", to: "B.field" }` | Usa `A.default_output → B.field` |
| `{ from: "A.x", to: "B.y" }` | Usa `A.x → B.y` (sin defaults) |

**Smart Extraction & Fallbacks:**
- Si source emite objeto raw sin campo de salida específico, y target espera campo específico, el engine extrae automáticamente ese campo.
- Si target no tiene `default_input`, intenta aplanar (flatten) todas las claves del source.

### Tabla de Puertos por Defecto

Para la lista completa de nodos y sus defaults, ver [`docs/agent_context/node_ports_reference.md`](../agent_context/node_ports_reference.md).

**Resumen:**
- Nodos con `default_input`: `llm_call`, `output`, `log`, `suspend`, `loop_controller`, `exponential`
- Nodos **sin** `default_input` (requieren campos explícitos): `add`, `subtract`, `multiply`, `divide`, `http_request`, `task_memory_writer`
- Nodos con inputs dinámicos: `python_script`, `planner`, `critic`, `information_extraction`, `reactor`

### Ejemplos

#### Ejemplo 1: LLM Chain (Implicit)
```json
{
  "edges": [
    { "from": "researcher", "to": "writer" }
  ]
}
```
✅ Funciona: `researcher.result → writer.prompt`

#### Ejemplo 2: Math Operations (Explicit Required)
```json
{
  "edges": [
    { "from": "input_a.output", "to": "add_node.a" },
    { "from": "input_b.output", "to": "add_node.b" }
  ]
}
```
✅ Necesario: `AddNode` no tiene `default_input`

#### Ejemplo 3: Smart Extraction
```json
{
  "edges": [
    { "from": "mock_input", "to": "exponential" }
  ]
}
```
✅ Mock emite `{ input: 5 }`, exponential extrae `.input` automáticamente

---

## 📦 Tipos de Nodos Disponibles

### Nodos Matemáticos
- `add`, `subtract`, `multiply`, `divide`: Operaciones básicas
- `exponential`: Eleva un número a una potencia

### Nodos de Depuración
- `log`: Imprime valores a la consola
- `mock_input`: Proporciona datos de entrada para testing

### Nodos de Trigger
- `trigger_webhook`: Recibe peticiones HTTP en modo `serve` o usa `test_payload` en modo `run`

### Nodos HTTP
- `http_request`: Realiza peticiones HTTP a APIs externas

### Nodos LLM
- `llm_call`: Llama a modelos de lenguaje (OpenAI, Gemini, Anthropic). Soporta **Memoria**, **Streaming** y **Visión/Documentos**.

#### Visión y Soporte de Documentos
El nodo `llm_call` permite enviar archivos (imágenes y PDFs) a los modelos que lo soportan. Puedes pasar archivos de dos formas: mediante una ruta local o mediante un string Base64.

**Configuración de `files`:**
```json
"files": [
  {
    "mime_type": "application/pdf",
    "filename": "documento.pdf",
    "path": "ruta/al/archivo.pdf"
  },
  {
    "mime_type": "image/jpeg",
    "data": "base64_encoded_string..."
  }
]
```

- **`mime_type`**: Tipo MIME del archivo (ej. `image/png`, `application/pdf`).
- **`filename`**: (Recomendado) Nombre del archivo. Requerido por algunos proveedores como OpenAI para procesar documentos PDF.
- **`path`**: Ruta al archivo en el disco local.
- **`data`**: Contenido del archivo codificado en Base64 (si no se usa `path`).

> [!NOTE]
> **OpenAI Dual-Routing**: Para OpenAI, el motor utiliza automáticamente el endpoint `/v1/responses` cuando se detectan documentos (PDF), permitiendo un procesamiento nativo superior a la conversión a imágenes. Las imágenes siguen usando `/v1/chat/completions`.

## 🧠 Memoria y Persistencia

El `dag_engine` soporta **persistencia de conversaciones** para los nodos LLM mediante selección dinámica de backend de base de datos. Esto permite mantener el contexto entre múltiples ejecuciones y crear agentes con memoria a largo plazo.

### 🎯 Características

- **Selección Dinámica de Backend**: Elige entre SQLite y PostgreSQL por nodo
- **Variables de Entorno**: Usa `${VAR_NAME}` para configuración segura
- **Connection Pooling**: Reutilización automática de conexiones
- **Migraciones Automáticas**: Las tablas se crean automáticamente
- **Thread-Safe**: Soporte para ejecución concurrente

### 🔧 Configuración

#### Opción 1: SQLite (Desarrollo/Local)

Ideal para desarrollo, testing y aplicaciones single-user.

**Archivo `.env`:**
```bash
# No es necesario configurar DATABASE_URL para SQLite
# Puedes especificar la ruta directamente en el DAG
```

**En tu DAG:**
```json
{
  "type": "llm_call",
  "config": {
    "provider": "openai",
    "api_key": "${OPENAI_API_KEY}",
    "thread_id": "user_session_123",
    "connection_url": "sqlite://colmena_memory.db",
    "prompt": "Hello!"
  }
}
```

#### Opción 2: PostgreSQL (Producción)

Ideal para producción, aplicaciones multi-user y escalabilidad.

**Archivo `.env`:**
```bash
# PostgreSQL estándar
DATABASE_URL="postgresql://user:password@localhost:5432/database_name"

# O con el protocolo alternativo
DATABASE_URL="postgres://user:password@localhost:5432/database_name"

# Ejemplo con Supabase
DATABASE_URL="postgresql://postgres:password@db.xxxxx.supabase.co:5432/postgres"
```

**En tu DAG:**
```json
{
  "type": "llm_call",
  "config": {
    "provider": "openai",
    "api_key": "${OPENAI_API_KEY}",
    "thread_id": "user_session_123",
    "connection_url": "${DATABASE_URL}",
    "prompt": "Hello!"
  }
}
```

### 📝 Formatos de Connection URL Soportados

| Base de Datos            | Formato                               | Ejemplo                                         |
| ------------------------ | ------------------------------------- | ----------------------------------------------- |
| SQLite (relativo)        | `sqlite://path/to/file.db`            | `sqlite://memory.db`                            |
| SQLite (absoluto)        | `sqlite:///absolute/path/to/file.db`  | `sqlite:///var/data/memory.db`                  |
| SQLite (memoria)         | `sqlite::memory:`                     | `sqlite::memory:`                               |
| PostgreSQL               | `postgresql://user:pass@host:port/db` | `postgresql://postgres:pwd@localhost:5432/mydb` |
| PostgreSQL (alternativo) | `postgres://user:pass@host:port/db`   | `postgres://postgres:pwd@localhost:5432/mydb`   |

### 🎯 Uso en Nodos `llm_call`

Para habilitar memoria en un nodo LLM, necesitas dos campos:

1. **`thread_id`**: Identificador único de la conversación
2. **`connection_url`**: URL de conexión a la base de datos

Ambos pueden estar en `config` (estático) o en `inputs` (dinámico).

#### Ejemplo Básico

```json
{
  "nodes": {
    "chat": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-3.5-turbo",
        "thread_id": "conversation_001",
        "connection_url": "sqlite://chat.db",
        "prompt": "Remember: my name is Alice"
      }
    }
  }
}
```

### 📚 Ejemplos Completos

#### Ejemplo 1: Memoria con SQLite

Este ejemplo demuestra cómo usar SQLite para persistencia local.

**Archivo:** `tests/memory_sqlite_example.json`

```json
{
    "nodes": {
        "step_1": {
            "type": "llm_call",
            "config": {
                "provider": "openai",
                "api_key": "${OPENAI_API_KEY}",
                "model": "gpt-3.5-turbo",
                "system_message": "You are a helpful assistant with perfect memory.",
                "thread_id": "sqlite_test_thread_001",
                "connection_url": "sqlite://colmena_memory.db",
                "prompt": "My name is Alice and I love programming in Rust."
            }
        },
        "step_2": {
            "type": "llm_call",
            "config": {
                "provider": "openai",
                "api_key": "${OPENAI_API_KEY}",
                "model": "gpt-3.5-turbo",
                "thread_id": "sqlite_test_thread_001",
                "connection_url": "sqlite://colmena_memory.db",
                "prompt": "What is my name and what do I love?"
            }
        },
        "log_result": {
            "type": "log"
        }
    },
    "edges": [
        {
            "from": "step_1.output",
            "to": "step_2.dummy_input"
        },
        {
            "from": "step_2.output",
            "to": "log_result.input"
        }
    ]
}
```

**Ejecutar:**
```bash
cargo run --bin dag_engine -- run tests/memory_sqlite_example.json
```

**Resultado esperado:**
- `step_1` guarda "My name is Alice..." en la base de datos
- `step_2` recupera el historial y responde correctamente con el nombre

#### Ejemplo 2: Memoria con PostgreSQL

Este ejemplo usa PostgreSQL para producción con variables de entorno.

**Archivo `.env`:**
```bash
DATABASE_URL="postgresql://postgres:password@localhost:5432/colmena_memory"
OPENAI_API_KEY="sk-..."
```

**Archivo:** `tests/memory_postgres_example.json`

```json
{
    "nodes": {
        "step_1": {
            "type": "llm_call",
            "config": {
                "provider": "openai",
                "api_key": "${OPENAI_API_KEY}",
                "model": "gpt-3.5-turbo",
                "system_message": "You are a helpful assistant with perfect memory.",
                "thread_id": "postgres_test_thread_001",
                "connection_url": "${DATABASE_URL}",
                "prompt": "My favorite color is blue and I work as a software engineer."
            }
        },
        "step_2": {
            "type": "llm_call",
            "config": {
                "provider": "openai",
                "api_key": "${OPENAI_API_KEY}",
                "model": "gpt-3.5-turbo",
                "thread_id": "postgres_test_thread_001",
                "connection_url": "${DATABASE_URL}",
                "prompt": "What is my favorite color and what do I do for work?"
            }
        },
        "log_result": {
            "type": "log"
        }
    },
    "edges": [
        {
            "from": "step_1.output",
            "to": "step_2.dummy_input"
        },
        {
            "from": "step_2.output",
            "to": "log_result.input"
        }
    ]
}
```

**Ejecutar:**
```bash
cargo run --bin dag_engine -- run tests/memory_postgres_example.json
```

#### Ejemplo 3: Memoria Dinámica (Thread ID desde Webhook)

Este ejemplo muestra cómo usar diferentes threads por usuario en un servidor.

```json
{
  "nodes": {
    "webhook": {
      "type": "trigger_webhook",
      "config": {
        "path": "/chat",
        "method": "POST",
        "test_payload": {
          "user_id": "user_123",
          "message": "What's my name?"
        }
      }
    },
    "chat": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-3.5-turbo",
        "connection_url": "${DATABASE_URL}"
      }
    },
    "log_response": {
      "type": "log"
    }
  },
  "edges": [
    {
      "from": "webhook.output.user_id",
      "to": "chat.thread_id"
    },
    {
      "from": "webhook.output.message",
      "to": "chat.prompt"
    },
    {
      "from": "chat.output",
      "to": "log_response.input"
    }
  ]
}
```

**Modo Serve:**
```bash
cargo run --bin dag_engine -- serve tests/dynamic_memory.json
```

**Petición HTTP:**
```bash
curl -X POST http://localhost:3000/chat \
  -H "Content-Type: application/json" \
  -d '{"user_id": "alice_001", "message": "My name is Alice"}'

curl -X POST http://localhost:3000/chat \
  -H "Content-Type: application/json" \
  -d '{"user_id": "alice_001", "message": "What is my name?"}'
```

### 🔍 Cómo Funciona Internamente

1. **Primera ejecución con un `thread_id`:**
   - Se conecta a la base de datos especificada en `connection_url`
   - Ejecuta migraciones automáticamente (crea tablas si no existen)
   - Crea un nuevo thread en la base de datos
   - Guarda el mensaje del usuario y la respuesta del LLM

2. **Ejecuciones subsecuentes con el mismo `thread_id`:**
   - Reutiliza la conexión del pool (más rápido)
   - Carga todo el historial de mensajes del thread
   - Envía el historial completo al LLM para mantener contexto
   - Guarda el nuevo mensaje y respuesta

3. **Connection Pooling:**
   - Las conexiones se cachean por `connection_url`
   - Múltiples nodos pueden compartir la misma conexión
   - PostgreSQL: hasta 5 conexiones concurrentes
   - SQLite: 1 conexión (limitación de SQLite)

### ⚠️ Consideraciones Importantes

- **Thread IDs únicos**: Usa IDs únicos por conversación (ej: `user_id`, `session_id`)
- **Seguridad**: Nunca hardcodees credenciales, usa variables de entorno
- **SQLite Limitations**: SQLite no soporta escrituras concurrentes, usa PostgreSQL para producción
- **Migraciones**: Se ejecutan automáticamente en la primera conexión
- **Costos de LLM**: El historial completo se envía en cada llamada, considera el costo de tokens

### 🐛 Troubleshooting

**Error: "Unsupported database protocol"**
- Verifica que uses `sqlite://`, `postgres://` o `postgresql://`
- Revisa que la variable de entorno esté correctamente configurada

**Error: "Failed to connect to Postgres: pool timed out"**
- Verifica que la base de datos esté accesible
- Revisa las credenciales en el connection URL
- Asegúrate de que el firewall permita la conexión

**Error: "Environment variable X not found"**
- Verifica que el archivo `.env` exista en la raíz del proyecto
- Asegúrate de que la variable esté definida sin espacios: `VAR=value`
- El archivo `.env` se carga automáticamente al iniciar el DAG engine

## 🔐 Variables de Entorno en Configuración

Puedes usar variables de entorno directamente en la configuración de tus nodos usando la sintaxis `${VAR_NAME}`. Esto es ideal para no hardcodear API Keys.

```json
"config": {
  "api_key": "${OPENAI_API_KEY}",
  "model": "gpt-4"
}
```
El motor resolverá `${OPENAI_API_KEY}` buscando en las variables de entorno del sistema (o archivo `.env`).

## 🔧 Cómo Crear un Nuevo Nodo

Crear un nuevo nodo es la forma principal de extender el motor. Es un proceso simple de dos pasos.

### Paso 1: Implementar el Trait `ExecutableNode`

Primero, crea la estructura de tu nodo e implementa el trait `ExecutableNode`.

-   **Leer de `inputs`**: Usa `inputs.get("input_name")` para obtener datos de los `edges` entrantes.
-   **Leer de `config`**: Usa `config.get("config_key")` para obtener configuración estática.
-   **Configuración Dinámica**: Implementa la precedencia `inputs > config` para soportar configuración dinámica.
-   **Devolver Salida**: Devuelve tu resultado envuelto en `json!({ "output": ... })`.

```rust
// Ejemplo: HttpNode con configuración dinámica
use crate::domain::node::{ExecutableNode, NodeInputs};
use serde_json::{json, Value};
use std::error::Error as StdError;

pub struct HttpNode;

#[async_trait::async_trait]
impl ExecutableNode for HttpNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
    ) -> Result<Value, Box<dyn StdError>> {
        // Configuración dinámica: inputs > config
        let base_url = inputs.get("base_url").and_then(|v| v.as_str())
            .or_else(|| config.get("base_url").and_then(|v| v.as_str()))
            .unwrap_or("");
            
        let endpoint = inputs.get("endpoint").and_then(|v| v.as_str())
            .or_else(|| config.get("endpoint").and_then(|v| v.as_str()))
            .unwrap_or("");
        
        // ... realizar petición HTTP ...
        
        Ok(json!({
            "output": {
                "status": 200,
                "body": response_body
            }
        }))
    }

    fn schema(&self) -> Value {
        json!({
            "type": "http_request",
            "config": {
                "base_url": "string",
                "endpoint": "string",
                "method": "string"
            },
            "inputs": {
                "base_url": "string (optional)",
                "endpoint": "string (optional)",
                "method": "string (optional)",
                "body": "any (optional)"
            },
            "outputs": {
                "status": "integer",
                "body": "any"
            }
        })
    }
}
```

### Paso 2: Registrar el Nodo

Segundo, "inyecta" tu nuevo nodo en la aplicación añadiéndolo al registro.

Abre `src/dag_engine/infrastructure/registry.rs` y añade tu nodo en la función `HashMapNodeRegistry::new()`.

```rust
// en: src/dag_engine/infrastructure/registry.rs

// ... (otros registros de nodos) ...
nodes.insert("http_request".to_string(), Arc::new(HttpNode));
nodes.insert("llm_call".to_string(), Arc::new(LlmNode));
        
Self { nodes }
```

## 🧪 Testing Local con `test_payload`

Para facilitar el desarrollo y testing, el nodo `trigger_webhook` soporta la opción `test_payload` que permite ejecutar grafos localmente sin levantar un servidor.

### Modo Run (Testing Local)

```json
{
  "nodes": {
    "my_webhook": {
      "type": "trigger_webhook",
      "config": {
        "path": "/test",
        "method": "POST",
        "test_payload": {
          "message": "Hello from local test!"
        }
      }
    },
    "log_step": {
      "type": "log"
    }
  },
  "edges": [
    {
      "from": "my_webhook.output.message",
      "to": "log_step.input"
    }
  ]
}
```

Ejecutar:
```bash
cargo run --bin dag_engine -- run tests/my_graph.json
```

### Modo Serve (Producción)

En modo `serve`, el `test_payload` es ignorado y se usa el payload real de las peticiones HTTP:

```bash
cargo run --bin dag_engine -- serve tests/my_graph.json
```

Luego hacer peticiones:
```bash
curl -X POST http://localhost:3000/test \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello from HTTP!"}'
```

## 📊 Ejemplos Completos

### Ejemplo 1: Llamada HTTP Dinámica

```json
{
  "nodes": {
    "webhook": {
      "type": "trigger_webhook",
      "config": {
        "path": "/fetch-joke",
        "method": "POST",
        "test_payload": {
          "endpoint": "/random_joke"
        }
      }
    },
    "http_call": {
      "type": "http_request",
      "config": {
        "base_url": "https://official-joke-api.appspot.com",
        "method": "GET"
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    {
      "from": "webhook.output.endpoint",
      "to": "http_call.endpoint"
    },
    {
      "from": "http_call.output",
      "to": "log_result.input"
    }
  ]
}
```

### Ejemplo 2: Llamada a LLM

```json
{
  "nodes": {
    "webhook": {
      "type": "trigger_webhook",
      "config": {
        "path": "/ask-llm",
        "method": "POST",
        "test_payload": {
          "question": "What is Rust?"
        }
      }
    },
    "llm_step": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-3.5-turbo",
        "system_message": "You are a helpful programming assistant.",
        "max_tokens": 100
      }
    },
    "log_answer": {
      "type": "log"
    }
  },
  "edges": [
    {
      "from": "webhook.output.question",
      "to": "llm_step.prompt"
    },
    {
      "from": "llm_step.output",
      "to": "log_answer.input"
    }
  ]
}
```

### Ejemplo 3: Pipeline HTTP → LLM

```json
{
  "nodes": {
    "webhook": {
      "type": "trigger_webhook",
      "config": {
        "path": "/analyze-joke",
        "method": "POST",
        "test_payload": {}
      }
    },
    "get_joke": {
      "type": "http_request",
      "config": {
        "base_url": "https://official-joke-api.appspot.com",
        "endpoint": "/random_joke",
        "method": "GET"
      }
    },
    "analyze_joke": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-3.5-turbo",
        "system_message": "You are a comedy expert. Analyze jokes.",
        "max_tokens": 150
      }
    },
    "log_analysis": {
      "type": "log"
    }
  },
  "edges": [
    {
      "from": "get_joke.output.body.setup",
      "to": "analyze_joke.prompt"
    },
    {
      "from": "analyze_joke.output",
      "to": "log_analysis.input"
    }
  ]
}
```

### Ejemplo 4: Extracción de Información a JSON Estructurado

El nodo `information_extraction` permite tomar texto no estructurado y usar un LLM para extraer un JSON estrictamente apegado a un `schema`. Soporta múltiples entradas inyectadas dinámicamente en el objeto `texts`.

```json
{
  "nodes": {
    "slack_message": {
      "type": "input",
      "config": {
        "data": "Hi team, let's ship the new deployment feature. The deadline for this is 15-11-2026. Juan and Maria are assigned to the backend."
      }
    },
    "extract_info": {
      "type": "information_extraction",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-4o",
        "schema": {
          "main_objective": { "type": "string", "description": "The main goal or objective mentioned" },
          "dead_line": { "type": "string", "description": "The deadline in DD-MM-YYYY format" },
          "people_assigned": { "type": "array", "items": { "type": "string" } }
        }
      }
    }
  },
  "edges": [
    {
      "from": "slack_message.output",
      "to": "extract_info.texts.slack_message"
    }
  ]
}
```

## 🔐 Secure Values (Valores Seguros)

Colmena v0.3.0 introduce **Secure Values**, un sistema para manejar secretos (API Keys, Tokens, Credenciales) de forma cifrada en la base de datos y memoria, evitando que se filtren en logs o interfaces.

### Conceptos Clave
- **Cifrado AES-256-GCM**: Todos los valores marcados como sensibles se cifran usando una clave maestra (`SECURE_VALUES_KEY`).
- **Inyección Automática**: El motor detecta valores cifrados (ej. `<secure_value_8>`) y los descifra justo antes de ejecutar un nodo.
- **Precedencia de Configuración**: Los valores en `inputs` (provenientes de edges) siempre tienen prioridad sobre los de `config`.

### Uso en el DAG
Para usar valores seguros, primero deben ser registrados en la base de datos (vía API o CLI). En el JSON del grafo, se referencian por su identificador único o se pasan a través de edges.

```json
{
  "from": "get_token.output",
  "to": "api_call.bearer_token"
}
```
Si `get_token` emite un valor sensible, el motor lo mantendrá cifrado en el estado del grafo y solo lo descifrará para el nodo `api_call`.

### Variable Obligatoria
Para que el motor arranque, debes definir:
```bash
SECURE_VALUES_KEY="tu-clave-base64-de-32-bytes"
```

## 🚀 Comandos de Ejecución

### Run Mode (Local Testing)

Ejecuta el grafo de forma secuencial y síncrona. Ideal para debugging.

#### Basic Execution

```bash
# Ejecutar un grafo simple
cargo run --bin dag_engine -- run tests/my_graph.json
```

#### Suspend/Resume Workflow

Si un grafo contiene un nodo `suspend`, la ejecución se pausa y devuelve un `session_id`:

**Paso 1: Ejecutar el grafo (se suspenderá)**
```bash
cargo run --bin dag_engine -- run tests/my_graph.json
```

Output:
```json
{
  "type": "finish",
  "finishReason": "suspended",
  "output": {
    "__colmena_status": "SUSPENDED",
    "question": "¿Apruebas continuar?",
    "session_id": "6d8928e5-e38c-49c3-a40b-16a1202055f3"
  }
}
```

**Paso 2: Reanudar con respuesta del usuario**
```bash
cargo run --bin dag_engine -- run tests/my_graph.json \
  --session-id 6d8928e5-e38c-49c3-a40b-16a1202055f3 \
  --answer "Sí, aprobado"
```

El nodo `suspend` recibe la respuesta como `__colmena_resume_answer` y continúa la ejecución del grafo.

**Nota:** `--resume-id` es un alias de `--session-id`. Ambos funcionan igual.

#### State Persistence

- El estado de ejecución (cola activa, outputs de todos los nodos) se persiste en PostgreSQL
- El `session_id` del output es tu token para reanudar
- No hay timeout hardcodeado; el estado persiste indefinidamente hasta que se reanude o se ejecute cleanup

### Serve Mode (Producción)
Levanta un servidor HTTP (Axum) que expone los endpoints definidos en los nodos `trigger_webhook`.
```bash
# Iniciar servidor en puerto 3000 (default)
cargo run --bin dag_engine -- serve tests/my_graph.json
```

## 🔍 Best Practices

1. **Usa `test_payload` para desarrollo**: Acelera el ciclo de desarrollo evitando levantar servidores.
2. **Configuración dinámica**: Aprovecha `inputs > config` para crear grafos más flexibles.
3. **Manejo de Secretos**: Nunca pongas API Keys reales en el JSON. Usa `${VAR_ENV}` o Secure Values.
4. **Validación de Esquema**: Cada nodo valida sus entradas. Si un nodo falla, revisa que los `edges` estén enviando el tipo de dato correcto (String vs Number).

## 📚 Más Información

- Ver [USAGE_EXAMPLES.md](../examples/USAGE_EXAMPLES.md) para más ejemplos completos
- Ver [SECURE_VALUES_IMPLEMENTATION.md](../SECURE_VALUES_IMPLEMENTATION.md) para detalles del cifrado
- Ver [DAG_ENGINE_DISEÑO.md](../dds/DAG_ENGINE_DISEÑO.md) para detalles de arquitectura

---

**Última actualización**: 2026-04-05
**Revisado por**: Auditoría Sistemática v0.3.0
