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
- [api_explorer](#api_explorer) — poblado por Spec C.
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
