# Nodos Web (tavily_client, api_explorer, browser)

Tres nodos toolkit que otorgan capacidades de internet a los agentes LLM:

- **tavily_client** — búsqueda y lectura web vía Tavily. Nodo toolkit expuesto como sub-herramientas `web__search` y `web__fetch`. Ver [Spec A](../superpowers/specs/2026-04-23-web-nodes-a-tavily-client-design.md) y §2 más abajo.
- **api_explorer** — descubrimiento de OpenAPI/Swagger y constructor de `http_request`. Ver [Spec C](../superpowers/specs/2026-04-23-web-nodes-c-api-explorer-design.md).
- **browser** — navegador headless auto-hospedado (Browserless + chromiumoxide). Ver [Spec B](../superpowers/specs/2026-04-23-web-nodes-b-browser-design.md).

## Runtime compartido: nodos toolkit

Un *nodo toolkit* es un `ExecutableNode` que también implementa `ToolkitNode` y expone
múltiples *sub-tools* al LLM desde una sola instancia de nodo.

Declaración en un `llm_call`:

```json
"tool_configurations": {
  "web": {
    "node_type": "tavily_client",
    "node_config": { "api_key": "${TAVILY_API_KEY}" },
    "expose_sub_tools": "all"
  }
}
```

- `node_type` debe apuntar a un nodo toolkit registrado.
- `node_config` es la configuración estática por instancia que el nodo recibe en el momento de la ejecución.
- `expose_sub_tools` es la cadena `"all"` o un arreglo con los nombres de sub-tools a exponer.

Flujo en runtime:

1. El engine invoca `ToolkitNode::sub_tool_catalog(&node_config)` para obtener la lista de sub-tools.
2. Cada sub-tool que pase el filtro de `expose_sub_tools` se convierte en un `ToolDefinition` con nombre `"{alias}__{sub_tool}"` (p. ej. `web__search`).
3. Cuando el LLM invoca uno, `DagToolExecutor` inyecta `__sub_tool` en los inputs del nodo, pasa `node_config` como config de ejecución y llama a `execute()` una sola vez.

## Sub-secciones

- [tavily_client](#2-tavily_client) — poblado por Spec A.
- [api_explorer](#3-api_explorer) — poblado por Spec C.
- [browser](#browser) — poblado por Spec B.

## 2. `tavily_client`

Herramienta LLM para búsqueda y lectura web vía Tavily. Expone dos sub-herramientas:

| Sub-tool | Propósito | Costo aproximado |
|---|---|---|
| `search` | Búsqueda web con snippets; opcionalmente contenido completo | 1 crédito (basic), 2 créditos (advanced), +1-3× con `include_content=true` |
| `fetch`  | Lee texto limpio de una URL concreta | 1 crédito por URL |

### Configuración mínima

```json
{ "type": "tavily_client", "config": { "api_key": "${TAVILY_API_KEY}" } }
```

### Uso desde un `llm_call`

```json
"tool_configurations": {
  "web": {
    "name": "web",
    "node_type": "tavily_client",
    "node_config": {
      "api_key": "${TAVILY_API_KEY}",
      "max_calls_per_run": 20,
      "search_defaults": { "include_domains": ["docs.aws.amazon.com"] }
    },
    "expose_sub_tools": "all"
  }
}
```

El LLM verá dos herramientas: `web__search` y `web__fetch`. Con `expose_sub_tools: ["fetch"]` sólo ve la segunda.

> **Nota:** declarar un alias en `tool_configurations` expone automáticamente sus sub-tools al LLM. No es necesario repetir los nombres en `enabled_tools`; si se incluyen en ambos lados, el engine los deduplica.

### Uso directo como nodo DAG (sin LLM)

El nodo también es ejecutable como cualquier otro `ExecutableNode`. Para invocar una sub-herramienta directamente desde un edge, inyecta la clave reservada `__sub_tool` en los inputs (`"search"` o `"fetch"`) junto a los parámetros que correspondan (`query` para búsqueda, `url` para fetch).

```json
{
  "nodes": {
    "start": {
      "type": "input",
      "config": {
        "data": {
          "__sub_tool": "search",
          "query": "What is the Rust programming language?"
        }
      }
    },
    "tavily": {
      "type": "tavily_client",
      "config": {
        "api_key": "${TAVILY_API_KEY}",
        "search_defaults": { "max_results": 3 }
      }
    },
    "sink": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "start",  "to": "tavily" },
    { "from": "tavily", "to": "sink" }
  ]
}
```

Ver `tests/graphs/web/tavily_direct_search.json`. El output del nodo es el mismo JSON que recibiría el LLM como tool result (`{ query, results, answer, credits_used }` para search; `{ url, content, ... }` para fetch).

### Manejo de errores

| Upstream | Para el LLM (recuperable) | Crash de DAG |
|---|---|---|
| 429 Too Many Requests | `{ error: "rate_limit", ... }` | Sólo si `fail_on_limit=true` |
| 5xx (tras reintentos) | `{ error: "upstream_error", status, ... }` | No |
| Timeout | `{ error: "timeout", ms, ... }` | No |
| 401 / 403 / llave vacía | — | Sí (`AdapterInit`) |
| Config inválida | — | Sí (`InvalidConfig`) |

### Caché y rate-limit

- Caché LRU + TTL por hash estable del request. Los hits no consumen el presupuesto por run.
- Contador por `dag_run_id` para imponer `max_calls_per_run` (search y fetch comparten contador).
- Los reintentos (5xx, timeout) usan backoff exponencial comenzando en `retry_policy.initial_backoff_ms`.

### Ejemplo completo

Ver `tests/graphs/web/tavily_search_basic.json` y `tests/graphs/web/tavily_fetch_article.json`.

## 3. `api_explorer`

Nodo toolkit que permite a un LLM descubrir endpoints en una especificación **OpenAPI 3.x** o **Swagger 2.0** y construir deterministicamente una configuración válida del nodo `http_request`. Los documentos Swagger 2.0 se convierten transparentemente a OpenAPI 3.0.3 dentro del adaptador, así que el código río abajo ve un único modelo.

### Activation (recommended)

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

That's it. Las 5 sub-tools aparecen en el catálogo del LLM automáticamente, con el prefijo fijo `api_explorer__*`:

| Sub-tool | Propósito |
|---|---|
| `api_explorer__load_spec` | Descarga + parsea + cachea una spec. Debe llamarse antes que las otras. |
| `api_explorer__list_endpoints` | Browse paginado por tag. |
| `api_explorer__search_endpoint` | Búsqueda fuzzy (path + summary + op_id + tags + description). |
| `api_explorer__get_endpoint_details` | Parámetros, request body, responses y security por endpoint. Resuelve `$ref` inline. |
| `api_explorer__build_http_request` | Emite un objeto JSON con la forma exacta del input del nodo `http_request`, con placeholders `${SECURE:<ref>}` para auth. |

Para el schema completo de cada sub-tool ver la sección [Sub-tools — full reference](#sub-tools--full-reference) más abajo.

Grafo de referencia verificado end-to-end (OpenAI gpt-4o-mini + spec real de petstore3.swagger.io): [`tests/graphs/web/api_explorer_petstore_flag_only.json`](../../tests/graphs/web/api_explorer_petstore_flag_only.json).

### Fallback: `tool_configurations` custom

Usar entry explícito solo cuando se necesite (a) alias custom, (b) filtrar sub-tools con `expose_sub_tools`, o (c) knobs como `cache_ttl_seconds` / `max_spec_size_bytes` / `fuzzy_match_threshold`:

```json
"tool_configurations": {
  "my_apis": {
    "node_type": "api_explorer",
    "expose_sub_tools": ["load_spec", "search_endpoint"],
    "node_config": {
      "cache_ttl_seconds": 3600,
      "fuzzy_match_threshold": 0.2
    }
  }
}
```

El LLM verá las sub-tools con el prefijo del alias: `my_apis__load_spec`, `my_apis__search_endpoint`. Schema canónico de `node_config` en [`docs/node_configurations.json`](../node_configurations.json) (clave `api_explorer`).

Knobs principales:

- **`cache_ttl_seconds`** — vida útil de una spec parseada (default 24 h); las hits aún frescas se revalidan vía ETag/If-None-Match (304 ≈ HEAD).
- **`max_spec_size_bytes`** — protege contra specs gigantescas (default 10 MiB). El adaptador aborta a mitad de stream si se cruza el límite.
- **`fuzzy_match_threshold`** — sube para ser más estricto con `search_endpoint`; baja para mejor recall. Default `0.1` porque la normalización divide el score crudo de nucleo por la longitud del haystack y para queries cortos (p. ej. `"add pet"`) sobre summaries largos los normalizados quedan bajos. Subir por encima de `0.5` solo si se ve ruido.

### Sub-tools — full reference

**`api_explorer__load_spec`** — descarga, parsea y cachea una spec.

| Param | Tipo | Required | Notas |
|---|---|---|---|
| `url` | string | yes | URL pública a la spec (OpenAPI 3.x JSON/YAML o Swagger 2.0). Git-forge URLs se auto-normalizan a raw. |
| `force_reload` | bool | no | default `false`. `true` invalida la entry de cache y vuelve a descargar. |

Devuelve: `{ spec_url_input, resolved_url, original_format, internal_format, title, version, description, server_url, endpoints_count, tags, security_schemes, cached }`.

**`api_explorer__list_endpoints`** — browse paginado.

| Param | Tipo | Required | Notas |
|---|---|---|---|
| `spec_url` | string | yes | URL de la spec previamente cargada (acepta `spec_url_input` o `resolved_url`). |
| `tag` | string | no | Filtra por tag de OpenAPI. |
| `limit` | int | no | 1-200, default 50. |
| `offset` | int | no | default 0. |

Devuelve: `{ endpoints: [{ operation_id, method, path, summary, tags }, ...], total }`.

**`api_explorer__search_endpoint`** — búsqueda fuzzy.

| Param | Tipo | Required | Notas |
|---|---|---|---|
| `spec_url` | string | yes | — |
| `query` | string | yes | Términos a buscar (path + summary + op_id + tags + description). |
| `method` | enum | no | `GET` / `POST` / `PUT` / `PATCH` / `DELETE` para filtrar. |
| `max_results` | int | no | 1-50, default 10. |

Devuelve: `{ query, results: [{ operation_id, method, path, summary, score }, ...] }`.

**`api_explorer__get_endpoint_details`** — schema completo de un endpoint.

| Param | Tipo | Required | Notas |
|---|---|---|---|
| `spec_url` | string | yes | — |
| `operation_id` | string | yes | El `operation_id` devuelto por `list_endpoints` o `search_endpoint`. Si no matchea, el error incluye `did_you_mean`. |

Devuelve: `{ operation_id, method, path, summary, path_params, query_params, request_body: { content_type, schema }, responses, security }` con `$ref` ya inlinados.

**`api_explorer__build_http_request`** — emite JSON listo para `http_request`.

| Param | Tipo | Required | Notas |
|---|---|---|---|
| `spec_url` | string | yes | — |
| `operation_id` | string | yes | — |
| `params` | object | no | Map plano de nombre→valor (path params, query params, y campos del request body se mezclan en este mismo objeto). Default `{}`. |
| `auth_secret_ref` | string | no | Referencia a un secret. El header de auth queda como `${SECURE:<ref>}` que `http_request` resuelve más tarde. |

Devuelve: `{ url, method, headers, query_params, body }` — la forma exacta del input del nodo `http_request`.

### Dispatch flow & data injection

```
LLM
 │ Sees 5 tools in catalog: api_explorer__{load_spec, list_endpoints,
 │ search_endpoint, get_endpoint_details, build_http_request}
 │
 │ Emits: { name: "api_explorer__load_spec", arguments: "{\"url\":\"...\"}" }
 ↓
[DagToolExecutor::execute_inner]
 │ split name on "__":   alias="api_explorer", sub_tool="load_spec"
 │ tool_configurations[alias]?
 │   ├ HIT  (legacy): use explicit ToolConfiguration
 │   └ MISS (flag-only): synthesise_default_toolkit_config(alias)
 │                       { node_type: alias, node_config: None,
 │                         expose_sub_tools: All, ... }
 │ → execute_toolkit(alias, sub_tool, &cfg, tool_call)
 ↓
[DagToolExecutor::execute_toolkit]
 │ inputs = parse(tool_call.arguments)     ← LLM's args
 │ inputs["__sub_tool"] = sub_tool         ← discriminator (ONLY key injected)
 │ exec_node.execute(&inputs, &node_config, &mut state, None)
 ↓
[ApiExplorerNode::execute]
 │ conversation_id = inputs["conversation_id"] ?? "default"
 │ match sub_tool { load_spec → handle_load_spec, ... }
 ↓
[ApiSpecUseCase::fetch_spec]
 │ key = SessionKey { conversation_id: "default", session_name: "api_explorer" }
 │ SessionRegistry<Arc<SpecCache>> lookup
 │   ├ HIT  → return (entry, cached=true)
 │   └ MISS → port.fetch_and_parse(url, etag, last_mod) → cache → (entry, cached=false)
 │
 │ Background TTL sweeper (15min idle / 1h max / 60s period) cleans the registry
 ↓
JSON response → LLM
```

Tabla de qué se inyecta a `inputs` antes de `execute()`:

| Field | Set by | Notes |
|---|---|---|
| `__sub_tool` | `DagToolExecutor::execute_toolkit` | Hardcoded discriminator |
| `url`, `query`, `params`, … (LLM args) | LLM | Lo que el modelo pasa en `tool_call.function.arguments` |
| `conversation_id` | **Not injected** | Falls back to `"default"` en `extract_conversation_id`. La cache es efectivamente process-wide |
| `__colmena_session_id` | **Not injected** by toolkit path | El non-toolkit dispatch SÍ inyecta este — el toolkit no |
| `__colmena_agent_session_id` | **Not injected** by toolkit path | Igual |

Consecuencia útil: como `conversation_id` cae a `"default"`, la cache de specs es **compartida entre conversaciones del mismo proceso** — no hay leak porque las specs OpenAPI son públicas e inmutables, y el cross-conversation sharing amortiza el costo de descarga/parseo. Si en el futuro se necesita aislamiento por conversación, hay que inyectar `conversation_id` desde el toolkit path.

### Cache lifecycle

Las specs cacheadas viven en un `SessionRegistry<Arc<SpecCache>>` indexado por `conversation_id + session_name`. Dentro del `SpecCache` hay un LRU per `spec_url` (default 100 entradas) con TTL controlado por `cache_ttl_seconds`. Cada instancia de `api_explorer` arranca un *sweeper* TTL pasivo al construirse dentro de un runtime tokio (`ApiExplorerNode::new()` invoca `SessionRegistry::start_sweeper`).

**Política de eviction** del registry exterior (`TtlConfig::default()`):

| Parámetro | Valor |
|---|---|
| Idle timeout (sin acceso) | 15 min |
| Max lifetime (desde creación) | 1 h |
| Max active sessions | 50 |
| Sweep period | 60 s |

**No hay señal eager de "conversación cerrada".** Los hosts (worker de ADP, CLI `dag_engine`) confían únicamente en (a) el sweeper pasivo y (b) la muerte del proceso — cuando el proceso worker termina, el engine y todos sus registries se van con él. ADP corre un servicio axum long-lived que procesa jobs de Redis uno a uno y no tiene noción de "conversación cerrada", así que un bus de lifecycle sin productor era código muerto (eliminado en commit `8a6a17a`, ver [CHANGELOG_2026-05.md](../CHANGELOG_2026-05.md)).

Si un host futuro adquiere la señal explícita (p. ej. un endpoint `/conversations/:id/close`), el patrón recomendado es exponer un helper de cleanup por nodo (`ApiExplorerNode::evict_conversation(conversation_id)`); no existe hoy — construirlo cuando aparezca el primer consumidor.

Cada `load_spec` indexa la spec **bajo dos llaves** dentro del `SpecCache`: el `input_url` que pasó el LLM y el `resolved_url` post-normalización (cuando difieren, p. ej. tras un rewrite de Git-forge). Esto evita que un sub-tool subsecuente que use cualquiera de las dos formas tenga que reabrir la red.

**Múltiples specs en paralelo:** el LRU per `SpecCache` permite N specs distintas (default 100). El LLM puede llamar `load_spec` con N URLs y luego pasar `spec_url` distinto a cada `list_endpoints`/`search_endpoint`/`get_endpoint_details` — las specs no se mezclan; cada query consulta exactamente la spec indicada. Útil para flujos como "navegar el catálogo de specs de HubSpot, luego cargar Contacts y Deals juntas".

### Lazy vs eager

Dos conceptos distintos — fácil de confundir:

- **Framework `lazy_tool_loading`: NO aplica.** El feature de `summary`/`eager` per-tool descrito en [`docs/developer_guide/29_lazy_tool_loading.md`](29_lazy_tool_loading.md) solo opera sobre entries de `tool_configurations` que declaran un `summary`. La activación flag-only de `api_explorer` no pasa por esa ruta — las 5 sub-tools aparecen **eagerly** en el catálogo inicial enviado al LLM en cada turno.
- **Application-level lazy spec exploration: SÍ, por diseño.** El propio diseño de `api_explorer` es lazy: el LLM llama `load_spec` (devuelve resumen compacto ~pocos KB), después `search_endpoint` (~5KB de matches), y solo entonces `get_endpoint_details` para el endpoint elegido (schema completo, posiblemente ~10-30KB). Así el LLM nunca paga el costo de inyectar una spec completa (100KB+) en el prompt — solo consume bytes de los endpoints que de verdad le interesan.

### Normalización de URL

El adaptador reescribe estos patrones de Git-forge antes de hacer la petición HTTP:

| Patrón de entrada | Reescrito a |
|---|---|
| `github.com/{o}/{r}/blob/{ref}/{p}` | `raw.githubusercontent.com/{o}/{r}/{ref}/{p}` |
| `github.com/{o}/{r}/tree/{ref}/{p}` | mismo que arriba |
| `gitlab.com/{o}/{r}/-/blob/{ref}/{p}` | `gitlab.com/{o}/{r}/-/raw/{ref}/{p}` |
| `bitbucket.org/{o}/{r}/src/{ref}/{p}` | `bitbucket.org/{o}/{r}/raw/{ref}/{p}` |

Hosts desconocidos pasan sin cambio. El LLM ve tanto `spec_url_input` (lo que pasó) como `resolved_url` (lo que efectivamente se descargó) en el resultado de `load_spec`. Si una URL devuelve HTML (forge sin reescribir), el nodo retorna `unexpected_html_response` con sugerencia de usar la URL raw.

### Conversión Swagger 2.0 → OpenAPI 3.0

Toda la conversión es Rust puro (`swagger2_to_oas3.rs`). Reglas principales:

| Swagger 2.0 | OpenAPI 3.0.3 |
|---|---|
| `swagger: "2.0"` | `openapi: "3.0.3"` |
| `host` + `basePath` + `schemes[]` | `servers: [{ url: "{scheme}://{host}{basePath}" }]` (uno por scheme) |
| `definitions` | `components.schemas` |
| `securityDefinitions` | `components.securitySchemes` |
| body param + `consumes` | `requestBody.content` |
| formData params | `multipart/form-data` o `x-www-form-urlencoded` |
| `collectionFormat: csv` | `style: form, explode: false` |
| `collectionFormat: multi` | `style: form, explode: true` |
| `collectionFormat: ssv` | `style: spaceDelimited` |
| `collectionFormat: pipes` | `style: pipeDelimited` |
| `collectionFormat: tsv` | **error** — no hay equivalente 3.0. |

### `build_http_request` — contrato de salida

El JSON devuelto coincide exactamente con el input del nodo `http_request`:

```json
{
  "url": "...",
  "method": "GET|POST|...",
  "headers": { ... },
  "query_params": { ... },
  "body": "...raw string..." | { ... } | { "__multipart": true, "fields": { ... } }
}
```

Los headers de auth usan el placeholder `${SECURE:<ref>}` que el nodo `http_request` resuelve más adelante. Los secretos en plano nunca entran en el contexto visible del LLM.

### Resolución inline de `$ref`

`get_endpoint_details` **resuelve inline** las referencias `{"$ref": "#/components/schemas/X"}` dentro de `request_body.schema` y `responses[].content[]`, reemplazándolas con el schema concreto de `components.schemas`.

| Caso | Resultado |
|---|---|
| Ref válida y no cíclica | Schema completo inlinado |
| Ciclo detectado (vía path-tracking) | `{"type": "object", "x-cycle-to": "X"}` |
| Ref desconocida | `{"type": "object", "x-unresolved-ref": "X"}` |

Por qué importa: (1) Gemini rechaza con 400 cualquier `function_response` que contenga strings empezando por `#/` — es su validador estricto interpretándolas como referencias a `display_name`s. (2) El LLM ve directamente la forma del schema sin tener que seguir referencias por separado.

### Manejo de errores

Recuperables (devueltos al LLM como JSON estructurado): `rate_limit`, `timeout`, `upstream`, `spec_parse_failed`, `unsupported_spec_format`, `endpoint_not_found` (con `did_you_mean`), `missing_required_params`, `invalid_param_type`, `missing_auth`, `spec_not_loaded`, `unexpected_html_response`, `swagger2_conversion_failed`.

Crashean el DAG: `InvalidConfig`, `AdapterInit`, `SpecTooLarge`.

### Ejemplos completos

- `tests/graphs/web/api_explorer_petstore_flag_only.json` — activación flag-only (recommended) verificada e2e contra OpenAI gpt-4o-mini + petstore3.swagger.io.
- `tests/graphs/web/api_explorer_petstore.json` — flow completo OpenAPI 3.0 con `tool_configurations` (alias custom).
- `tests/graphs/web/api_explorer_amadeus_swagger2.json` — ejercita el rewrite GitHub → raw + conversión Swagger 2.0 → OpenAPI 3.0.
- `tests/graphs/web/api_explorer_hubspot_conversation.json` — demo conversacional multi-turn contra el catálogo de specs de HubSpot (`web__fetch` para el índice + `apis__*` para cada sub-spec). Memoria persistente vía `${DATABASE_URL}` y `--session-id`.

### Uso conversacional (memoria multi-turn)

Para un agente que mantiene contexto entre invocaciones del grafo:

1. **Una sola** instancia del nodo `llm_call` con `session_id` + `connection_url` (Postgres recomendado).
2. Re-ejecutar el grafo cambiando el campo `nodes.<input>.config.default` en el JSON entre turnos. El flag `--session-id` debe matchear el del config para que la memoria persista.
3. La cache de specs en `SessionRegistry` vive en proceso, así que **se reinicia entre `cargo run`s**. El LLM, recordando del turno previo qué URLs cargó, simplemente vuelve a llamar `load_spec` (la red puede aprovechar ETags si el server los soporta).

### Referencia

Spec: [2026-04-23-web-nodes-c-api-explorer-design.md](../superpowers/specs/2026-04-23-web-nodes-c-api-explorer-design.md).
Implementación: `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`, `src/libs/colmena/src/web/application/api_spec_use_case.rs`, `src/libs/colmena/src/web/infrastructure/openapi_adapter.rs`, `src/libs/colmena/src/web/application/swagger2_to_oas3.rs`, `src/libs/colmena/src/web/application/url_normalizer.rs`.
