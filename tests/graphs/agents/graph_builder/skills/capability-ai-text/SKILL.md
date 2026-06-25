---
name: capability-ai-text
description: Use when the user wants an AI to answer, write, summarize, classify or transform text. Covers the llm_call node — provider/model/api_key, system_message, and giving it tools.
---

# Capacidad: IA que trabaja con texto (`llm_call`)

Usá esta capacidad cuando la persona quiere que una IA **responda preguntas,
escriba, resuma, clasifique o transforme texto**. El nodo que hace todo esto es
`llm_call`: es el corazón de cualquier agente en Colmena.

> Para cómo se arma y se conecta un grafo (anatomía, nodos, edges, triggers),
> mirá [[building-graphs-core]]. Esta skill solo cubre el nodo `llm_call`.

---

## El nodo `llm_call`

Un nodo de tipo `llm_call` manda un prompt a un modelo de lenguaje y devuelve la
respuesta. Vive dentro de `nodes`, con su `type` y su bloque `config`.

### Campos de `config` que importan

Estos son los nombres **exactos** de los campos (no inventar otros):

| Campo            | Requerido | Qué es |
|------------------|-----------|--------|
| `provider`       | Sí        | Proveedor del modelo. Valores válidos: `"openai"`, `"google"`, `"anthropic"`, `"mock"`. |
| `api_key`        | Sí        | Clave del proveedor. Soporta sintaxis `${VAR}` para leer de variables de entorno. |
| `model`          | No        | Identificador del modelo (ej. `"gemini-2.5-flash"`). Si se omite, usa el default del proveedor. |
| `system_message` | No        | Instrucción de sistema: define el rol, el tono y las reglas de la IA. |

> El prompt del usuario (la pregunta o el texto a procesar) normalmente **no** se
> escribe en `config`: entra por un edge desde un nodo anterior (un `trigger` o un
> `input`) hacia el puerto `prompt` del `llm_call`. Ver [[building-graphs-core]].

---

## Stack por defecto (usalo siempre salvo que pidan otra cosa)

Cuando la persona no especifica proveedor ni modelo, usá Gemini Flash:

```json
{
  "provider": "google",
  "model": "gemini-2.5-flash",
  "api_key": "${GEMINI_API_KEY}"
}
```

Es rápido, barato y suficiente para casi todo. Solo cambiá de proveedor/modelo si
la persona lo pide explícitamente.

---

## El `system_message`: el alma del agente

El `system_message` es donde le decís a la IA **quién es y cómo se comporta**.
Sé concreto: rol, tono, qué hacer y qué NO hacer.

```json
"system_message": "Sos un asistente de atención al cliente amable y conciso. Respondé siempre en español, en máximo 3 frases. Si no sabés algo, decilo en vez de inventar."
```

---

## Darle herramientas a la IA con `tool_configurations`

Si la IA necesita **hacer algo** además de hablar (buscar en la web, consultar una
base de datos, ejecutar otro grafo), le das herramientas con el campo
`tool_configurations` dentro del `config` del `llm_call`. Cada entrada describe
una herramienta que el modelo puede decidir invocar durante su razonamiento.

```json
"tool_configurations": {
  "probar_grafo": {
    "name": "probar_grafo",
    "node_type": "subgraph",
    "description": "Ejecuta un grafo Colmena completo y devuelve su resultado real.",
    "node_schema": {
      "child_graph_inline": {
        "type": "object",
        "required": true,
        "description": "El grafo Colmena completo a ejecutar."
      }
    }
  }
}
```

El detalle fino de cada tipo de herramienta (web, SQL, HTTP, subgrafos, etc.) vive
en otras skills de capacidad. Aquí solo importa saber que **las herramientas se
enganchan en `tool_configurations`**.

---

## Ejemplo runnable: "responde preguntas"

Grafo mínimo de un solo `llm_call` que responde lo que le manden, usando el stack
Gemini por defecto. Flujo: `trigger` → `llm_call` → `output`.

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/preguntar",
        "test_payload": { "message": "¿Cuál es la capital de Francia?" }
      }
    },
    "responder": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "system_message": "Sos un asistente que responde preguntas de forma clara y breve, siempre en español."
      }
    },
    "out": { "type": "output" }
  },
  "edges": [
    { "from": "trigger.message", "to": "responder.prompt" },
    { "from": "responder", "to": "out" }
  ]
}
```

Qué hace cada parte:
- `trigger`: recibe el mensaje de la persona (campo `message` del payload).
- El edge `trigger.message → responder.prompt`: pasa ese mensaje como prompt.
- `responder`: el `llm_call` con el stack Gemini por defecto y un `system_message`.
- `out`: devuelve la respuesta de la IA.

Para entender por qué los edges se escriben así y cómo se nombran los puertos,
volvé a [[building-graphs-core]].
