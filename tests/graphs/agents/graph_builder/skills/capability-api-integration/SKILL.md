---
name: capability-api-integration
description: Use when the user wants an agent to connect to an external HTTP API and points you at its documentation (a URL). Covers reading the docs, building an agent whose tools call the API, secure token handling, and testing against the real API.
---

# Capacidad: Integrar una API externa desde su documentación

Esta skill cubre el caso completo en el que el usuario quiere que su agente
**hable con una API externa** y te pasa la URL de su documentación. Tu trabajo
como agente constructor es: leer esa doc, armar un agente cuyas herramientas
llaman a la API, manejar el token de forma segura, y probar contra la API real.

El ejemplo de referencia a lo largo de esta skill es **HubSpot** (listar
contactos del CRM), pero el método aplica a cualquier API HTTP.

Para el cableado del grafo (cómo conectar los bloques con conexiones de bloque
completo, dónde van `tool_configurations`) consulta [[building-graphs-core]].
Para el detalle de las tres herramientas web/API (`http_request`,
`tavily_client`, `api_explorer`) consulta [[capability-web-and-apis]].

---

## 1. Leer la doc (enfoque híbrido)

Antes de construir nada, **lee la documentación** que te pasó el usuario. Tienes
tres herramientas y las combinas según lo que encuentres:

- **`leer_web`** (Tavily fetch) — descarga y extrae el texto limpio de una
  página de documentación. Úsala para docs en HTML/prosa.
- **`http_get`** — hace un GET directo a una URL. Úsala para traer specs o
  endpoints crudos (un `.json`/`.yaml` de OpenAPI, una respuesta de ejemplo).
- **`api_explorer`** — si existe un **spec OpenAPI 3.x / Swagger 2.0**, esta es
  la mejor vía. Se activa solo por flag con `enabled_tools: ["api_explorer"]`.
  Sus **5 sub-tools** (prefijo `api_explorer__*`):
  - `load_spec` — descarga, parsea y cachea la spec. Llámala **primero**.
  - `list_endpoints` — browse paginado de endpoints por tag.
  - `search_endpoint` — búsqueda fuzzy de un endpoint.
  - `get_endpoint_details` — parámetros, body, responses y security de un
    endpoint; resuelve `$ref` inline.
  - `build_http_request` — emite la config exacta para un nodo `http_request`.

**De la doc tienes que extraer tres cosas**, sí o sí:

1. **`base_url`** — la dirección base del servicio (ej. `https://api.hubapi.com`).
2. **El esquema de autenticación** — cómo se autentica (típicamente un token
   tipo Bearer en el header `Authorization`).
3. **Los endpoints relevantes** — solo los que sirven para lo que el usuario
   quiere lograr (no toda la API).

---

## 2. Armar el agente generado

El agente que entregas es un **`llm_call`** con esta forma:

- `"provider": "openai"`, `"model": "gpt-4o"`,
  `"api_key": "${OPENAI_API_KEY}"` — para APIs externas el agente generado usa
  OpenAI gpt-4o (no Gemini).
- Cableado simple de bloque a bloque: **`trigger → llm → output`**. La API
  **no** va en las conexiones; va en `tool_configurations` como herramienta(s).
- **Una herramienta `http_request` por operación** que el usuario necesita.

La dirección base, el endpoint, el método y la autenticación van **fijos**
(`"fixed"`); solo los parámetros que el LLM debe decidir quedan abiertos.

Bloque VERBATIM de la herramienta HubSpot "listar contactos" (variante
**ENTREGADA**):

```json
"tool_configurations": {
  "listar_contactos_hubspot": {
    "name": "listar_contactos_hubspot",
    "node_type": "http_request",
    "description": "Lista contactos de HubSpot (nombre, email, etc.).",
    "node_schema": {
      "base_url":     { "type": "string", "fixed": "https://api.hubapi.com" },
      "endpoint":     { "type": "string", "fixed": "/crm/v3/objects/contacts" },
      "method":       { "type": "string", "fixed": "GET" },
      "bearer_token": { "type": "string", "fixed": "${HUBSPOT_PRIVATE_APP_TOKEN}" },
      "query_params": { "type": "object", "properties": {
          "limit": { "type": "string", "required": false, "description": "Cuántos traer (máx 100)" }
      }}
    }
  }
}
```

Cómo leer este bloque:
- `base_url`, `endpoint`, `method`, `bearer_token` → `"fixed"`: el motor los
  inyecta siempre; el LLM no los ve ni los cambia.
- `query_params.limit` → `"required": false`: parámetro opcional que el LLM
  puede rellenar.
- Una operación distinta (crear contacto, buscar, etc.) = otra entrada en
  `tool_configurations` con su propio `endpoint`/`method`.

---

## 3. Auth segura (las dos variantes)

El token **nunca** entra al LLM ni queda escrito en el grafo en texto plano.
Hay dos variantes del campo `bearer_token` según para qué sea el grafo:

- **Grafo de PRUEBA (TEST):** `bearer_token` fijo = el **handle seguro** que
  devuelve la herramienta `ask_secret`, por ejemplo
  `<sv_hubspot_private_app_token_a3f2bc7d>`. Ese identificador es lo único que
  ves; nunca ves la clave real.
- **Grafo ENTREGADO (DELIVERED):** `bearer_token` fijo =
  `${HUBSPOT_PRIVATE_APP_TOKEN}` (variable de entorno, resuelta al salir hacia
  la API).

**Por qué dos variantes:** el handle `<sv_...>` está acotado por sesión y por
TTL, así que **no se puede entregar** — fuera de esa sesión deja de resolver.
El grafo entregado, en cambio, lee la variable de entorno en el host donde
corre. Por eso el agente debe **avisarle al usuario que tiene que setear esa
variable de entorno** (`HUBSPOT_PRIVATE_APP_TOKEN`) en la máquina/servicio que
ejecuta el agente.

Nota: `bearer_token` **antepone automáticamente `"Bearer "`** al valor, así que
solo pones el token, sin el prefijo.

---

## 4. Probar seguro

1. **Recolecta el token con `ask_secret`** — el usuario lo responde por un canal
   seguro, **nunca como mensaje de chat**. `ask_secret` te devuelve el handle
   `<sv_...>`.
2. **Hornea ese handle** en el `bearer_token` (fijo) del grafo de PRUEBA.
3. **Ejecuta con `probar_grafo`.** Necesitas un `agent_session_id` estable para
   que el alcance (scope) del secreto se propague dentro del subgrafo de prueba;
   sin un `agent_session_id` estable el handle no resuelve dentro del subgrafo.
4. **Solo operaciones de LECTURA por defecto.** Antes de probar cualquier
   operación que **escribe** (POST/PUT/PATCH/DELETE: crear, editar, borrar o
   enviar), **avisa y pide confirmación explícita** al usuario; si sigues, usa
   datos de prueba inocuos.

Una vez que la prueba da verde, entregas el grafo en su variante DELIVERED (con
`${HUBSPOT_PRIVATE_APP_TOKEN}`), no la de prueba.

---

## Referencias

- [[building-graphs-core]] — conexiones de bloque completo (los edges simples
  `trigger → llm → output`).
- [[capability-web-and-apis]] — detalle de `http_request`, `tavily_client` y
  `api_explorer`.
