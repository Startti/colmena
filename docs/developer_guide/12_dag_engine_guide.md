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
- `llm_call`: Llama a modelos de lenguaje (OpenAI, Gemini, Anthropic)

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
        "api_key": "sk-...",
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
        "api_key": "sk-...",
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

## 🚀 Comandos de Ejecución

### Run Mode (Local Testing)
```bash
# Ejecutar un grafo con test_payload
cargo run --bin dag_engine -- run tests/my_graph.json

# Ver el output completo
cargo run --bin dag_engine -- run tests/my_graph.json | jq
```

### Serve Mode (Production)
```bash
# Iniciar servidor en puerto 3000 (default)
cargo run --bin dag_engine -- serve tests/my_graph.json

# Iniciar servidor en puerto custom
cargo run --bin dag_engine -- serve tests/my_graph.json --port 8080
```

## 🔍 Best Practices

1. **Usa `test_payload` para desarrollo**: Acelera el ciclo de desarrollo evitando levantar servidores.
2. **Configuración dinámica**: Aprovecha `inputs > config` para crear grafos más flexibles.
3. **Modularidad**: Crea nodos pequeños y reutilizables.
4. **Error handling**: Siempre maneja errores apropiadamente en tus nodos.
5. **Testing**: Prueba con `run` antes de usar `serve`.

## 📚 Más Información

- Ver [USAGE_EXAMPLES.md](../examples/USAGE_EXAMPLES.md) para más ejemplos completos
- Ver [DAG_ENGINE_DISEÑO.md](../dds/DAG_ENGINE_DISEÑO.md) para detalles de arquitectura
- Ver [MODULO_LLM_DISEÑO.md](../dds/MODULO_LLM_DISEÑO.md) para integración con LLMs
