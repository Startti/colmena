# QA — Nodo `tavily_client`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`

Fuentes de doc revisadas:
- `docs/node_configurations.json` (schema de inputs, config, outputs)
- `docs/node_as_tools_reference.json` (exposición como LLM tool)
- `docs/agent_context/node_ports_reference.md` (puertos y outputs)
- `docs/developer_guide/25_web_nodes.md` (guía de uso)

## 1) Config documentada NO soportada por el código

### S1.1: max_results silenciosamente ajustado fuera de rango

**Qué dice la doc:** En `node_configurations.json` y `node_as_tools_reference.json`, `max_results` es entero con descripción "Number of results (1-10). Default 5."

**Qué hace el código:** `tavily_client.rs:207` — `.clamp(1, 10)` ajusta valores fuera del rango sin notificar al LLM. Si el LLM pide 20 resultados, silenciosamente obtiene 10.

**Impacto QA:** El comportamiento es correcto (fail-safe), pero la documentación no advierte que valores fuera del rango [1,10] se ajustan automáticamente. QA debe verificar que un `max_results=0`, `=15`, `=-1` todos devuelven 10 (upper clamp) o 1 (lower clamp), según corresponda.

### S1.2: search_defaults sin time_range documentado

**Qué dice la doc:** `node_configurations.json` define `search_defaults` con propiedades: `search_depth`, `max_results`, `include_content`, `include_domains`, `exclude_domains`. NO menciona `time_range`.

**Qué hace el código:** `tavily_client.rs:233-243` — el handler `handle_search` lee `time_range` desde inputs Y desde defaults (fallback). El código soporta que `search_defaults.time_range` sea un valor de default válido.

**Impacto QA:** La documentación está incompleta. QA debe probar que `search_defaults: { "time_range": "week" }` en config es respetado cuando el LLM no lo override.

### S1.3: fetch extract_format fallback a markdown sin advertencia

**Qué dice la doc:** `node_configurations.json` enumera `extract_format` como `["markdown", "text"]`. No especifica qué sucede con valores inválidos.

**Qué hace el código:** `tavily_client.rs:281-284` — cualquier valor que no sea "text" es tratado como "Markdown" (no hay rechazo).

**Impacto QA:** El fallback es silent (fail-safe). QA debe verificar que `extract_format: "pdf"` es tratado como markdown sin error.

## 2) Código NO documentado

### S2.1: Session ID hardcoded a "default" para rate limiting

**Ubicación:** `tavily_client.rs:324` — `let session_id = "default";`

**Comportamiento:** Rate limiting (`max_calls_per_run`, `fail_on_limit`) es global por configuración, NO por `dag_run_id`. El comentario en línea 322-323 dice que `dag_run_id` no está wired through.

**Impacto:** Dos runs del mismo grafo comparten el contador de rate limit (o caché si `enable_cache=true`). No está documentado en ningún JSON.

**Recomendación QA:** Verificar que dos ejecuciones sucesivas del mismo grafo afectan mutuamente el rate limit (comparten `session_id="default"`).

### S2.2: Error JSON structures no documentados

**Ubicación:** `tavily_client.rs:447-489` (`format_llm_error`)

**Comportamientos no documentados:**

1. **RateLimit** (línea 462-471): estructura `{ "error": "rate_limit", "calls_used": N, "cap": N, "message": "..." }`. La doc solo menciona "rate_limit" como nombre de error.

2. **Timeout** (línea 473-477): estructura `{ "error": "timeout", "ms": N, "message": "..." }`. Doc no detalla.

3. **Upstream** (línea 478-483): estructura `{ "error": "upstream_error", "status": N, "retryable": false, "message": "..." }`. Doc no detalla.

4. **Generic error** (línea 484-487): estructura `{ "error": "web_error", "message": "..." }`.

5. **Non-recoverable errors**: lanzan `Box::new(e)` como `Err`, no JSON (línea 458-459). Esto causa fallo de DAG.

**Impacto QA:** El LLM ve estas estructuras pero la doc no especifica qué campos contienen. QA debe capturar SSE para verificar que la estructura es determinista.

### S2.3: Env var resolution para api_key

**Ubicación:** `tavily_client.rs:49-57` (`resolve_env_var`)

**Comportamiento:** Resuelve `${VAR}` a partir del ambiente del proceso. Literales que NO empiezan con `${` pasan intactos. Genera WARN si un literal empieza con `tvly-` (Tavily key prefix).

**Impacto:** No documentado que los literales `tvly-xxx` generan warn. QA debe verificar que un config `"api_key": "tvly-xxx"` loguea advertencia (búscar en logs stderr).

### S2.4: Secure value placeholder injection

**Ubicación:** `tavily_client.rs:72-79`

**Comportamiento:** Si `SecureValueService` está presente, inyecta placeholders `<value_N>` en una copia de config antes de leer `api_key`. El comentario (línea 75-77) menciona que no hay plumbing de `chat_handle`, por lo que siempre pasa `None`.

**Impacto:** No documentado que los placeholders `<value_N>` son soportados en `api_key`. El CLAUDE.md ya menciona secure values en general, pero este nodo específico no lo documenta.

### S2.5: Sub-tool resolution precedence

**Ubicación:** `tavily_client.rs:157-163` (`resolve_sub_tool`)

**Precedencia:** `inputs[__sub_tool]` → `inputs[sub_tool]` → `config[sub_tool]`. La doc en `node_ports_reference.md` lo menciona ("inyectados por el ejecutor"), pero `node_configurations.json` NO especifica explícitamente que `config.sub_tool` es una opción válida.

**Impacto QA:** Para un uso standalone (nodo DAG regular), el autor PUEDE poner `sub_tool` en config. Esto debe probarse.

### S2.6: Merge strategy para include_domains y exclude_domains

**Ubicación:** `tavily_client.rs:224-231, 429-445` (`merge_string_array`)

**Comportamiento:** Si inputs tiene el array, úsalo. Si no, retrocede a defaults. NO se unen — es precedencia simple.

**Impacto:** No documentado que los arrays NO se unen (inputs no extienden a defaults). QA debe verificar que `config.search_defaults.include_domains: ["a.com"]` + `inputs.include_domains: ["b.com"]` usa SOLO `["b.com"]`, no ambas.

### S2.7: Invalid input error structure (search/fetch missing required)

**Ubicación:** `tavily_client.rs:193-197` (search missing query), `270-274` (fetch missing url)

**Estructura:** `{ "error": "invalid_input", "message": "search requires `query` (string)" }`. Estos SON devueltos como OK JSON, no como `Err`.

**Impacto:** No documentado en ninguna fuente de doc. El LLM recibe estos JSONs como "success" (HTTP 200) no error.

### S2.8: Unknown sub_tool error

**Ubicación:** `tavily_client.rs:325-329`

**Comportamiento:** Si sub_tool no es "search" ni "fetch", retorna `Err(format!("tavily_client: unknown sub_tool '{other}'"))`. Esto FALLA el DAG (no JSON).

**Impacto:** No documentado que es un error fail-closed de DAG, no un JSON amigable al LLM.

## 3) Plan de pruebas QA

### Caso S3.1: Search con query básico (happy path)

**Objetivo:** Verificar que un search simple devuelve resultados con estructura correcta.

**Grafo JSON mínimo:**
```json
{
  "nodes": [
    {
      "id": "search_news",
      "node_type": "tavily_client",
      "config": {
        "api_key": "${TAVILY_API_KEY}"
      },
      "inputs": {
        "__sub_tool": "search",
        "query": "Rust programming language news"
      }
    }
  ],
  "edges": [],
  "provider": "google",
  "model": "gemini-2.5-flash"
}
```

**Comando:** `cargo run --bin dag_engine -- run /tmp/s3_1_search_basic.json --agent-session-id qa_001`

**Entrada:** Query string "Rust programming language news"

**Resultado esperado:** Output JSON con keys `query`, `results` (array), `credits_used`. Cada resultado tiene `title`, `url`, `snippet`, `score`.

**Verificación:** SSE capturada en `/tmp/colmena_e2e/s3_1.sse`. Buscar `node_output` con `results.length > 0`.

**Requisito:** TAVILY_API_KEY en env.

---

### Caso S3.2: Search max_results clamping a lower bound

**Objetivo:** Verificar que max_results < 1 es ajustado a 1.

**Entrada:** `max_results: 0`

**Resultado esperado:** La búsqueda se ejecuta pero Tavily API recibe max_results=1. Output contiene 1 resultado (o menos si Tavily falla).

**Verificación:** SSE: buscar en `node_output` que `results.length == 1`.

**Requisito:** TAVILY_API_KEY.

---

### Caso S3.3: Search max_results clamping a upper bound

**Objetivo:** Verificar que max_results > 10 es ajustado a 10.

**Entrada:** `max_results: 20`

**Resultado esperado:** Se ejecuta, Tavily recibe max_results=10. Output contiene ≤10 resultados.

**Verificación:** SSE: `results.length <= 10`.

**Requisito:** TAVILY_API_KEY.

---

### Caso S3.4: Search con include_content true

**Objetivo:** Verificar que include_content true devuelve campo `content` en cada resultado.

**Entrada:** `include_content: true`

**Resultado esperado:** Cada resultado en array tiene `content` field (puede ser null en algunos).

**Verificación:** SSE: buscar `results[0].content` presente.

**Requisito:** TAVILY_API_KEY. Nota: este call cuesta 2-9 créditos (más caro).

---

### Caso S3.5: Search con search_depth advanced

**Objetivo:** Verificar que search_depth: "advanced" es aceptado.

**Entrada:** `search_depth: "advanced"`

**Resultado esperado:** Búsqueda se ejecuta (el API de Tavily cuesta 2 créditos vs 1).

**Verificación:** SSE: sin error, `credits_used >= 2`.

**Requisito:** TAVILY_API_KEY.

---

### Caso S3.6: Search con include_domains

**Objetivo:** Verificar que los resultados provienen SOLO de dominios especificados.

**Entrada:** `include_domains: ["github.com"]`, `query: "rust"`

**Resultado esperado:** Todos los URLs en results tienen dominio github.com.

**Verificación:** SSE: todos `results[*].url` contienen "github.com".

**Requisito:** TAVILY_API_KEY.

---

### Caso S3.7: Search con exclude_domains

**Objetivo:** Verificar que resultados NO incluyen dominios excluidos.

**Entrada:** `exclude_domains: ["wikipedia.org"]`, `query: "rust"`

**Resultado esperado:** Ningún resultado tiene dominio wikipedia.org.

**Verificación:** SSE: ningún `results[*].url` contiene "wikipedia.org".

**Requisito:** TAVILY_API_KEY.

---

### Caso S3.8: Search con time_range day

**Objetivo:** Verificar que time_range: "day" restringe a resultados recientes.

**Entrada:** `time_range: "day"`, `query: "news"`

**Resultado esperado:** Search se ejecuta sin error.

**Verificación:** SSE: `error` ausente.

**Requisito:** TAVILY_API_KEY.

---

### Caso S3.9: Search_defaults heredados (max_results)

**Objetivo:** Verificar que config.search_defaults.max_results es usado si inputs.max_results está ausente.

**Grafo:**
```json
{
  "nodes": [
    {
      "id": "search_with_defaults",
      "node_type": "tavily_client",
      "config": {
        "api_key": "${TAVILY_API_KEY}",
        "search_defaults": { "max_results": 3 }
      },
      "inputs": {
        "__sub_tool": "search",
        "query": "test"
      }
    }
  ]
}
```

**Resultado esperado:** Búsqueda devuelve ≤3 resultados.

**Verificación:** SSE: `results.length <= 3`.

**Requisito:** TAVILY_API_KEY.

---

### Caso S3.10: Search_defaults con include_domains

**Objetivo:** Verificar que search_defaults.include_domains es usado como fallback.

**Config:**
```json
"search_defaults": {
  "include_domains": ["hn.algolia.com"]
}
```

**Entrada:** `query: "startup"`

**Resultado esperado:** Todos los URLs vienen de hn.algolia.com.

**Verificación:** SSE: todos `results[*].url` contienen "hn.algolia.com".

**Requisito:** TAVILY_API_KEY.

---

### Caso S3.11: Search_defaults input override

**Objetivo:** Verificar que inputs.max_results anula config.search_defaults.max_results.

**Grafo:**
```json
{
  "config": {
    "search_defaults": { "max_results": 2 }
  },
  "inputs": {
    "max_results": 8
  }
}
```

**Resultado esperado:** Búsqueda devuelve ≤8 resultados (NO 2).

**Verificación:** SSE: `results.length <= 8`.

**Requisito:** TAVILY_API_KEY.

---

### Caso S3.12: Fetch happy path

**Objetivo:** Verificar que fetch con URL devuelve contenido.

**Entrada:** `url: "https://docs.rs/serde/latest/serde/"`

**Resultado esperado:** Output JSON con keys `url`, `content`, `content_length`, `credits_used`.

**Verificación:** SSE: `content.length > 0`.

**Requisito:** TAVILY_API_KEY. URL debe ser válida.

---

### Caso S3.13: Fetch extract_format text

**Objetivo:** Verificar que extract_format: "text" devuelve texto plano.

**Entrada:** `url: "https://www.example.com"`, `extract_format: "text"`

**Resultado esperado:** Content es texto plano (sin markdown).

**Verificación:** SSE: `content` no contiene `#` (markdown header) ni `**` (bold).

**Requisito:** TAVILY_API_KEY.

---

### Caso S3.14: Fetch extract_format markdown (default)

**Objetivo:** Verificar que omitir extract_format usa markdown por defecto.

**Entrada:** `url: "https://www.example.com"`

**Resultado esperado:** Content puede contener markdown.

**Verificación:** SSE: sin error.

**Requisito:** TAVILY_API_KEY.

---

### Caso S3.15: Fetch invalid URL

**Objetivo:** Verificar que URL inválida o no alcanzable devuelve error estructurado.

**Entrada:** `url: "https://this-domain-does-not-exist-12345.com"`

**Resultado esperado:** Output es JSON con `error` key (upstream_error o similar).

**Verificación:** SSE: `error` field presente.

**Requisito:** TAVILY_API_KEY.

---

### Caso S3.16: Search missing query (invalid input)

**Objetivo:** Verificar que search sin query devuelve JSON estructurado (no falla DAG).

**Entrada:** `__sub_tool: "search"` (sin query)

**Resultado esperado:** Output JSON: `{ "error": "invalid_input", "message": "..." }`.

**Verificación:** SSE: sin `node_error`, sí `node_output` con error JSON.

**Requisito:** No requiere TAVILY_API_KEY (falla antes).

---

### Caso S3.17: Fetch missing url (invalid input)

**Objetivo:** Verificar que fetch sin url devuelve JSON estructurado.

**Entrada:** `__sub_tool: "fetch"` (sin url)

**Resultado esperado:** Output JSON: `{ "error": "invalid_input", "message": "fetch requires `url`..." }`.

**Verificación:** SSE: `node_output.error == "invalid_input"`.

**Requisito:** No requiere TAVILY_API_KEY.

---

### Caso S3.18: Unknown sub_tool error (fail-closed)

**Objetivo:** Verificar que __sub_tool inválido causa fallo de DAG.

**Entrada:** `__sub_tool: "invalid_tool"`

**Resultado esperado:** DAG falla con error "unknown sub_tool".

**Verificación:** SSE: `node_error` event con mensaje conteniendo "unknown sub_tool".

**Requisito:** No requiere TAVILY_API_KEY.

---

### Caso S3.19: Rate limit comportamiento (fail_on_limit false)

**Objetivo:** Verificar que exceder max_calls_per_run devuelve JSON error (no falla).

**Config:**
```json
{
  "api_key": "${TAVILY_API_KEY}",
  "max_calls_per_run": 1,
  "fail_on_limit": false
}
```

**Entrada:** Ejecutar search dos veces en el mismo run (via subgraph o loop).

**Resultado esperado:** Primera búsqueda OK. Segunda búsqueda devuelve JSON `{ "error": "rate_limit", ... }`.

**Verificación:** SSE: segundo `node_output.error == "rate_limit"`.

**Requisito:** TAVILY_API_KEY. Necesita grafo con loop o dos nodos search.

---

### Caso S3.20: Rate limit comportamiento (fail_on_limit true)

**Objetivo:** Verificar que fail_on_limit true causa fallo de DAG.

**Config:**
```json
{
  "api_key": "${TAVILY_API_KEY}",
  "max_calls_per_run": 0,
  "fail_on_limit": true
}
```

**Entrada:** Ejecutar search una vez (cap es 0).

**Resultado esperado:** DAG falla inmediatamente.

**Verificación:** SSE: `node_error` event con "rate".

**Requisito:** TAVILY_API_KEY.

---

### Caso S3.21: API key env var resolution

**Objetivo:** Verificar que ${TAVILY_API_KEY} es resuelto desde ambiente.

**Config:**
```json
{
  "api_key": "${TAVILY_API_KEY}"
}
```

**Entrada:** Env var `TAVILY_API_KEY=tvly-...` (válida), query "test"

**Resultado esperado:** Búsqueda se ejecuta sin error de clave inválida.

**Verificación:** SSE: `results` array presente.

**Requisito:** TAVILY_API_KEY debe existir en env.

---

### Caso S3.22: API key literal warning

**Objetivo:** Verificar que api_key literal tvly-xxx genera warn log.

**Config:**
```json
{
  "api_key": "tvly-abc123xyz"
}
```

**Entrada:** query "test"

**Resultado esperado:** DAG ejecuta, pero logs stderr contienen WARN mencionando "prefer ${TAVILY_API_KEY}".

**Verificación:** Capturar stderr: `grep -i "warn" stderr.log | grep tavily_client`.

**Requisito:** Clave inválida puede hacer que falle la búsqueda, pero el warn debe estar antes.

---

### Caso S3.23: Cache behavior

**Objetivo:** Verificar que cache es respetado (misma query en run 1 y 2 reutiliza).

**Config:**
```json
{
  "api_key": "${TAVILY_API_KEY}",
  "enable_cache": true,
  "cache_ttl_seconds": 3600
}
```

**Entrada:** 
- Run 1: query "test", esperar resultado
- Run 2 (mismo agent_session_id): query "test" nuevamente

**Resultado esperado:** Run 2 devuelve resultado identical sin credito additional (o con 0 credits_used si Tavily lo reporta).

**Verificación:** SSE de Run 1 vs Run 2: comparar `results` y `credits_used`.

**Requisito:** TAVILY_API_KEY. Dos ejecuciones del mismo grafo con `--agent-session-id` igual.

---

### Caso S3.24: Timeout behavior

**Objetivo:** Verificar que timeout_seconds causa error si la búsqueda demora.

**Config:**
```json
{
  "api_key": "${TAVILY_API_KEY}",
  "timeout_seconds": 1
}
```

**Entrada:** query que típicamente es lenta (ej: búsqueda muy amplia)

**Resultado esperado:** Output JSON con `error: "timeout"` y `ms` field.

**Verificación:** SSE: `error == "timeout"`.

**Requisito:** TAVILY_API_KEY. Difícil de reproducir; puede ser skip o usar mock si no se puede forzar timeout real.

---

### Caso S3.25: Standalone node vs tool path (sub_tool en config)

**Objetivo:** Verificar que un nodo DAG standalone puede fijar sub_tool en config.

**Grafo:**
```json
{
  "nodes": [
    {
      "id": "fetch_standalone",
      "node_type": "tavily_client",
      "config": {
        "api_key": "${TAVILY_API_KEY}",
        "sub_tool": "fetch",
        "url": "https://www.rust-lang.org"
      },
      "inputs": {}
    }
  ]
}
```

**Resultado esperado:** Nodo se ejecuta como fetch (no requiere inputs.__sub_tool).

**Verificación:** SSE: `node_output.url` es "https://www.rust-lang.org".

**Requisito:** TAVILY_API_KEY.

---

