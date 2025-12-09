# 🛠️ Uso de Herramientas (Tool Calling)

Colmena permite a los agentes LLM utilizar "herramientas" para interactuar con el mundo exterior. Estas herramientas pueden ser nodos del DAG pre-configurados o funciones nativas.

## Configuración de Herramientas en el DAG

Puedes exponer nodos del DAG (como `http_request`) como herramientas para el LLM. Esto se hace mediante la configuración `tool_configurations` en el nodo `llm_call`.

### Ejemplo: Nodo HTTP como Herramienta

Este ejemplo muestra cómo configurar un nodo HTTP para que el LLM pueda "buscar usuarios" o "crear usuarios" sin conocer los detalles técnicos (URL base, headers, etc.).

```json
{
  "nodes": {
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-4o-mini",
        "system_message": "Eres un asistente útil. Usa las herramientas disponibles.",
        "enabled_tools": ["fetch_users", "create_user"],
        "tool_configurations": {
          "fetch_users": {
            "node_type": "http_request",
            "description": "Obtener datos de usuarios de la API. Puedes filtrar por estado.",
            "fixed_config": {
              "base_url": "https://jsonplaceholder.typicode.com",
              "endpoint": "/users",
              "method": "GET"
            },
            "exposed_inputs": ["query_parameters"]
          },
          "create_user": {
            "node_type": "http_request",
            "description": "Crear un nuevo usuario. Proporciona nombre, email y teléfono.",
            "fixed_config": {
              "base_url": "https://jsonplaceholder.typicode.com",
              "endpoint": "/users",
              "method": "POST",
              "headers": {
                "Content-Type": "application/json"
              }
            },
            "exposed_inputs": ["body"]
          }
        }
      }
    }
  }
}
```

### Cómo Funciona

1.  **Definición**: Defines una herramienta con un nombre (ej. `fetch_users`) y un tipo de nodo subyacente (`http_request`).
2.  **Configuración Fija (`fixed_config`)**: Estableces parámetros que el LLM no debe ver ni modificar (URL, método, headers de autenticación).
3.  **Inputs Expuestos (`exposed_inputs`)**: Indicas qué parámetros puede controlar el LLM (ej. `query_parameters`, `body`).
4.  **Generación de Schema**: Colmena genera automáticamente la definición de la herramienta (JSON Schema) para el LLM, mostrando solo los inputs expuestos y la descripción.

### Ventajas

*   **Seguridad**: El LLM no tiene acceso a credenciales ni URLs base.
*   **Simplicidad**: El LLM solo ve los parámetros relevantes para su tarea.
*   **Reutilización**: Puedes usar el mismo tipo de nodo (`http_request`) para múltiples herramientas distintas.

## Ejecución

Cuando el LLM decide usar una herramienta, Colmena:
1.  Intercepta la llamada.
2.  Combina los argumentos del LLM con la `fixed_config`.
3.  Ejecuta el nodo correspondiente.
4.  Devuelve el resultado al LLM para que continúe la conversación.
