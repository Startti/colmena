---
name: capability-web-and-apis
description: Use when the user wants to search the internet or pull data from an external service/API. Covers tavily_client (web search), http_request (calling any API), and api_explorer (discovering endpoints of a known API).
---

# Capacidad: Web y APIs

Esta skill cubre las tres herramientas que permiten a un agente Colmena **mirar
hacia afuera**: buscar en internet, leer una página, o llamar a cualquier API
externa. Sirve para que tú —el agente constructor de grafos— elijas la
herramienta correcta y la configures con la forma exacta que espera el motor.

Para el cableado del grafo (cómo conectar un `trigger` a un `llm_call`, cómo
fluyen los datos entre nodos, dónde van `tool_configurations`) consulta
[[building-graphs-core]]. Esta skill solo documenta las tres herramientas de
web/APIs.

## Cuándo usar cada una

| El usuario quiere… | Herramienta | Por qué |
|---|---|---|
| "Busca en internet…", noticias, datos frescos, leer un artículo | `tavily_client` | Búsqueda y lectura web sin saber URLs de antemano. |
| Llamar a **una** API concreta cuyo endpoint ya conoces | `http_request` | Una sola llamada HTTP fija o parametrizada. |
| Integrar una API grande/desconocida (Stripe, HubSpot, Petstore…) | `api_explorer` | Descubre endpoints desde su OpenAPI/Swagger antes de construir la llamada. |

---

## 1. `tavily_client` — búsqueda y lectura web

Nodo *toolkit* que se expone al LLM como **dos sub-herramientas**:

| Sub-tool | Qué hace |
|---|---|
| `search` | Búsqueda web con snippets; opcionalmente contenido completo. |
| `fetch`  | Lee el texto limpio de una URL concreta. |

Cuando se expone con un alias `web`, el LLM ve las herramientas con el prefijo
del alias: **`web__search`** y **`web__fetch`**.

### Forma de `tool_configurations` (requiere `api_key`)

Copiada de un grafo real (`tests/graphs/web/tavily_llm_openai.json`). El campo
`api_key` es **obligatorio** — sin él el nodo no puede autenticarse contra
Tavily:

```json
"tool_configurations": {
  "web": {
    "name": "web",
    "description": "Web search and fetch via Tavily",
    "node_type": "tavily_client",
    "node_config": {
      "api_key": "${TAVILY_API_KEY}",
      "max_calls_per_run": 5,
      "search_defaults": { "max_results": 5 }
    },
    "expose_sub_tools": "all"
  }
}
```

Notas:
- `expose_sub_tools` es la cadena `"all"` o un arreglo con los nombres de
  sub-tools a exponer (p. ej. `["fetch"]` para que el LLM solo lea URLs y no
  busque).
- `api_key` se referencia como variable de entorno `${TAVILY_API_KEY}` — nunca
  pegues la clave en texto plano en el grafo.
- `max_calls_per_run` limita el gasto: `search` y `fetch` comparten el contador.

`tavily_client` es de **solo lectura** (no muta nada externo), así que es seguro
ejecutarlo en pruebas sin confirmación.

---

## 2. `http_request` — llamar a cualquier API

`http_request` sirve en dos modos:

### a) Como **nodo** del grafo (llamada fija)

Un nodo `http_request` normal hace una sola petición HTTP. Sus campos clave son
`base_url` + `endpoint` (la URL se arma como `base_url/endpoint`), `method`,
`headers` y `body`. **No existe un campo `url` único** — la dirección SIEMPRE se
parte en `base_url` (ej. `"https://api.hubapi.com"`) y `endpoint` (ej.
`"/crm/v3/objects/contacts"`); si pones todo en un solo `url` el nodo falla con
`Invalid URL ''`. Para autenticación tipo Bearer usá el campo dedicado
`bearer_token` (el nodo le antepone `"Bearer "` automáticamente — pasás solo el
token, sin el prefijo), **no** armes el header `Authorization` a mano. También
soporta subida de archivos por `multipart/form-data` (los archivos se transmiten
por streaming). Para el cableado de un nodo dentro del grafo, ver
[[building-graphs-core]].

### b) Como **herramienta** del LLM (el modelo decide cuándo llamarla)

Aquí usamos `node_schema` para fijar lo que el LLM **no** debe ver/cambiar
(`base_url`, `endpoint`, `method`) y dejar como `required` solo lo que el modelo
debe rellenar (`title`, `content`).

Bloque `create_blog_post` copiado **VERBATIM** de
`tests/graphs/agents/http_tool_node_schema_test.json`:

```json
"create_blog_post": {
  "name": "create_blog_post",
  "node_type": "http_request",
  "description": "Create a new blog post. You MUST provide title and content. Tags are optional.",
  "node_schema": {
    "base_url": {
      "type": "string",
      "fixed": "https://jsonplaceholder.typicode.com"
    },
    "endpoint": {
      "type": "string",
      "fixed": "/posts"
    },
    "method": {
      "type": "string",
      "fixed": "POST"
    },
    "body": {
      "type": "object",
      "properties": {
        "userId": {
          "type": "string",
          "fixed": "1"
        },
        "author": {
          "type": "string",
          "fixed": "Fulanito"
        },
        "title": {
          "type": "string",
          "required": true,
          "description": "Post title (required)"
        },
        "content": {
          "type": "string",
          "required": true,
          "description": "Post content (required)"
        },
        "tags": {
          "type": "string",
          "required": false,
          "description": "Comma-separated tags (optional)"
        }
      }
    }
  }
}
```

Cómo leer este bloque:
- `base_url`, `endpoint`, `method` → `"fixed"`: valores que el motor inyecta
  siempre; el LLM **no** los ve ni los puede cambiar.
- `title` y `content` → `"required": true`: el LLM **debe** rellenarlos.
- `tags` → `"required": false`: opcional.
- `userId` y `author` están `"fixed"` dentro de `body.properties`: plumbing
  estático que el modelo no toca.

### ⚠️ ADVERTENCIA DE EFECTOS SECUNDARIOS

Cualquier `http_request` con `method` **POST / PUT / PATCH / DELETE muta estado
remoto** — crea, modifica o borra datos en el servicio externo. A diferencia de
`GET` (solo lectura), ejecutar uno de estos métodos en una prueba puede crear
registros reales, sobrescribir datos o borrarlos de forma irreversible.

**Como agente constructor, antes de hacer test-run de un grafo que contenga un
`http_request` con POST/PUT/PATCH/DELETE, DEBES avisar al usuario y pedir
confirmación explícita.** Nunca ejecutes una mutación remota "para probar" sin
que el usuario lo sepa. Si solo necesitas validar la forma del grafo, prefiere
un endpoint de prueba (sandbox/dummy) o un `GET`.

---

## 3. `api_explorer` — descubrir endpoints de una API conocida

Cuando la API es grande o no conoces sus endpoints exactos, `api_explorer`
permite al LLM **descubrir** la API desde su especificación OpenAPI 3.x /
Swagger 2.0 y construir deterministicamente una configuración válida de
`http_request`.

### Activación solo-por-flag

`api_explorer` es un toolkit *flag-only*: basta con listarlo en `enabled_tools`,
sin necesidad de un `tool_configurations` por instancia.

```json
"enabled_tools": ["api_explorer"]
```

Eso es todo. Las **5 sub-tools** aparecen automáticamente en el catálogo del
LLM, con el prefijo fijo `api_explorer__*`:

| Sub-tool | Qué hace |
|---|---|
| `api_explorer__load_spec` | Descarga + parsea + cachea una spec. Debe llamarse **antes** que las otras. |
| `api_explorer__list_endpoints` | Browse paginado de endpoints por tag. |
| `api_explorer__search_endpoint` | Búsqueda fuzzy (path + summary + op_id + tags + description). |
| `api_explorer__get_endpoint_details` | Parámetros, request body, responses y security de un endpoint; resuelve `$ref` inline. |
| `api_explorer__build_http_request` | Emite un JSON con la forma exacta del input del nodo `http_request`, con placeholders `${SECURE:<ref>}` para auth. |

### Patrón típico

1. `web__search` (tavily) encuentra la URL de la spec OpenAPI.
2. `api_explorer__load_spec` la carga.
3. `api_explorer__search_endpoint` / `list_endpoints` / `get_endpoint_details`
   localizan el endpoint correcto.
4. `api_explorer__build_http_request` emite la config lista para `http_request`.

> **El placeholder `${SECURE:<ref>}` NO se escribe literal.** `build_http_request`
> lo emite como marcador de posición para la auth; vos DEBÉS reemplazarlo por el
> valor real antes de usar la config: en el grafo de PRUEBA, por el handle
> `<sv_...>` que devuelve `ask_secret` (en el campo `bearer_token`); en el grafo
> ENTREGADO, por la variable de entorno `${NOMBRE_DEL_TOKEN}`. Dejar
> `${SECURE:...}` tal cual hace que la llamada se autentique con un valor que no
> resuelve y la API responde 401.

> **Misma advertencia de efectos secundarios aplica:** si el endpoint que el LLM
> termina construyendo usa POST/PUT/PATCH/DELETE, el `http_request` resultante
> muta estado remoto — avisa y confirma antes de ejecutarlo en pruebas.

---

## Resumen para el constructor de grafos

- **Buscar/leer la web** → `tavily_client` (`web__search`, `web__fetch`),
  requiere `api_key`. Solo lectura, seguro de probar.
- **Una API que ya conoces** → `http_request` (nodo o tool con `node_schema`
  + `fixed`/`required`).
- **Descubrir una API grande** → `api_explorer` (`enabled_tools:
  ["api_explorer"]`, 5 sub-tools).
- **Siempre** advierte/confirma antes de test-run cuando el método HTTP sea
  POST/PUT/PATCH/DELETE.
- Para conectar estos nodos/tools dentro del grafo, ver [[building-graphs-core]].
