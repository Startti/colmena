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

> **REGLA — las aristas son PELADAS por defecto, con UNA excepción: los routers.**
> Toda arista normal se escribe `{ "from": "A", "to": "B" }`, usando solo ids de
> nodo. La selección de campos **no** va en la arista: va dentro del `config` del
> nodo (con `{{templates}}` en `llm_call`) o en un adaptador `python_script`.
>
> **La ÚNICA excepción son los puertos de rama de un `router`.** Cuando ramificás el
> flujo con un router, las aristas de sus ramas DEBEN ir punteadas — `router.<rama>`,
> `router.input` y opcionalmente `router.__decision` — porque el motor rutea por
> nombre de puerto: solo la rama elegida emite payload, las demás emiten `null`. Una
> arista pelada desde un router haría disparar TODAS las ramas a la vez. Fuera de los
> routers, la forma punteada `nodo.campo` NO se usa.

Una arista pelada mueve datos del **puerto de salida por defecto** de `A` al
**puerto de entrada por defecto** de `B`. El motor resuelve los puertos solo:

- Cada tipo de nodo declara un puerto de salida por defecto (ej.: `llm_call` emite
  por `result`; `trigger_webhook` emite el payload completo) y uno de entrada por
  defecto (ej.: `output` recibe por `input`).
- **Auto-flatten:** si `B` no tiene puerto de entrada por defecto, el motor
  **desarma el objeto de salida de `A`** y mete cada clave como un input de `B`.
  Esto es lo que hace funcionar el patrón adaptador (sección 3).

### Cómo seleccionar un campo SIN arista punteada

Conectás el nodo entero con una arista pelada y leés el campo que te interesa
**dentro del `config`** con un `{{template}}`. Los `{{templates}}` en `llm_call`
referencian sus inputs inmediatos:

```json
"nodes": {
  "trigger": { "type": "trigger_webhook", "config": { "path": "/chat", "test_payload": { "message": "hola" } } },
  "agent":   { "type": "llm_call", "config": { "...": "...", "prompt": "{{message}}" } },
  "out":     { "type": "output", "config": {} }
},
"edges": [
  { "from": "trigger", "to": "agent" },
  { "from": "agent",   "to": "out" }
]
```

- La arista `trigger → agent` es **pelada**. El webhook emite su payload completo;
  el `llm_call` lee el campo `message` con `"prompt": "{{message}}"` en su `config`.
- La arista `agent → out` también es pelada: el motor conecta `result` → `input`.

**Regla práctica:** ¿Querés un campo específico? No toques la arista — leelo con
`{{campo}}` en el `config` del `llm_call`, o usá un adaptador `python_script`
(sección 3).

### Routers (ramificar el flujo) — la excepción punteada

Cuando el flujo tiene que desviarse por caminos distintos según el caso (ventas por
un lado, soporte por otro), usás un nodo `router`. Es el único nodo cuyas aristas van
punteadas: alimentás el router con una arista pelada (`{ "from": "trigger", "to":
"router" }`, que entra por `router.input`) y conectás **cada rama por su nombre**
(`{ "from": "router.<rama>", "to": "<destino>" }`). Opcionalmente leés la decisión
con un edge `router.__decision`. El ejemplo completo y los modos del router están en
[[capability-code-and-logic]].

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

### Adaptador `python_script` → alimentar nodos sin `{{templates}}`

Algunos nodos **no** soportan `{{templates}}` en su `config` — `http_request` solo
resuelve `${ENV}`, no `{{campo}}`. Para pasarle datos dinámicos con aristas peladas,
poné un `python_script` adaptador **antes** del nodo: su variable `output` debe ser
un **objeto**, y el motor lo **auto-flattenea** (sección 2) sobre la arista pelada,
metiendo cada clave (`base_url`, `endpoint`, `method`, …) como input del nodo destino.

Ejemplo runnable — `trigger → python_script → http_request → output`, todas peladas:

```json
{
  "nodes": {
    "trigger": { "type": "trigger_webhook", "config": { "path": "/run", "test_payload": { "ciudad": "Bogota" } } },
    "preparar": { "type": "python_script", "config": { "code": "output = { 'base_url': 'https://api.exemplo.com', 'endpoint': '/clima/' + ciudad, 'method': 'GET' }" } },
    "llamar": { "type": "http_request", "config": {} },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "trigger", "to": "preparar" },
    { "from": "preparar", "to": "llamar" },
    { "from": "llamar", "to": "out" }
  ]
}
```

- El `python_script` lee `ciudad` (del payload, auto-flatteneado del trigger) y arma
  el objeto `output`. Esas claves caen como inputs de `http_request` por auto-flatten.
- `http_request` no necesita `{{templates}}`: recibe `base_url`/`endpoint`/`method`
  ya resueltos por el adaptador.

> **Preferido para agentes:** si lo que querés es que un LLM decida cuándo y con qué
> datos llamar a `http_request` o `sql_query`, **no uses aristas ni adaptador** —
> exponé ese nodo como **herramienta** de un `llm_call` en `tool_configurations`
> (sección 6b). El agente completa los campos y el motor ejecuta la tool sin cablear
> ninguna arista.

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

### Secretos y APIs

Para autenticar contra una API (campo `bearer_token` de `http_request`) hay **dos
placeholders según el destino del grafo**:

- **Grafo de PRUEBA** (lo corrés vos localmente): usá un *secure handle* `<sv_...>`
  en `bearer_token` — un identificador que el motor resuelve contra el almacén de
  secure values.
- **Grafo ENTREGADO** (el que recibe la persona): usá `${ENV_VAR}` en `bearer_token`
  — la clave se inyecta desde el entorno en producción.

Detalle completo en [[capability-api-integration]].

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
        "system_message": "Sos un asistente conversacional. Saludá y ayudá a la persona.",
        "prompt": "{{message}}"
      }
    },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "trigger", "to": "agent" },
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
3. **Arista punteada (`nodo.campo`).** Por defecto las aristas son peladas
   (`{ "from": "A", "to": "B" }`); para tomar un campo, leelo con `{{campo}}` en el
   `config` del `llm_call`, o usá un adaptador `python_script` (sección 3). **La
   única excepción son los puertos de rama de un `router`** (`router.<rama>`,
   `router.input`, `router.__decision`): ahí el punteado es OBLIGATORIO para que el
   ruteo funcione (ver sección 2 y [[capability-code-and-logic]]).
4. **El `node_type` de una herramienta debe ser un tipo de nodo real registrado.**
   En `tool_configurations`, `node_type` tiene que ser un nodo que existe de verdad
   (`http_request`, `sql_query`, `subgraph`, `current_time`, etc.), no un placeholder
   como `log`. Si el nodo no existe, la herramienta no se ejecuta.
5. **Campos ocultos al LLM van en `node_schema` con `"fixed"`.** Todo lo que el modelo
   NO debe ver ni elegir (URLs base, métodos HTTP, ids fijos, claves) se declara como
   `"fixed"` dentro de `node_schema`. Solo los campos con `"required": true` o sin
   `fixed` quedan expuestos para que el LLM los complete.
