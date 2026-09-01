# QA — Nodo `api_explorer`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs`
Fuentes de doc revisadas:
- `docs/node_configurations.json` (líneas 2907-3019)
- `docs/node_as_tools_reference.json` (líneas 659-794)
- `docs/agent_context/node_ports_reference.md` (líneas 1129-1159)
- `docs/developer_guide/25_web_nodes.md` (líneas 132-404)

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas.

**Hallazgo aclaratorio (no es discrepancia):** El campo `enable_cache` en `node_configurations.json` (línea 2915) documenta que activa/desactiva caché, pero el código `ApiExplorerNode::new()` (líneas 40-64) siempre construye `SessionRegistry` e inyecta un sweeper pasivo sin una rama condicional sobre este campo. El nodo ignora silenciosamente `enable_cache=false` y cachea en todo caso. La doc debería aclarar que `enable_cache` es reservado para futuro uso; hoy siempre está en `true` de facto. Impacto para QA: cualquier test que intente `"enable_cache": false` obtendrá comportamiento de caché sin error — no es fail-closed, es degeneración silenciosa hacia el comportamiento por defecto.

## 2) Código NO documentado

**Campos de config:**
1. **`enable_cache` ignorado en tiempo de ejecución** (`api_explorer.rs:40-64`): El constructor nunca consulta este campo de la config — siempre inicia el registry y el sweeper. La descripción en `node_configurations.json:2915` promete que este campo controla el cacheo, pero el código es agnóstico a su valor. Remediar: documentar en node_configurations.json que este campo es **deprecated/future** y hoy sin efecto; o implementar la rama condicional en ApiExplorerNode::with_config (no existe hoy).

2. **`retry_policy.max_attempts` y `retry_policy.initial_backoff_ms` nunca usados** (`node_configurations.json:2966-2973`, código `api_explorer.rs`): El schema documenta estos campos dentro de `retry_policy`, pero no hay evidencia en `ApiExplorerNode` de que se lean o usen. El adaptador `OpenApiAdapter` / `ApiSpecUseCase` puede usar los valores, pero el nodo no los expone en su `schema()` (líneas 521-538) — la Promise a los consumidores es el schema que retorna `schema()`, no el JSON documentado. Remediar: verificar si `ApiSpecUseCase` o `OpenApiAdapter` de verdad leen estos campos en `node_config` (posible que esté delegado y sin bug, solo documentado a nivel node pero no surfaceado por `schema()`).

**Salida (outputs):**
- Documento especifica "envelope JSON específico por sub-tool" (`node_ports_reference.md:1152`), pero no enumera estructura exacta de cada error recuperable. El código define `format_spec_error()` (líneas 352-475) con casos como `spec_parse_failed`, `unexpected_html_response`, `swagger2_conversion_failed`, etc. Hallazgo: los códigos de error en el código SON documentados en la descripción de `format_spec_error()` (líneas 346-350), pero falta enumerarlos en `node_ports_reference.md` como tabla de salida de errores. Consultar `developer_guide/25_web_nodes.md:379-383` (sí está documentado ahí, aunque en sección "Manejo de errores").

**Comportamientos:**
- **`conversation_id` falls back a "default"** (`api_explorer.rs:100-106`): Doc de `node_as_tools_reference.json:674-681` ("data_injection") aclara que conversation_id NO se inyecta por toolkit path y cae a "default", pero `node_ports_reference.md:1140` dice simplemente "Default `"default"`." ambigüedad de quién lo seta. Aclaratorio: ya está documentado, solo que en dos lugares con énfasis distinto.

- **Parámetros `limit` y `max_results` se truncan (clamp)** (`api_explorer.rs:186-187`, `236-237`): El código restringe `limit` a [1, 200] y `max_results` a [1, 50], pero `node_configurations.json:2924-2928` y `2994` no explicitan el clamp — solo dicen "default". `developer_guide/25_web_nodes.md:206-207`, `218` sí lo documentan. Hallazgo: doc distribuida, `node_configurations.json` incompleta vs devguide. Remediar: agregar `validation: { min, max }` a `node_configurations.json`.

- **Método HTTP en `search_endpoint` se convierte a mayúsculas** (`api_explorer.rs:232`): `.to_ascii_uppercase()` pero `node_configurations.json` y node_as_tools_reference.json no mencionan normalización — solo dicen "Filter by HTTP method". Remediar: agregar en la descripción que valores se normalizan a mayúsculas (GET, POST, etc.).

**Seguridad / Internals:**
- **`secure_values` field nunca usado** (`api_explorer.rs:36-37, 87-90`): Campo `#[allow(dead_code)]` con comentario "used in Tasks 14-15" pero estos tasks/PRs no existen en el código visible. Campo se setea pero el código nunca lo consulta. Remediar: usar o remover.

## 3) Plan de pruebas QA

### Caso 1: Activación flag-only (recomendada)

**Objetivo:** Verificar que `enabled_tools: ["api_explorer"]` auto-expone las 5 sub-tools sin entry en `tool_configurations`.

**Grafo mínimo:**
```json
{
  "version": "1",
  "nodes": {
    "input": {
      "type": "input",
      "config": { "default": "Carga la especificación OpenAPI de https://petstore3.swagger.io/v3/openapi.json y lista todos los endpoints con tag 'pet'." }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "system_message": "Eres un asistente que explora APIs OpenAPI. Usa api_explorer para descubrir endpoints.",
        "enabled_tools": ["api_explorer"]
      }
    }
  },
  "edges": [
    { "from": "input", "to": "agent" }
  ]
}
```

**Entrada / Prompt:** `Carga la especificación OpenAPI de https://petstore3.swagger.io/v3/openapi.json y lista todos los endpoints con tag 'pet'.`

**Resultado esperado:**
- LLM recibe 5 tools con prefijo `api_explorer__`: `load_spec`, `list_endpoints`, `search_endpoint`, `get_endpoint_details`, `build_http_request`.
- LLM llama `api_explorer__load_spec` con `{"url": "https://petstore3.swagger.io/v3/openapi.json", "force_reload": false}`.
- Nodo retorna resumen con `cached: false, endpoints_count: <N>, tags: ["pet", ...]`.
- LLM llama `api_explorer__list_endpoints` con `{"spec_url": "https://petstore3.swagger.io/v3/openapi.json", "tag": "pet"}`.
- Nodo retorna paginado con endpoints que llevan `tags: ["pet"]`.

**Cómo verificar:** Inspeccionar SSE; verificar que todas las 5 sub-tools aparecen en `tool-choice` o equivalente al inicio; verificar output JSON de cada tool contra schema esperado.

---

### Caso 2: Sub-tool `load_spec` — URL Git-forge reescrita

**Objetivo:** Verificar reescritura automática de GitHub/GitLab/Bitbucket blob URLs a raw.

**Grafo mínimo:**
```json
{
  "version": "1",
  "nodes": {
    "input": {
      "type": "input",
      "config": { "default": "load_spec_github" }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "enabled_tools": ["api_explorer"]
      }
    },
    "output": {
      "type": "output"
    }
  },
  "edges": [
    { "from": "input", "to": "agent" },
    { "from": "agent", "to": "output" }
  ]
}
```

**Entrada / Prompt:** `Carga esta spec de GitHub (blob): https://github.com/OAI/OpenAPI-Specification/blob/main/examples/v3.0/petstore.yaml`

**Resultado esperado:**
- LLM invoca `api_explorer__load_spec`.
- Nodo reescribe `github.com/{org}/{repo}/blob/{ref}/{path}` → `raw.githubusercontent.com/{org}/{repo}/{ref}/{path}`.
- Output contiene `resolved_url: "https://raw.githubusercontent.com/..."`.
- `spec_url_input` = entrada original; `resolved_url` = reescrita.
- No hay error `unexpected_html_response`.

**Cómo verificar:** Capturar SSE; verificar campos `spec_url_input` vs `resolved_url` en el output JSON.

---

### Caso 3: Sub-tool `search_endpoint` — fuzzy matching + method filter

**Objetivo:** Verificar búsqueda fuzzy sobre path/summary/operationId/tags y filtro por método HTTP.

**Entrada:**
- Previamente cargada spec petstore con `load_spec`.
- `search_endpoint` con `spec_url`, `query: "get all pets"`, `method: "GET"`, `max_results: 5`.

**Resultado esperado:**
- Retorna array `results[]` con max 5 items, cada uno con `{ operation_id, method, path, summary, score, match_reason }`.
- Todos los items tienen `method: "GET"` (filtro aplicado).
- Items rankeados por `score` (descendente).
- Ejemplo: `operation_id: "listPets"` está arriba porque "listPets" contiene fuzzy-match de "get all pets".

**Cómo verificar:** Inspeccionar array `results[]` en output JSON; verificar count <= 5, todos con `method === "GET"`, scores ordenados desc.

---

### Caso 4: Sub-tool `list_endpoints` — pagination límites

**Objetivo:** Verificar que `limit` se trunca a [1, 200] y `offset` funciona.

**Entrada:**
- Spec petstore cargada.
- `list_endpoints` con `limit: 500` (excede máximo), `offset: 0`.

**Resultado esperado:**
- Retorna con `returned: min(50, endpoints_count)` (50 es default cuando limit excede máximo documentado, pero código clamp a 200 así que si limit=500 debería usarse 200 ó default 50).
- Verificar que `limit` se respeta: si el spec tiene 30 endpoints y pasamos `limit: 500`, retorna max 30 (no 200, no 500).

**Cómo verificar:** Pasar `limit: 999`, verificar que `returned` no supera min(999, endpoints_count, 200); luego pasar `limit: 50, offset: 10` y verificar que skip los primeros 10.

---

### Caso 5: Sub-tool `build_http_request` — auth con `${SECURE:...}` placeholder

**Objetivo:** Verificar que `auth_secret_ref` se interpola como placeholder para `http_request`.

**Entrada:**
- Spec petstore con security scheme "api_key" en header "X-API-Key".
- `build_http_request` con `operation_id: "listPets"`, `params: {}`, `auth_secret_ref: "petstore_api_key"`.

**Resultado esperado:**
- Retorna JSON con `headers: { "X-API-Key": "${SECURE:petstore_api_key}" }`.
- No hay valor real del secret en el output (placeholder sin resolver).
- Output tiene `method`, `url`, `query_params`, `body` con forma exacta de input del nodo `http_request`.

**Cómo verificar:** Inspeccionar headers en output JSON; verificar que ningún campo contiene valor sensible.

---

### Caso 6: Sub-tool `build_http_request` — `params: null` o absent

**Objetivo:** Verificar comportamiento cuando `params` no se proporciona o es null.

**Entrada:**
- Spec petstore.
- `build_http_request` con `operation_id: "listPets"`, omitir `params`.

**Resultado esperado:**
- Si endpoint no requiere parámetros: output exitoso con `query_params: {}`, `body: null`.
- Si endpoint requiere parámetros: error JSON `{ error: "missing_required_params", missing: ["param1", ...], example_params: {...} }`.

**Cómo verificar:** Capturar output; si error, verificar estructura `missing[]` y `example_params`.

---

### Caso 7: Sub-tool `load_spec` — fuerza recarga

**Objetivo:** Verificar que `force_reload: true` invalida caché y reaccede a la red.

**Entrada:**
- Llamar `load_spec` dos veces con misma URL y `force_reload: false` (default).
- Tercera llamada con `force_reload: true`.

**Resultado esperado:**
- Primera: `cached: false`, puerto HTTP accedido (observable si se loguea).
- Segunda: `cached: true` (mismo proceso, caché en RAM).
- Tercera con `force_reload: true`: `cached: false` nuevamente, puerto accedido otra vez.

**Cómo verificar:** Pasar `--agent-session-id "sess-7"` para mantener caché entre `cargo run` invocaciones (si BD configurada) o dentro del mismo run; inspeccionar `cached` field.

---

### Caso 8: Sub-tool `get_endpoint_details` — `did_you_mean` hint

**Objetivo:** Verificar que operationId erróneo retorna lista de sugerencias.

**Entrada:**
- Spec petstore cargada.
- `get_endpoint_details` con `operation_id: "listPet"` (typo, correcto es "listPets").

**Resultado esperado:**
- Error JSON con `error: "endpoint_not_found"`, `searched_for: "listPet"`, `did_you_mean: ["listPets", ...]`.

**Cómo verificar:** Inspeccionar array `did_you_mean[]`, verificar que "listPets" está en la lista.

---

### Caso 9: Swagger 2.0 conversion

**Objetivo:** Verificar que Swagger 2.0 se convierte transparentemente a OpenAPI 3.0.

**Entrada:**
- `load_spec` con spec Swagger 2.0 (ej: Amadeus API histórica).
- URL: `https://raw.githubusercontent.com/AeroDataLabs/api-specs/main/specs/amadeus-openapi.yaml` (o similar).

**Resultado esperado:**
- `original_format: "swagger-2.0"` (o `"swagger-2.0-yaml"`).
- `internal_format: "openapi-3.0.3"`.
- `endpoints_count: <N>`.
- Sin error.
- Subsecuentes `list_endpoints`/`search_endpoint`/`get_endpoint_details` funcionan igual que con OpenAPI 3.x.

**Cómo verificar:** Inspeccionar `original_format` vs `internal_format` en output; llamar `list_endpoints` después y verificar éxito.

---

### Caso 10: Error `unexpected_html_response`

**Objetivo:** Verificar que un blob URL de Git-forge que NO se reescribe correctamente retorna error helpful.

**Entrada:**
- `load_spec` con una URL que retorna HTML (simular pasando URL a página GitHub no normalizada).
- Ej: `https://github.com/some/repo/blob/main/openapi.json` sin auto-reescritura (si el rewriter falla o desconoce el patrón).

**Resultado esperado:**
- Error JSON `{ error: "unexpected_html_response", url_given: "...", resolved_url: "...", message: "Use the raw content URL instead" }`.
- Sin crash del DAG.

**Cómo verificar:** Registrar la respuesta de error; verificar estructura y mensaje sugerente.

---

### Caso 11: Caché TTL (proceso long-lived)

**Objetivo:** Verificar que sweeper pasivo evicta especificaciones después de idle/lifetime.

**Contexto:** Requiere modo `serve` (long-lived) o múltiples `cargo run` con `--agent-session-id` estable + DB persistida.

**Entrada:**
- Primera invocación: `load_spec` con URL → `cached: false`.
- Segunda (mismo process / mismo agent_session_id, dentro de 15 min): `load_spec` misma URL → `cached: true`.
- Esperar 15 min + enviar tercera invocación: `load_spec` misma URL → `cached: false` (sweeper evictó por idle).

**Resultado esperado:**
- Transición de `cached: true` → `cached: false` después de timeout idle.

**Cómo verificar:** Timestamps de logs + valor `cached` en outputs; o acceso a directorio de BD si usa Postgres.

---

### Caso 12: Flag-only vs explicit tool_configurations con filtering

**Objetivo:** Verificar que `expose_sub_tools` en entry explícita filtra el catálogo de tools.

**Entrada (tool_configurations con filtro):**
```json
"tool_configurations": {
  "apis": {
    "node_type": "api_explorer",
    "expose_sub_tools": ["load_spec", "build_http_request"],
    "node_config": { "fuzzy_match_threshold": 0.3 }
  }
}
```

**Resultado esperado:**
- LLM ve solo 2 tools: `apis__load_spec`, `apis__build_http_request`.
- Tools `list_endpoints`, `search_endpoint`, `get_endpoint_details` NO están disponibles.
- Custom threshold 0.3 se respeta (si LLM llamara `search_endpoint` directamente, fallaría; pero como no está expuesto, irrelevante).

**Cómo verificar:** Inspeccionar catálogo de tools en SSE inicial; contar que son exactamente 2.

---

### Caso 13: Config field que ignora (`enable_cache: false`)

**Objetivo:** Verificar comportamiento cuando `enable_cache: false` (hoy ignorado).

**Entrada:**
```json
"node_config": {
  "enable_cache": false,
  "cache_ttl_seconds": 3600
}
```
Llamar `load_spec` dos veces con misma URL.

**Resultado esperado (hoy):**
- `cached: false` (primera), `cached: true` (segunda) — el campo `enable_cache` es ignorado y se cachea anyway.
- QA debe registrar como **LOW severity**, "degeneración silenciosa"; no es fail-closed ni error.

**Remediar:** Documentar que `enable_cache` es deprecated o sin efecto.

**Cómo verificar:** Inspeccionar `cached` field; si es `true` en segunda llamada, comportamiento es cache-always (no honra `enable_cache: false`).

