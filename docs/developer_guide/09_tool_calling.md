# Herramientas HTTP para agentes LLM

Los nodos `http_request` se pueden exponer como herramientas llamables por un agente LLM. El agente decide cuándo llamarlas y qué parámetros pasar.

## Comportamiento automático del engine al activar tools

Cuando se definen `enabled_tools` en un nodo `llm_call`, el engine inyecta automáticamente un bloque de instrucciones al final del `system_message` del usuario. Esto asegura que el LLM use las herramientas correctamente **sin que el usuario tenga que incluir estas instrucciones manualmente**.

El bloque inyectado tiene esta forma:

```
---
## Tool Use Instructions
You have access to the following tools:
- list_products
- search_products

Rules:
- ALWAYS use the available tools to answer questions that require real or live data. Never answer from your own knowledge when a tool can provide the data.
- Call the most relevant tool before responding. Do not skip tool calls.
- If a tool call fails, report the error clearly instead of guessing an answer.
- Only respond without a tool call when the user's request is purely conversational and no tool is needed.
```

**Implicaciones para el diseño de grafos:**
- El `system_message` del nodo solo necesita describir el rol y comportamiento del agente.
- La `description` de cada tool solo necesita explicar **qué hace** — el engine ya se encarga de cuándo y cómo usarla.
- Las instrucciones manuales como "ALWAYS call this tool" o "NEVER answer from memory" son redundantes y pueden omitirse.

---

## Activación por flag — `api_explorer`

`api_explorer` es el **único toolkit** hoy que puede activarse sin un entry en `tool_configurations`. Basta con incluir el alias literal `"api_explorer"` en `enabled_tools`:

```json
"agent": {
  "type": "llm_call",
  "config": {
    "provider": "openai",
    "model": "gpt-4o-mini",
    "api_key": "${OPENAI_API_KEY}",
    "enabled_tools": ["api_explorer"]
  }
}
```

Efecto:
1. El catálogo auto-expande las 5 sub-tools (`api_explorer__load_spec`, `__search_endpoint`, `__list_endpoints`, `__get_endpoint_details`, `__build_http_request`).
2. El filtro de `enabled_tools` trata cada entry como prefijo de toolkit (match exacto o `{alias}__*`), así que `"api_explorer"` habilita las 5.
3. El path de dispatch sintetiza una `ToolConfiguration` por defecto (`node_config: {}`, `expose_sub_tools: All`) cuando no hay entry explícito.

Cuándo usar el shortcut: la UI del frontend ofrece un toggle booleano único para activar OpenAPI tooling — sin config per-instance que mostrar. Cuándo NO: si necesitás alias custom (≠ `api_explorer`), filtrado de sub-tools (`expose_sub_tools: ["load_spec"]`), o knobs como `cache_ttl_seconds`/`fuzzy_match_threshold` — en esos casos declará un entry explícito en `tool_configurations`.

Otros toolkits (`tavily_client`, futuro `browser`) **siguen requiriendo** `tool_configurations` porque necesitan config per-instance (`api_key`, defaults).

Grafo de referencia verificado end-to-end con OpenAI gpt-4o-mini + spec real de Petstore: [`tests/graphs/web/api_explorer_petstore_flag_only.json`](../../tests/graphs/web/api_explorer_petstore_flag_only.json).

---

## Regla de oro: ¿qué enfoque usar?

```
¿Necesitas campos opcionales, tipos no-string, o body con objetos anidados?
  → USA node_schema  (el default para todo caso nuevo)

¿Son todos los campos planos, string, y obligatorios? ¿Son 1-5 campos máximo? ¿Sin body anidado?
  → PUEDES usar $DYNAMIC  (más simple de escribir, pero NO soporta nesting)
```

**En la duda: usa `node_schema`.** Es más explícito, soporta todos los casos, y es el estándar del proyecto.

---

## Enfoque 1: `node_schema` (default recomendado)

Un mapa plano donde cada clave es un campo del nodo (e.g. `base_url`, `method`, `query_params`, `body`, `bearer_token`). Cada entrada puede ser:

- **Fixed** (`"fixed": value`) — oculto al LLM, siempre aplicado tal cual.
- **LLM-visible** (sin `fixed`) — expuesto al LLM con tipo, descripción, y restricciones opcionales.
- **Contenedor** (`"type": "object"` + `"properties"`) — objeto con hijos que pueden ser fixed o LLM-visible individualmente.

### Ejemplo: GET con query params dinámicos

```json
"search_products": {
  "name": "search_products",
  "node_type": "http_request",
  "description": "Buscar productos por nombre y categoría.",
  "node_schema": {
    "base_url": { "type": "string", "fixed": "https://dummyjson.com" },
    "endpoint": { "type": "string", "fixed": "/products/search" },
    "method":   { "type": "string", "fixed": "GET" },
    "query_params": {
      "type": "object",
      "properties": {
        "q":     { "type": "string", "required": true,  "description": "Término de búsqueda" },
        "limit": { "type": "string", "required": false, "description": "Número máximo de resultados (default 10)" },
        "skip":  { "type": "string", "required": false, "description": "Offset para paginación" }
      }
    }
  }
}
```

### Ejemplo: POST con body dinámico

```json
"create_post": {
  "name": "create_post",
  "node_type": "http_request",
  "description": "Crear un nuevo post. Proporciona título y contenido.",
  "node_schema": {
    "base_url":  { "type": "string", "fixed": "https://jsonplaceholder.typicode.com" },
    "endpoint":  { "type": "string", "fixed": "/posts" },
    "method":    { "type": "string", "fixed": "POST" },
    "headers":   { "type": "object", "fixed": { "Content-Type": "application/json" } },
    "body": {
      "type": "object",
      "properties": {
        "userId":  { "type": "string", "fixed": "1" },
        "title":   { "type": "string", "required": true,  "description": "Título del post" },
        "content": { "type": "string", "required": true,  "description": "Contenido del post" },
        "tags":    { "type": "string", "required": false, "description": "Etiquetas separadas por coma" }
      }
    }
  }
}
```

**Grafos de referencia ejecutables:**
- `tests/graphs/agents/http_tool_node_schema_test.json` — GET + POST con body dinámico
- `tests/graphs/external/http_tool_configured.json` — GET con query params opcionales + POST con body
- `tests/graphs/external/product_sales_assistant.json` — agente completo con múltiples tools (multi-turno con SQLite)
- `tests/graphs/external/product_sales_dummyjson.json` — agente simple con una tool, demuestra inyección automática de instrucciones

---

## Enfoque 2: `$DYNAMIC` (solo casos muy simples)

> **`$DYNAMIC` NO soporta body anidado.** Si el body tiene objetos dentro de objetos (e.g. `body.metadata.author`), usa `node_schema`. `$DYNAMIC` solo escanea un nivel de profundidad.

> **Antes de usar `$DYNAMIC`, confirma que se cumplen TODAS estas condiciones:**
> - Todos los campos que el LLM debe rellenar son `string`
> - Todos son obligatorios (no hay opcionales)
> - El body es plano — ningún campo está dentro de un objeto anidado
> - Son 5 campos o menos
>
> Si falla cualquiera → usa `node_schema`.

Usa `fixed_config` normalmente pero marca valores específicos con el literal `"$DYNAMIC"`. El ejecutor detecta estos marcadores y los expone automáticamente como parámetros requeridos de tipo `string` para el LLM.

```json
"create_blog_post": {
  "name": "create_blog_post",
  "node_type": "http_request",
  "description": "Crear un post. Proporciona título y contenido.",
  "fixed_config": {
    "base_url": "https://jsonplaceholder.typicode.com",
    "endpoint": "/posts",
    "method": "POST",
    "headers": { "Content-Type": "application/json" },
    "body": {
      "userId": 1,
      "author": "Fulanito",
      "title": "$DYNAMIC",
      "content": "$DYNAMIC"
    }
  }
}
```

### ⚠️ Limitaciones de `$DYNAMIC`

| Limitación | Detalle |
|---|---|
| **Solo string, siempre requerido** | Todos los campos `$DYNAMIC` se exponen como `string` y se marcan `required`. No puedes tener campos opcionales ni de otro tipo. |
| **Solo 1 nivel de profundidad** | `body.title` funciona ✅. `body.metadata.author.name` NO funciona ❌ — el ejecutor solo escanea un nivel dentro de un contenedor. |
| **Sin patrones ni descripciones** | No puedes añadir restricciones de formato ni descripciones por campo. |

**Cuándo usar `$DYNAMIC`:** cuando tienes 1-5 campos planos y simples que el LLM debe rellenar, sin necesidad de tipos especiales, opcionales o anidamiento. Para todo lo demás, usa `node_schema`.

**Grafos de referencia ejecutables:**
- `tests/graphs/agents/http_tool_dynamic_placeholder_test.json`
- `tests/graphs/external/http_headers_dynamic.json`

---

## 🔒 Herramientas HTTP con Secure Values

Cuando una herramienta HTTP tiene `"secure": true` en su `fixed_config`, **`DagToolExecutor` aplica `hash_output()` ANTES de devolver el resultado al LLM**. Esto garantiza que el LLM nunca vea tokens reales.

### Flujo de seguridad en tools:

```
LLM llama get_amadeus_token
  ↓
HttpNode ejecuta POST → Amadeus responde {access_token: "real_xyz"}
  ↓
DagToolExecutor detecta secure: true
  ↓
hash_output() → {access_token: "<value_1>"}, encripta real en DB
  ↓
LLM recibe {access_token: "<value_1>"} ← nunca ve el token real ✅
  ↓
LLM llama search_flights con bearer_token: "<value_1>"
  ↓
DagToolExecutor llama inject_secrets() → reemplaza "<value_1>" con token real
  ↓
HttpNode ejecuta GET con Authorization: Bearer real_xyz ✅
```

### Ejemplo completo: LLM controla autenticación + búsqueda (patrón Amadeus)

Este patrón fue validado en `tests/graphs/agents/amadeus_llm_http_auth_experiment.json`:

```json
{
  "nodes": {
    "travel_agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "${OPENAI_API_KEY}",
        "system_message": "Para buscar vuelos: (1) llama get_amadeus_token, (2) usa el placeholder retornado en search_flights.",
        "enabled_tools": ["get_amadeus_token", "search_flights"],
        "tool_configurations": {
          "get_amadeus_token": {
            "name": "get_amadeus_token",
            "node_type": "http_request",
            "description": "Autenticar con Amadeus. Retorna un token como <value_1>.",
            "fixed_config": {
              "base_url": "https://api.amadeus.com/v1/security/oauth2",
              "endpoint": "/token",
              "method": "POST",
              "headers": { "Content-Type": "application/x-www-form-urlencoded" },
              "body": "grant_type=client_credentials&client_id=${AMADEUS_CLIENT_ID}&client_secret=${AMADEUS_CLIENT_SECRET}",
              "secure": true
            }
          },
          "search_flights": {
            "name": "search_flights",
            "node_type": "http_request",
            "description": "Buscar vuelos. Usar el placeholder del token en bearer_token.",
            "node_schema": {
              "base_url": { "type": "string", "fixed": "https://api.amadeus.com" },
              "endpoint": { "type": "string", "fixed": "/v2/shopping/flight-offers" },
              "method": { "type": "string", "fixed": "GET" },
              "bearer_token": {
                "type": "string",
                "required": true,
                "description": "Token obtenido de get_amadeus_token. Pasar el placeholder exacto."
              },
              "query_params": {
                "type": "object",
                "properties": {
                  "originLocationCode":      { "type": "string", "required": true },
                  "destinationLocationCode": { "type": "string", "required": true },
                  "departureDate":           { "type": "string", "required": true },
                  "adults":                  { "type": "string", "required": true }
                }
              }
            }
          }
        }
      }
    }
  }
}
```

**Requisitos:** `DATABASE_URL`, `SECURE_VALUES_KEY`, `AMADEUS_CLIENT_ID`, `AMADEUS_CLIENT_SECRET`, `OPENAI_API_KEY`.

```bash
set -a && source .env && set +a
cargo run --bin dag_engine -- run tests/graphs/agents/amadeus_llm_http_auth_experiment.json
```

---

## Tipos de Body en Herramientas HTTP

El `HttpNode` distingue body de query params **por la clave del campo**, no por el método HTTP:

| Clave | Comportamiento | Equivalente curl |
|---|---|---|
| `"body": "string"` | Raw text body (URL-encoded, GraphQL, etc.) | `--data-raw "..."` |
| `"body": { }` | JSON body automático | `-d '{...}'` con `Content-Type: application/json` |
| `"query_params": { }` | Query parameters en la URL | `?key=val&...` |
| `"bearer_token": "..."` | Header `Authorization: Bearer ...` | `-H "Authorization: Bearer ..."` |
| `"headers": { }` | HTTP headers arbitrarios | `-H "Key: Val"` |

> **Importante:** El `Content-Type: application/x-www-form-urlencoded` para OAuth2 debes setearlo explícitamente en `headers`. El nodo no lo infiere del formato del body string.

### Campos internos que NUNCA se envían como query params

La lista `reserved_keys` en `HttpNode` filtra estos campos del mecanismo de `extra_params`:

```
body, query_params, query_parameters, headers, base_url, endpoint, method,
bearer_token, authorization, secure, __colmena_session_id, __node_id, __colmena_resume_answer
```

> **Bug corregido (2026-04-05):** `"secure": true` fue añadido a `reserved_keys`. Antes se filtraba como `?secure=true` a APIs externas causando errores 400.

---

## Compatibilidad con frontends que generan UUIDs como keys

Los frontends suelen generar grafos donde el identificador de cada tool en `tool_configurations` y `enabled_tools` es un **UUID** en lugar del nombre semántico. El campo `name` dentro del objeto sí contiene el nombre legible.

El engine resuelve esto automáticamente en dos lugares:

**1. Generación del nombre de la tool para el LLM (`generate_tool_definition`):**
```
effective_name = tool_config.name  (si no está vacío)
             ↓ fallback
effective_name = key del mapa       (si name está vacío)
```
El LLM siempre recibe el nombre semántico (`"list_products"`), nunca el UUID.

**2. Resolución al ejecutar (`execute`):**
Cuando el LLM llama `"list_products"`, el engine busca primero por key del mapa y luego por `config.name`. Así el lookup funciona aunque la key sea un UUID.

**Resultado:** ambos formatos funcionan sin cambios:

```json
// Formato frontend (UUID como key) — funciona ✅
"enabled_tools": ["0618e7a1-2d50-4c7d-9244-52f2b504a3ca"],
"tool_configurations": {
  "0618e7a1-2d50-4c7d-9244-52f2b504a3ca": {
    "name": "list_products",
    ...
  }
}

// Formato manual (nombre semántico como key) — sigue funcionando ✅
"enabled_tools": ["list_products"],
"tool_configurations": {
  "list_products": {
    "name": "list_products",
    ...
  }
}
```

---

## Ejecución

Cuando el LLM decide usar una herramienta, `DagToolExecutor`:
1. Selecciona la estrategia: `node_schema` (si presente) → `$DYNAMIC` (si hay marcadores en `fixed_config`) → fallback deprecado.
2. Resuelve el nombre efectivo: usa `tool_config.name` si no está vacío, fallback a la key del mapa.
3. Mezcla los argumentos del LLM con los valores fijos según la estrategia seleccionada.
4. Llama `inject_secrets()` para reemplazar `<value_N>` con valores reales (si hay `SecureValueService`).
5. Ejecuta el nodo.
6. Si `secure: true`, llama `hash_output()` — el LLM recibe placeholders, nunca valores reales.
7. Devuelve el resultado (seguro) al LLM.

