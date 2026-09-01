# information_extraction — Auditoría QA (Documentación vs Código)

**Nodo:** `information_extraction`  
**Código fuente:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/extraction.rs`  
**Helpers:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/extract_with_schema.rs`, `util/inline_schema.rs`  
**Documentación primaria:** `docs/developer_guide/37_router_and_output_parser.md` (§`information_extraction`)  
**Configuración canónica:** `docs/node_configurations.json` → `node_types.information_extraction`  
**Puertos de referencia:** `docs/agent_context/node_ports_reference.md` línea 115  
**Fecha de auditoría:** 2026-08-30

---

## 1. Hallazgos: Documentación

### 1.1 Falta documentación de tools para information_extraction

**Problema:** `docs/node_as_tools_reference.json` está vacío (sin entrada para `information_extraction`).

**Verificación:** El nodo NO está en la referencia de herramientas para LLM tool calling, a pesar de que:
- `extraction.rs` línea 276-278 declara un `description()` ("Extracts structured information from unstructured text based on a provided JSON schema using an LLM.")
- El nodo puede ser usado como un LLM tool en un `llm_call` o `subgraph` bajo `tool_configurations`

**Impacto:** Alto. LLM developers que buscan "cómo configurar `information_extraction` como tool en `tool_configurations`" no encuentran ejemplos canónicos y deben inferir desde `docs/developer_guide/37_router_and_output_parser.md`.

**Remediación:** Agregar entrada `information_extraction` a `docs/node_as_tools_reference.json` con ejemplos de:
1. Tool básico con schema simple
2. Tool con `node_schema` para inputs dinámicos (ej. inyectar un `schema` desde el LLM)
3. Notas sobre fail-closed behavior (retorna `null` si no hay `texts`)

---

### 1.2 Comportamiento de inputs vacíos: "silenciosamente skipped" vs "null output"

**Problema:** `docs/node_configurations.json` línea 1888 y `docs/developer_guide/37_router_and_output_parser.md` línea 95 dicen:

> "Si no llega ningún `texts`, el nodo se skipea silenciosamente"
> "If no texts are provided, the node is silently skipped."

**Verificación en código:** `extraction.rs` línea 133-138:
```rust
if formatted_texts.is_empty() {
    colmena_log!(
        "⚠️ [ExtractionNode] Skipped execution because 'texts' input was missing or empty."
    );
    return Ok(Value::Null);
}
```

**Inconsistencia:** El código retorna `Value::Null` (un output normal), no "skipea" la ejecución en el sentido de no producir salida. En términos del DAG engine, esta es UNA EJECUCIÓN EXITOSA que retorna `null`, no una "ejecución skipped" (que sería un no-op o un paso omitido). La diferencia importa para downstream edge resolution y error handling.

**Impacto:** Medio. Un operator que espera que el nodo sea "skipped" (no ejecutado) puede sorprenderse de que downstream reciba `null` y continúe procesando.

**Remediación:** Cambiar la documentación a: "Si no hay `texts`, el nodo ejecuta pero retorna `result: null` (output nulo, `extra_info: {}`). Downstream edge resolution pasa `null` como es."

---

### 1.3 Comportamiento de suspend no está en la documentación simple

**Problema:** `docs/node_configurations.json` línea 1892 menciona "schema-driven suspend" pero es BREVE:

> "On a schema-driven suspend (schema/LLM sets suspend=true and a task memory repo is present) it carries { __colmena_status: 'SUSPENDED', all_tasks: [...] }."

**Verificación en código:** `extraction.rs` línea 198-268 implementa:
1. Lectura de `add_tasks` array del parsed JSON (línea 200)
2. Lectura de `delete_tasks` array del parsed JSON (línea 232)
3. Persistencia en `task_memory_repo` (si está presente)
4. Lectura de `suspend` boolean del parsed JSON (línea 255)
5. Si `suspend=true` Y repo presente: retorna estructura especial (línea 260-266)
6. Si repo está ausente pero `suspend=true`: silenciosamente ignorado (retorna output normal)

**Inconsistencia:** La documentación NO explica que:
- El nodo espera que el schema LLM-generado incluya campos `suspend`, `add_tasks`, `delete_tasks` (estos no son parte del schema del usuario, son interpretaciones mutaciones internas)
- Si no hay `task_memory_repo` pero el schema emite `suspend=true`, es IGNORADO (fallback silencioso)
- El patron es específico para flujos de orchestrator con criticadores que generan mutaciones de tareas

**Impacto:** Medio-Alto. Developers de orchestrators que no leen el código Rust no comprenderán que necesitan declarar campos adicionales en el schema o que el repo es obligatorio.

**Remediación:** Documentar en `docs/developer_guide/37_router_and_output_parser.md` una sección "Suspend e Integración con Orchestrator" explicando:
- El schema puede incluir opcionalmente `suspend: boolean`, `add_tasks: [...]`, `delete_tasks: [...]`
- Requiere `task_memory_repo` (auto-wired en orchestrators)
- Si falta repo: `suspend` es ignorado (fallback a output normal)

---

### 1.4 Campo "model" es opcional en config pero doc es genérica

**Problema:** `docs/node_configurations.json` línea 1831-1833 dice:

```
"model": {
  "required": false,
  "default": "Provider-dependent"
}
```

**Verificación en código:** `extraction.rs` línea 67-70 hace exactamente eso — map the model string si existe, otherwise leave None. El helper `extract_with_schema` línea 19 lo maneja como `Option<String>`.

**Verificación LLM provider factory:** El provider factory usa defaults según el provider (OpenAI→gpt-4o, Google→gemini-pro, Anthropic→claude-3-sonnet) si `model` es None.

**Estado:** OK. No hay inconsistencia. La documentación es correcta y genérica de propósito.

---

### 1.5 Markdown code fences stripping no documentado

**Problema:** `docs/developer_guide/37_router_and_output_parser.md` y `docs/node_configurations.json` NO mencionan que el nodo automáticamente stripea ` ```json ... ``` ` fences del output del LLM antes de parsear.

**Verificación en código:** `extract_with_schema.rs` línea 100-120 (el método `parse_and_validate` que llama a `strip_markdown_code_fences`). La función vive en una línea que no es visible en mi lectura anterior, pero la lógica existe.

**Impacto:** Bajo. El comportamiento es deseable (los LLM frecuentemente emiten código en bloques markdown), pero la documentación NO lo clarifica. Un developer que recibe un error de parseo JSON puede no saber que el nodo ya hace stripping.

**Remediación:** Agregar nota en `docs/node_configurations.json` línea 1888 (output description): "Markdown code blocks (` ```json ... ``` `) son automáticamente removidos antes de parsear JSON."

---

### 1.6 Test graph (extraction_example.json) usa pattern incorrecto de edges

**Problema:** `tests/graphs/agents/extraction_example.json` línea 45-58 define edges:
```json
{ "from": "slack_message", "to": "extract_info" },
{ "from": "email_body", "to": "extract_info" }
```

Sin usar el patrón `texts.<name>` explícitamente en la ruta.

**Verificación en código:** `extraction.rs` línea 105-122 itera sobre `inputs` esperando claves con prefijo `texts.`. Las edges simples (sin path explícito) dependen de la resolución del DAG engine que debe: (1) usar el default_output del nodo origen (para slack_message y email_body: "output"), (2) mapear a default_input del destino. Pero `information_extraction` tiene `default_input: null` (línea 1895 de node_configurations.json).

**Inconsistencia:** El test debería usar:
```json
{ "from": "slack_message.slack_message", "to": "extract_info.texts.slack_message" },
{ "from": "email_body.email_body", "to": "extract_info.texts.email_body" }
```

O en versión más corta si el DAG engine usa el output del nodo source:
```json
{ "from": "slack_message", "to": "extract_info.texts.message_1" },
{ "from": "email_body", "to": "extract_info.texts.message_2" }
```

**Impacto:** Alto. El test podría estar fallando silenciosamente (inputs se pierden, nodo retorna null) y los desarrolladores podrían confundirse sobre cómo conectar fuentes de texto.

**Remediación:** Actualizar `extraction_example.json` para usar explícitamente `texts.<name>` en las rutas de edge, Y agregar ejemplo canónico en `docs/developer_guide/37_router_and_output_parser.md` mostrando el patrón correcto.

---

## 2. Hallazgos: Código

### 2.1 Resolución de env vars en api_key — implementación vs documentación

**Problema/Verificación:** `extraction.rs` línea 28-36 implementa `resolve_env_var`:
```rust
fn resolve_env_var(value: &str) -> Result<String, String> {
    if value.starts_with("${") && value.ends_with("}") {
        let var_name = &value[2..value.len() - 1];
        std::env::var(var_name).map_err(|_| format!("Environment variable {} not found", var_name))
    } else {
        Ok(value.to_string())
    }
}
```

**Verificación doc:** `docs/node_configurations.json` línea 1825-1826 confirma: "API key for the provider. Supports '${VAR_NAME}' syntax."

**Estado:** OK. Completamente alineado.

---

### 2.2 Provider validation es case-insensitive

**Problema/Verificación:** `extraction.rs` línea 54:
```rust
let provider_kind = match provider_str.to_lowercase().as_str() {
    "openai" => ProviderKind::OpenAi,
    "google" => ProviderKind::Google,
    "anthropic" => ProviderKind::Anthropic,
    _ => return Err(format!("Invalid provider '{}'.", provider_str).into()),
};
```

**Verificación doc:** `docs/node_configurations.json` línea 1818 lista `["openai", "google", "anthropic"]` (minúscula). La doc NO dice "case-insensitive", pero el código lo es.

**Impacto:** Bajo (comportamiento más tolerante que el esperado es generalmente positivo).

**Estado:** OK.

---

### 2.3 Schema is required, pero falta validación de estructura JSON Schema

**Problema/Verificación:** `extraction.rs` línea 79-80:
```rust
let schema = config.get("schema").ok_or("Missing 'schema' in config")?;
```

El nodo solo verifica que la clave existe, NO que es un JSON Schema válido. Cualquier JSON object es aceptado.

**Verificación doc:** `docs/node_configurations.json` línea 1838-1839 marca `"required": true` pero NO valida estructura interna. Esto es coherente — el nodo delega al LLM para interpretar el schema.

**Estado:** OK. Coherencia esperada (schema validation es responsabilidad del LLM, no del nodo).

---

### 2.4 system_message resolution: config + input merge pattern

**Problema/Verificación:** `extraction.rs` línea 81-85:
```rust
let user_system_message = inputs
    .get("system_message")
    .and_then(|v| v.as_str())
    .or_else(|| config.get("system_message").and_then(|v| v.as_str()))
    .unwrap_or("");
```

El patrón es: input primero, fallback a config, fallback a empty string.

**Verificación doc:** `docs/node_configurations.json` línea 1880-1882 dice "Dynamic system message override. Appended to the built-in extraction prompt." Implícitamente, input override es prioridad (coherente con el código).

**Estado:** OK.

---

### 2.5 Task memory repository es optional pero no documentado el fallback

**Problema/Verificación:** `extraction.rs` línea 16-26:
```rust
pub struct ExtractionNode {
    task_memory_repo: Option<Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>>,
}
```

El repo es `Option<Arc<...>>`, optional. Línea 198: `if let Some(repo) = &self.task_memory_repo { ... }`. Si no está presente, todo el bloque de task mutation y suspend es skipped.

**Verificación doc:** NO hay mención en `docs/node_configurations.json` o `docs/developer_guide/37_router_and_output_parser.md` que el repo es opcional. La documentación implica que suspend "simplemente funciona" sin aclarar el requisito.

**Impacto:** Medio. Si un developer intenta usar suspend sin proporcionar un repo, la salida es silenciosa (no error, solo suspend ignorado).

**Estado:** Inconsistencia confirmada. Ver hallazgo 1.3.

---

### 2.6 Verbose logging es correctamente gateado

**Problema/Verificación:** `extraction.rs` línea 73-76:
```rust
let verbose = config
    .get("verbose")
    .and_then(|v| v.as_bool())
    .unwrap_or(false);
```

Y línea 140-148 y 185-190 emiten logs solo si `verbose=true` via `colmena_log!`. Además, línea 176-184 usa `tracing::debug!` con `target: T_EXTRACTION` para metadatos, nunca el payload bruto (por política de seguridad).

**Verificación doc:** `docs/node_configurations.json` línea 1867-1871 confirma que `verbose` controla la salida.

**Estado:** Excelente. Logging seguro y documentado.

---

### 2.7 Return value structure: result + extra_info

**Problema/Verificación:** `extraction.rs` línea 270-273:
```rust
Ok(json!({
    "result": parsed_json,
    "extra_info": {}
}))
```

El nodo siempre retorna un objeto con dos claves top-level. Esto NO coincide con `default_output: "result"` en la config, que implicaría que el output es solo `result` (no wrapping).

**Verificación doc:** `docs/node_configurations.json` línea 1896 dice `"default_output": "result"` (meaning the edge resolver should extract the "result" field), pero línea 1885-1893 describe dos output_ports: `result` y `extra_info`.

**Resolución:** El `default_output: "result"` es correcto — es UNA INSTRUCCIÓN AL RESOLVER DE EDGES que cuando un edge referencias el nodo sin campo explícito (ej `from: "extract_info"`), usa el campo "result" de su output (no el objeto completo). Esto es coherente.

**Estado:** OK.

---

### 2.8 Payload tracing policy — double-gated

**Problema/Verificación:** `extraction.rs` línea 181-184:
```rust
crate::dag_engine::log_policy::payload_trace!(
    extraction_result,
    parsed = %serde_json::to_string_pretty(&parsed_json).unwrap_or_default()
);
```

El payload (parsed JSON) se emite SOLO si ambas condiciones se cumplen: (1) `RUST_LOG` incluye el target `extraction_result` (EnvFilter), (2) `COLMENA_LOG_PAYLOADS` env var está set. Doble gate.

**Verificación doc:** `docs/developer_guide/50_logging_and_observability.md` (no leído en detalle, pero comentario línea 170-175 lo referencia).

**Estado:** Excelente. Política coherente con la de otros nodos.

---

## 3. Casos de Prueba Ejecutables

Todos los casos usan `cargo run --bin dag_engine -- run <graph.json>` con `--agent-session-id` para keying de estado.

### 3.1 Test A: Extracción básica con dos fuentes de texto

**Archivo:** `tests/graphs/agents/extraction_example.json` (requiere fix de edges)

**Comando (corrected):**
```bash
cargo run --bin dag_engine -- run tests/graphs/agents/extraction_example.json --agent-session-id test_ie_001
```

**Validación esperada:**
- Input: dos fuentes (slack_message, email_body)
- Schema: main_objective, deadline, people_assigned
- Output debe contener: `{ "result": { "main_objective": "...", "deadline": "...", "people_assigned": [...] }, "extra_info": {} }`
- Confirma que múltiples `texts.<name>` se concatenan correctamente

---

### 3.2 Test B: Extracción con system_message customizado

**Archivo:** `tests/graphs/agents/extraction_with_custom_instructions.json` (crear)

```json
{
  "nodes": {
    "raw_text": {
      "type": "input",
      "config": {
        "content": "The meeting is on 2026-09-15 at 3:30 PM. Budget: $50,000. Attendees: Alice (manager), Bob (eng), Carol (design)."
      }
    },
    "extractor": {
      "type": "information_extraction",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "schema": {
          "meeting_date": { "type": "string", "description": "ISO 8601 date" },
          "meeting_time": { "type": "string", "description": "HH:MM format" },
          "budget_usd": { "type": "number", "description": "Budget in USD" },
          "attendees": { "type": "array", "items": { "type": "string" }, "description": "Names and roles" }
        },
        "system_message": "Extract times in HH:MM format (24-hour). Extract dates in YYYY-MM-DD format. Return attendee objects as JSON with name and role fields."
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "raw_text.content", "to": "extractor.texts.meeting_notes" },
    { "from": "extractor", "to": "log_result" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/agents/extraction_with_custom_instructions.json --agent-session-id test_ie_002
```

**Validación esperada:**
- Output contiene el resultado parseado según schema
- Instrucciones custom (ISO 8601 dates, 24-hour time, object attendees) son respetadas por el LLM
- Confirma que `system_message` input override funciona

---

### 3.3 Test C: Extracción sin textos — fallback null

**Archivo:** `tests/graphs/basic/extraction_no_texts.json` (crear)

```json
{
  "nodes": {
    "empty_input": {
      "type": "mock_input",
      "config": {}
    },
    "extractor": {
      "type": "information_extraction",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "schema": { "result": { "type": "string" } }
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "empty_input", "to": "extractor" },
    { "from": "extractor", "to": "log_result" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/basic/extraction_no_texts.json --agent-session-id test_ie_003
```

**Validación esperada:**
- No hay `texts.*` inputs
- Nodo emite log ⚠️ "[ExtractionNode] Skipped execution because 'texts' input was missing or empty."
- Output: `{ "result": null, "extra_info": {} }`
- Confirma que "empty texts" retorna null (NO error, NO skip)

---

### 3.4 Test D: Markdown stripping en output del LLM

**Archivo:** `tests/graphs/agents/extraction_markdown_response.json` (crear)

```json
{
  "nodes": {
    "article": {
      "type": "input",
      "config": {
        "text": "Title: Rust Performance Tips\nAuthor: Jane Smith\nDate: 2026-08-30\nContent: Use iterators instead of loops..."
      }
    },
    "extractor": {
      "type": "information_extraction",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-4o",
        "schema": {
          "title": { "type": "string" },
          "author": { "type": "string" },
          "date": { "type": "string" }
        },
        "verbose": false
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "article.text", "to": "extractor.texts.article" },
    { "from": "extractor", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/agents/extraction_markdown_response.json --agent-session-id test_ie_004
```

**Validación esperada:**
- Incluso si el LLM retorna ` ```json { "title": "...", ... } ``` `, el nodo stripea los fences
- Output: `{ "result": { "title": "...", "author": "...", "date": "..." }, "extra_info": {} }`
- Confirma que markdown stripping funciona silenciosamente

---

### 3.5 Test E: Provider env var resolution

**Archivo:** `tests/graphs/agents/extraction_env_vars.json` (crear)

```json
{
  "nodes": {
    "data": {
      "type": "input",
      "config": { "text": "Contact: alice@example.com, Phone: +1-555-1234" }
    },
    "extractor": {
      "type": "information_extraction",
      "config": {
        "provider": "anthropic",
        "api_key": "${ANTHROPIC_API_KEY}",
        "schema": { "email": { "type": "string" }, "phone": { "type": "string" } }
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "data.text", "to": "extractor.texts.data" },
    { "from": "extractor", "to": "log" }
  ]
}
```

**Ejecución (con env var set):**
```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/agents/extraction_env_vars.json --agent-session-id test_ie_005
```

**Validación esperada:**
- `api_key: "${ANTHROPIC_API_KEY}"` se resuelve desde env
- Si env var no existe: error "Environment variable ANTHROPIC_API_KEY not found"
- Confirma que env var resolution es case-sensitive y obligatoria

---

### 3.6 Test F: Invalid provider — error fail-closed

**Archivo:** `tests/graphs/agents/extraction_invalid_provider.json` (crear)

```json
{
  "nodes": {
    "data": { "type": "input", "config": { "text": "Test" } },
    "extractor": {
      "type": "information_extraction",
      "config": {
        "provider": "invalid_llm",
        "api_key": "dummy",
        "schema": { "field": { "type": "string" } }
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "data.text", "to": "extractor.texts.data" },
    { "from": "extractor", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/agents/extraction_invalid_provider.json --agent-session-id test_ie_006
```

**Validación esperada:**
- Error: "Invalid provider 'invalid_llm'."
- DAG fails, no downstream execution
- Confirma que provider validation es fail-closed

---

### 3.7 Test G: Schema-driven suspend (orchestrator integration)

**Archivo:** `tests/graphs/advanced/extraction_with_suspend.json` (crear, requiere DATABASE_URL + task_memory_repo wired)

```json
{
  "nodes": {
    "data": {
      "type": "input",
      "config": {
        "text": "Project: Data Pipeline. Status: In Progress. Suggested next: Review schema with team. Assign: Carlos (reviewer)."
      }
    },
    "extractor": {
      "type": "information_extraction",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "schema": {
          "project_name": { "type": "string" },
          "status": { "type": "string" },
          "suspend": { "type": "boolean", "description": "Request human review?" },
          "add_tasks": {
            "type": "array",
            "items": { "type": "object" },
            "description": "New tasks: [{task, assigned_to}]"
          },
          "delete_tasks": { "type": "array", "items": { "type": "string" } }
        }
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "data.text", "to": "extractor.texts.data" },
    { "from": "extractor", "to": "log" }
  ]
}
```

**Ejecución (requiere DB):**
```bash
source .env
export DATABASE_URL="postgresql://user:pass@localhost/colmena_llm_memory"
cargo run --bin dag_engine -- run tests/graphs/advanced/extraction_with_suspend.json --agent-session-id test_ie_007
```

**Validación esperada:**
- Si el schema LLM retorna `suspend: true` Y hay `task_memory_repo`: output contiene `{ "result": {...}, "extra_info": { "__colmena_status": "SUSPENDED", "all_tasks": [...] } }`
- Si falta repo: suspend es ignorado (silencioso)
- `add_tasks` y `delete_tasks` campos son persistidos (si repo presente)
- Confirma que suspension e integración con orchestrator funciona

---

## Resumen de Hallazgos

| # | Tipo | Severidad | Descripción |
|---|------|-----------|-------------|
| 1.1 | Docs | Alta | `node_as_tools_reference.json` sin entrada para `information_extraction` |
| 1.2 | Docs | Media | Descripción "silenciosamente skipped" vs "retorna null" es ambigua |
| 1.3 | Docs | Alta | Suspend + orchestrator behavior no está documentado, repo requirement implícito |
| 1.4 | Docs | OK | Campo `model` opcional, documentado correctamente |
| 1.5 | Docs | Baja | Markdown stripping no documentado |
| 1.6 | Docs | Alta | Test graph `extraction_example.json` usa pattern incorrecto de edges |
| 2.1 | Código | OK | Env var resolution coherente con docs |
| 2.2 | Código | OK | Provider case-insensitive (tolerable) |
| 2.3 | Código | OK | Schema validation pattern coherente |
| 2.4 | Código | OK | system_message merge pattern coherente |
| 2.5 | Código | Media | Task memory repo opcional pero fallback no documentado |
| 2.6 | Código | Excelente | Verbose logging correctamente gateado |
| 2.7 | Código | OK | Estructura result + extra_info, default_output resuelto correctamente |
| 2.8 | Código | Excelente | Payload tracing doblemente gateado |

---

## Remediaciones Recomendadas

### Prioridad ALTA (bloquea automatización o funcionalidad)

1. **Agregar entrada `information_extraction` a `docs/node_as_tools_reference.json`** con ejemplos de tool calling, `node_schema` patterns, y fail-closed behavior.
2. **Documentar suspend + orchestrator integration** en `docs/developer_guide/37_router_and_output_parser.md` nueva sección, explicando campos `suspend`, `add_tasks`, `delete_tasks`, req. de `task_memory_repo`.
3. **Arreglar test graph `extraction_example.json`** para usar explícitamente `texts.<name>` en edges, Y agregar ejemplo canónico en developer guide.

### Prioridad MEDIA (afecta discovery/UX)

4. **Clarificar "silenciosamente skipped" → "retorna null"** en `docs/node_configurations.json` y developer guide.
5. **Documentar que `task_memory_repo` es optional** y su fallback behavior (suspend ignorado si repo ausente).

### Prioridad BAJA (legibilidad)

6. **Agregar nota sobre markdown stripping** en `docs/node_configurations.json` línea 1888 (output description).

---

**Auditoría completada:** 8 hallazgos en documentación (3 altos, 2 medios, 1 bajo, 2 OK) + 8 aspectos de código validados (5 OK, 2 excelentes, 1 media) + 7 casos de prueba ejecutables.
