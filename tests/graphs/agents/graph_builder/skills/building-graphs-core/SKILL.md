---
name: building-graphs-core
description: Use when assembling ANY Colmena graph JSON — the structural rules. Covers the nodes/edges anatomy, default vs explicit ports, ${ENV} for keys, entry (trigger_webhook/input) and output nodes, and the most common wiring gotchas.
---

# Cómo armar un grafo Colmena (reglas estructurales)

Un grafo es un objeto JSON con dos partes que importan: un mapa de **nodos** (cada
nodo hace una cosa) y una lista de **aristas** (cómo viaja la información de un nodo
a otro). Esta guía documenta la anatomía mínima y los errores más comunes. Todos los
ejemplos de abajo están copiados de grafos reales que corren — podés pegarlos tal cual
y adaptarlos.

---

## 1. Estructura de nivel superior

```json
{
  "timezone": "America/Bogota",
  "locale": "es-CO",
  "nodes": {
    "trigger": { "type": "trigger_webhook", "config": { "path": "/chat" } },
    "agent":   { "type": "llm_call", "config": { "...": "..." } },
    "out":     { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "trigger", "to": "agent" },
    { "from": "agent",   "to": "out" }
  ]
}
```

- `nodes` — un objeto donde **cada clave es el id del nodo** (`"trigger"`, `"agent"`,
  `"out"`). Ese id es como lo referenciás en las aristas. Cada nodo tiene `type`
  (el tipo de nodo) y `config` (sus parámetros).
- `edges` — una lista de objetos `{ "from": ..., "to": ... }`.
- `timezone` y `locale` — **opcionales**. Si los ponés, el contexto temporal/geográfico
  se inyecta automáticamente en cada `llm_call`.

> **`config` es obligatorio aunque esté vacío.** Si un nodo no necesita configuración
> (por ejemplo un `output`), igual escribí `"config": {}`. Omitir `config` rompe el grafo.

---

## 2. Aristas y puertos

Una arista mueve datos de un nodo a otro. El `from` y el `to` pueden escribirse de
dos formas:

- **Id pelado** (`"agent"`) → usa los **puertos por defecto** del nodo. Cada tipo de
  nodo declara un puerto de entrada y uno de salida por defecto (ej.: `llm_call` recibe
  por `prompt` y emite por `result`; `output` recibe por `input`).
- **`nodo.campo`** (`"trigger.message"`) → usa un **puerto explícito**. Lo usás cuando
  querés tomar/dejar un campo específico, o cuando el nodo no tiene puerto por defecto.

Ejemplo real con las dos formas mezcladas (de `graph_builder.json`):

```json
"edges": [
  { "from": "trigger.message", "to": "agent.prompt" },
  { "from": "agent",           "to": "out" }
]
```

- La primera arista es **explícita**: toma el campo `message` del webhook y lo entrega
  al puerto `prompt` del agente.
- La segunda es **por defecto**: `agent` emite por `result`, `out` recibe por `input`,
  y el motor los conecta solo.

**Regla práctica:** si dudás, escribí la arista explícita (`from: "A.campo"`,
`to: "B.campo"`) — nunca es ambigua.

---

## 3. Nodos de entrada y de salida

Todo grafo necesita **un punto de entrada** y debería **terminar en un nodo `output`**.

### Entrada A — `trigger_webhook` (grafos servidos / por evento)

Recibe datos de afuera (un webhook real o, en pruebas, un `test_payload`):

```json
"trigger": {
  "type": "trigger_webhook",
  "config": {
    "path": "/chat",
    "test_payload": { "message": "hola" }
  }
}
```

- `path` — la ruta donde se escucha (ej.: `/chat`, `/create_post`).
- `test_payload` — datos de prueba que el nodo emite al correr el grafo localmente,
  para no necesitar un webhook real.

### Entrada B — `input` (valores estáticos)

Cuando querés inyectar una constante o dato fijo, sin webhook:

```json
"request": {
  "type": "input",
  "config": { "data": 10 }
}
```

El nodo `input` emite el contenido de `config` por su puerto de salida (`output`).

### Salida — `output`

El nodo terminal que captura el resultado final del grafo:

```json
"out": { "type": "output", "config": {} }
```

---

## 4. Secretos: nunca pongas claves en el JSON

Las API keys y URLs de base de datos **nunca** se escriben literales en el grafo. Se
referencian con la sintaxis `${NOMBRE_DE_VARIABLE}`, y el motor las resuelve desde el
entorno en tiempo de ejecución:

| Variable | Para qué |
|---|---|
| `${OPENAI_API_KEY}` | proveedor OpenAI |
| `${GEMINI_API_KEY}` | proveedor Google (Gemini) |
| `${ANTHROPIC_API_KEY}` | proveedor Anthropic (Claude) |
| `${DATABASE_URL}` | memoria conversacional / Postgres |

```json
"config": {
  "api_key": "${GEMINI_API_KEY}",
  "connection_url": "${DATABASE_URL}"
}
```

---

## 5. Stack LLM por defecto

Salvo que haya una razón concreta para otra cosa, usá Google Gemini Flash — es rápido
y barato:

```json
"config": {
  "provider": "google",
  "model": "gemini-2.5-flash",
  "api_key": "${GEMINI_API_KEY}"
}
```

---

## 6. Tres esqueletos canónicos (copiá y adaptá)

### (a) Un solo `llm_call` — pregunta/respuesta

```json
{
  "nodes": {
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "system_message": "Sos un asistente claro y conciso.",
        "prompt": "¿Cuál es la capital de Colombia?"
      }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "agent", "to": "out" }
  ]
}
```

### (b) `llm_call` con una herramienta en `tool_configurations`

El agente decide cuándo llamar a una herramienta. Esta expone un `http_request` real
que crea un post. Fijate que los campos que el LLM **no** debe ver (`base_url`,
`endpoint`, `method`, `userId`, `author`) van como `"fixed"` dentro de `node_schema`;
los que el LLM **sí** completa (`title`, `content`) llevan `"required": true` y una
`description`.

```json
{
  "nodes": {
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-4o-mini",
        "system_message": "You are a blog post writer. Use the create_blog_post tool to create posts. The userId and author are always fixed. You provide the title and content. Tags are optional.",
        "tool_configurations": {
          "create_blog_post": {
            "name": "create_blog_post",
            "node_type": "http_request",
            "description": "Create a new blog post. You MUST provide title and content. Tags are optional.",
            "node_schema": {
              "base_url": { "type": "string", "fixed": "https://jsonplaceholder.typicode.com" },
              "endpoint": { "type": "string", "fixed": "/posts" },
              "method":   { "type": "string", "fixed": "POST" },
              "body": {
                "type": "object",
                "properties": {
                  "userId":  { "type": "string", "fixed": "1" },
                  "author":  { "type": "string", "fixed": "Fulanito" },
                  "title":   { "type": "string", "required": true, "description": "Post title (required)" },
                  "content": { "type": "string", "required": true, "description": "Post content (required)" },
                  "tags":    { "type": "string", "required": false, "description": "Comma-separated tags (optional)" }
                }
              }
            }
          }
        }
      }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "agent", "to": "out" }
  ]
}
```

### (c) `trigger_webhook` → `llm_call` → `output` (con memoria)

El esqueleto completo de un agente conversacional servido. La memoria conversacional
se activa con `session_id` + `connection_url` (Postgres).

```json
{
  "timezone": "America/Bogota",
  "locale": "es-CO",
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": { "path": "/chat", "test_payload": { "message": "hola" } }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "session_id": "mi_agente_session_001",
        "connection_url": "${DATABASE_URL}",
        "system_message": "Sos un asistente conversacional. Saludá y ayudá a la persona."
      }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "trigger.message", "to": "agent.prompt" },
    { "from": "agent", "to": "out" }
  ]
}
```

> **Memoria:** `session_id` agrupa los turnos de una misma conversación; `connection_url`
> apunta a Postgres vía `${DATABASE_URL}`. Sin estos dos campos el agente no recuerda
> nada entre turnos.

---

## 7. Errores comunes (gotchas)

1. **Falta `config`.** Cada nodo necesita la clave `config`, aunque sea `{}`. Omitirla
   rompe el grafo.
2. **Una arista apunta a un id que no existe.** El `from` y el `to` deben referenciar
   ids de nodos que estén realmente en `nodes`. Un typo (`"agnet"` en vez de `"agent"`)
   hace fallar el grafo.
3. **El `node_type` de una herramienta debe ser un tipo de nodo real registrado.**
   En `tool_configurations`, `node_type` tiene que ser un nodo que existe de verdad
   (`http_request`, `sql_query`, `subgraph`, `current_time`, etc.), no un placeholder
   como `log`. Si el nodo no existe, la herramienta no se ejecuta.
4. **Campos ocultos al LLM van en `node_schema` con `"fixed"`.** Todo lo que el modelo
   NO debe ver ni elegir (URLs base, métodos HTTP, ids fijos, claves) se declara como
   `"fixed"` dentro de `node_schema`. Solo los campos con `"required": true` o sin
   `fixed` quedan expuestos para que el LLM los complete.
