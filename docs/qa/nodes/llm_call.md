# QA — Nodo `llm_call`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (líneas 391-6599)

Fuentes de doc revisadas:
- `docs/node_configurations.json` (líneas 1454-1806, entrada `llm_call`)
- `docs/node_as_tools_reference.json`
- `docs/agent_context/node_ports_reference.md`
- `docs/developer_guide/14_llm_deep_dive.md`
- `docs/developer_guide/09_tool_calling.md`

## 1) Config documentada NO soportada por el código

**Sin discrepancias detectadas.** Todos los campos descritos en `docs/node_configurations.json` para la entrada `llm_call` (líneas 1459–1769) son soportados por el código en llm.rs. El precedence de inputs > config se respeta uniformemente. Validaciones de tipos y defaults coinciden.

Ejemplos verificados:
- `provider` (requerido, valores "openai"|"google"|"anthropic"|"mock") — validado línea 1244-1256 en llm.rs
- `max_iterations` (default 3, min 1) — usado línea 1489-1494 en llm.rs
- `stream` (boolean, default false) — accedido línea 438 en llm.rs
- `attachments_enabled` (boolean, default true) — validado línea 2098 en llm.rs
- `crdt_documents` (objeto con campos typed) — procesado línea ~2184 en llm.rs

## 2) Código NO documentado

### 2.1 Campo `thinking_budget` (sin entrada en node_configurations.json)

**Qué documenta:** `docs/node_configurations.json` NO incluye `thinking_budget` como campo de configuración para llm_call.

**Qué hace el código:** `llm.rs:1476-1481` — El código acepta `thinking_budget` (como u64 desde inputs o config) y lo pasa a `llm_config.with_thinking_budget(thinking_budget as u32)`. Es un campo válido que controla el budget de "thinking tokens" en proveedores que lo soportan (Anthropic).

**Impacto para QA:** Operadores pueden pasar `thinking_budget` en grafos JSON sin error, pero la documentación oficial omite este campo. Causa brecha entre qué es legal en grafos vs. qué documenta la API.

**Hallazgo:** Campo funcional no documentado. Se menciona de pasada en `docs/developer_guide/14_llm_deep_dive.md:109` ("thinking_budget se mapea a...") pero falta entrada formal en `node_configurations.json::llm_call::config_fields`.

---

### 2.2 Campo `documents` (sin entrada en node_configurations.json)

**Qué documenta:** `docs/node_configurations.json` NO incluye `documents`.

**Qué hace el código:** `llm.rs:2146-2160` — El código acepta `documents` (objeto de configuración desde inputs o config) y lo pasa a `DocumentRuntime::from_config(&doc_cfg)`. Si tiene éxito, monta herramientas sintéticas para lectura/escritura de documentos.

**Impacto para QA:** Campo funcional no expuesto en la documentación oficial. Los operadores que intenten usar `documents` en la config de llm_call deben conocer la estructura exacta del config object (que no está documentada).

**Hallazgo:** Campo funcional no documentado. Correlato: `crdt_documents` SÍ está documentado (línea 1718-1769 en node_configurations.json), pero `documents` (su antecesor o variante) no lo está.

---

### 2.3 Campo `skills_path` (sin entrada en node_configurations.json)

**Qué documenta:** `docs/node_configurations.json` documenta `skills` (objeto con sub-campos `builtin` y `paths`), pero NO menciona un campo `skills_path` a nivel de config.

**Qué hace el código:** `llm.rs:721-762` — El código accede a:
- `config.get("skills_path")` (línea 721) — valor string de ruta única
- Luego durante construcción de skill repository también accede a `config.get("skills_path")` (línea 762)

**Impacto para QA:** Campo `skills_path` es una forma antigua o alternativa de declarar rutas de skills. La forma moderna y documentada es `skills.paths` (array). Operadores usando `skills_path` pueden ver comportamiento inesperado o incompatibilidad.

**Hallazgo:** Campo legado no documentado. Soportado en código pero marcado como no-canónico (la forma oficial es `skills.paths`).

---

### 2.4 Output field `tools_discovered` en extra_info (parcialmente documentado)

**Qué documenta:** `docs/node_configurations.json:1797-1800` describe `extra_info` como `{ "usage": ..., "tool_calls": ... }` pero no menciona `tools_discovered`.

**Qué hace el código:** `llm.rs:3757-3763` — Cuando `lazy_tool_loading` es true, un array de nombres de tools descubiertos (en orden) se añade a `extra_info["tools_discovered"]`.

**Impacto para QA:** Frontend/sistemas que lean `extra_info` de llm_call pueden no esperar el campo `tools_discovered`, causando surpresas o parsing errors.

**Hallazgo:** Output field condicional no documentado.

---

### 2.5 Output field `skills_used` en extra_info (no documentado)

**Qué documenta:** `docs/node_configurations.json:1797-1800` no menciona `skills_used`.

**Qué hace el código:** `llm.rs:3714-3755` — Cuando skills se cargan via skill repository, un resumen agregado de skills (nombre, source, referencias, load_count) se añade a `extra_info["skills_used"]`.

**Impacto para QA:** Consumidores de `extra_info` pueden no esperar este campo.

**Hallazgo:** Output field condicional no documentado.

---

### 2.6 Validación fail-closed: missing provider/api_key SIN mensaje descriptivo documentado

**Qué documenta:** `docs/node_configurations.json:1459-1473` describe que `provider` y `api_key` son requeridos, pero NO documenta el mensaje de error exacto que el operador verá si faltan.

**Qué hace el código:** `llm.rs:1241-1242, 1262-1263` — Retorna `Err("Missing 'provider' in inputs or config")` y `Err("Missing 'api_key' in inputs or config")` respectivamente.

**Impacto para QA:** Usuarios reciben errores genéricos sin guía sobre dónde configurar estos campos. Los mensajes de error NO mencionan que pueden venir via inputs O config, lo cual podría confundir a operadores que esperaban solo una fuente.

**Hallazgo:** Mensajes de error fail-closed no documentados.

---

### 2.7 Comportamiento de precedencia: resumption via `__colmena_resume_answer`

**Qué documenta:** `docs/node_configurations.json` no menciona `__colmena_resume_answer` como un campo especial que el engine inyecta en inputs para reanudar después de una suspend.

**Qué hace el código:** `llm.rs:1279-1282` — El código detecta `__colmena_resume_answer` en inputs y activa el camino de resume. Esto es un campo inyectado internamente, no configurable por usuarios.

**Impacto para QA:** Documentación incompletamente describe cómo funciona el flujo de resume (suspend/resume) a nivel de configuración de llm_call.

**Hallazgo:** Campo inyectado no mencionado en documentación de config (aunque SÍ está bien documentado en developer_guide/19_nested_agents_and_subgraphs.md).

---

### 2.8 Precedencia no explícita: prompt vs task fallback

**Qué documenta:** `docs/node_configurations.json:1482-1494` describe que `prompt` es el campo principal y `task` es fallback, pero el CÓDIGO implementa una cadena diferente.

**Qué hace el código:** `llm.rs:425-427` en `resolve_prompt_or_task()` muestra la cadena exacta:
```rust
inputs.get("prompt").filter(|v| is_present(v))
  .or_else(|| config.get("prompt").filter(|v| is_present(v)))
  .or_else(|| inputs.get("task"))
  .or_else(|| config.get("task"))
```

La función `is_present()` descarta valores `null` y vacíos de prompt, permitiendo que task tome su lugar incluso cuando prompt está explícitamente seteado a `null` desde inputs.

**Impacto para QA:** La documentación dice "falls back to 'task' if prompt is not found" pero NO explica que `prompt: null` en inputs fuerza la caída a `task`. Esto puede sorprender a operadores que pasan `prompt: null` esperando saltar la ejecución.

**Hallazgo:** Precedencia de inputs no completamente explicitada en la documentación.

---

## 3) Plan de pruebas QA

### Caso T1: Happy path básico — prompt simple, default provider

**Objetivo:** Verificar que una llamada mínima (provider, api_key, prompt) retorna un resultado válido.

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "id": "trigger",
      "type": "trigger_webhook",
      "config": { "test_payload": { "prompt": "¿Cuál es la capital de Francia?" } }
    },
    {
      "id": "llm",
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}"
      }
    },
    {
      "id": "output",
      "type": "output"
    }
  ],
  "edges": [
    { "from": "trigger.output.prompt", "to": "llm.prompt" },
    { "from": "llm.result", "to": "output.input" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run test_t1.json \
  --agent-session-id test_t1_001
```

**Resultado esperado:**
- `output.result` es un string no vacío (respuesta del modelo)
- `output.extra_info.usage.prompt_tokens > 0`
- `output.extra_info.usage.completion_tokens > 0`
- `output.extra_info.tool_calls` es array vacío (sin tools)

**Cómo verificar pass/fail:**
- No hay error en stderr
- `result` NO es null ni cadena vacía
- `extra_info.usage` tiene números positivos

---

### Caso T2: Config con tools enabled — LLM llama a herramientas

**Objetivo:** Verificar que enabled_tools permite al LLM elegir y ejecutar tools.

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "id": "trigger",
      "type": "trigger_webhook",
      "config": { "test_payload": { "prompt": "¿Qué hora es ahora en hora UTC?" } }
    },
    {
      "id": "llm",
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "enabled_tools": ["current_time"]
      }
    },
    {
      "id": "output",
      "type": "output"
    }
  ],
  "edges": [
    { "from": "trigger.output.prompt", "to": "llm.prompt" },
    { "from": "llm.result", "to": "output.input" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run test_t2.json \
  --agent-session-id test_t2_001
```

**Resultado esperado:**
- El nodo llm retorna un resultado
- `extra_info.tool_calls` es un array con al menos una llamada (el modelo optó por usar `current_time`)
- La respuesta final contiene información de fecha/hora

**Cómo verificar pass/fail:**
- No hay error
- `extra_info.tool_calls` no es array vacío
- El contenido de la respuesta es coherente con la query de hora

---

### Caso T3: Memory y conversation state — multi-turn con session_id

**Objetivo:** Verificar que conversation memory persiste entre runs con el mismo agent_session_id.

**Grafo JSON (mismo para ambos runs):**
```json
{
  "nodes": [
    {
      "id": "trigger",
      "type": "trigger_webhook",
      "config": { "test_payload": { "prompt": "Mi nombre es Alice" } }
    },
    {
      "id": "llm",
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "connection_url": "sqlite:///tmp/test_t3_memory.db"
      }
    },
    {
      "id": "output",
      "type": "output"
    }
  ],
  "edges": [
    { "from": "trigger.output.prompt", "to": "llm.prompt" },
    { "from": "llm.result", "to": "output.input" }
  ]
}
```

**Ejecución — Turn 1:**
```bash
cargo run --bin dag_engine -- run test_t3.json \
  --agent-session-id test_t3_memory
```

**Ejecución — Turn 2:**
```bash
cargo run --bin dag_engine -- run test_t3.json \
  --agent-session-id test_t3_memory \
  --answer $'Q[unknown]: ...\nA[unknown]: ¿Cuál es mi nombre?'
```

(Nota: Turn 2 requeriría un suspend en el grafo; este caso es simplificado para verificar que memory se carga.)

**Resultado esperado — Turn 1:**
- Modelo recibe el mensaje inicial
- Historia se persiste en la DB

**Resultado esperado — Turn 2:**
- Modelo recarga la historia anterior
- El modelo "recuerda" que el usuario se llama Alice (puede verificarse en el contenido de la respuesta)

**Cómo verificar pass/fail:**
- DB file existe después de Turn 1 (`/tmp/test_t3_memory.db`)
- Turn 2 conecta y lee la DB sin error
- Contenido de respuesta en Turn 2 es coherente con el contexto de Turn 1

---

### Caso T4: Streaming — stream flag activo

**Objetivo:** Verificar que streaming emite tokens via observer en modo real-time.

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "id": "trigger",
      "type": "trigger_webhook",
      "config": { "test_payload": { "prompt": "Escribe una lista de 5 frutas" } }
    },
    {
      "id": "llm",
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "stream": true
      }
    },
    {
      "id": "output",
      "type": "output"
    }
  ],
  "edges": [
    { "from": "trigger.output.prompt", "to": "llm.prompt" },
    { "from": "llm.result", "to": "output.input" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run test_t4.json \
  --agent-session-id test_t4_stream 2>&1 | tee /tmp/test_t4_stream.sse
```

**Resultado esperado:**
- SSE output contiene múltiples eventos `token-delta` (uno por token)
- Total de `completion_tokens` en `LlmUsage` event coincide con cantidad de tokens emitidos
- `result` final en output es string completo (no fragmentado)

**Cómo verificar pass/fail:**
- Grep `/tmp/test_t4_stream.sse` para contar eventos `"type": "token-delta"` — debe haber >1
- El resultado final en output node es coherente
- No hay error en stderr

---

### Caso T5: Multimodal — files array con imagen

**Objetivo:** Verificar que el nodo acepta y procesa archivos (imagen).

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "id": "trigger",
      "type": "trigger_webhook",
      "config": {
        "test_payload": {
          "prompt": "Describe this image",
          "files": [
            {
              "mime_type": "image/png",
              "path": "tests/graphs/media/test_image.jpg"
            }
          ]
        }
      }
    },
    {
      "id": "llm",
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}"
      }
    },
    {
      "id": "output",
      "type": "output"
    }
  ],
  "edges": [
    { "from": "trigger.output.prompt", "to": "llm.prompt" },
    { "from": "trigger.output.files", "to": "llm.files" },
    { "from": "llm.result", "to": "output.input" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run test_t5.json \
  --agent-session-id test_t5_multimodal
```

**Resultado esperado:**
- No hay error sobre archivo no encontrado
- `result` contiene una descripción de la imagen (no genérica, específica al contenido)
- `extra_info.usage` muestra tokens gastos (multimodal input es más costoso)

**Cómo verificar pass/fail:**
- No stderr
- `result` NO es "I can't see the image" o "Invalid file"
- `usage.prompt_tokens > 100` (imagen = muchos tokens)

---

### Caso T6: Defaults — mínimos campos, usando defaults

**Objetivo:** Verificar que los defaults documentados se aplican correctamente.

**Grafo JSON (provider y api_key requeridos, pero otros campos son defaults):**
```json
{
  "nodes": [
    {
      "id": "trigger",
      "type": "trigger_webhook",
      "config": { "test_payload": { "prompt": "Hola" } }
    },
    {
      "id": "llm",
      "type": "llm_call",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}"
      }
    },
    {
      "id": "output",
      "type": "output"
    }
  ],
  "edges": [
    { "from": "trigger.output.prompt", "to": "llm.prompt" },
    { "from": "llm.result", "to": "output.input" }
  ]
}
```

**Verificaciones de defaults aplicados:**
- `model` no está en config — debe usarse el default por provider (google → gemini-2.5-flash según los defaults)
- `stream` no está en config — default `false`, no hay eventos token-delta
- `enabled_tools` no está en config — default none, LLM no tiene tools
- `verbose` no está en config — default `false`, sin output de debug
- `temperature` no está en config — LLM usa su default interno
- `system_message` no está en config — usará `LLM_DEFAULT_SYSTEM` (línea 40 en llm.rs)

**Ejecución:**
```bash
cargo run --bin dag_engine -- run test_t6.json \
  --agent-session-id test_t6_defaults 2>&1 | head -20
```

**Resultado esperado:**
- No hay mensajes VERBOSE en stdout (porque verbose=false por default)
- `result` es válido
- No hay eventos token-delta (porque stream=false por default)
- El prompt lleva el sistema message default (verificable solo via logs internos)

**Cómo verificar pass/fail:**
- No stdout con "VERBOSE"
- Resultado es válido y completo
- No "token-delta" events

---

### Caso T7: Error fail-closed — falta provider

**Objetivo:** Verificar mensaje de error descriptivo cuando falta configuración requerida.

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "id": "trigger",
      "type": "trigger_webhook",
      "config": { "test_payload": { "prompt": "Hola" } }
    },
    {
      "id": "llm",
      "type": "llm_call",
      "config": {
        "api_key": "${GEMINI_API_KEY}"
      }
    },
    {
      "id": "output",
      "type": "output"
    }
  ],
  "edges": [
    { "from": "trigger.output.prompt", "to": "llm.prompt" },
    { "from": "llm.result", "to": "output.input" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run test_t7_error.json \
  --agent-session-id test_t7_error 2>&1
```

**Resultado esperado — falla con error:**
- stderr contiene: "Missing 'provider' in inputs or config"
- DAG execution stops, output node is not reached
- No partial result is returned

**Cómo verificar pass/fail:**
- Exit code is non-zero
- Error message is in stderr (not swallowed)
- Message is actionable (tells user where to set provider)

---

### Caso T8: thinking_budget field (Anthropic only)

**Objetivo:** Verificar que thinking_budget es aceptado y pasa al provider sin error.

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "id": "trigger",
      "type": "trigger_webhook",
      "config": { "test_payload": { "prompt": "Resuelve: 2+2=?" } }
    },
    {
      "id": "llm",
      "type": "llm_call",
      "config": {
        "provider": "anthropic",
        "model": "claude-opus-4-1-20250805",
        "api_key": "${ANTHROPIC_API_KEY}",
        "thinking_budget": 5000
      }
    },
    {
      "id": "output",
      "type": "output"
    }
  ],
  "edges": [
    { "from": "trigger.output.prompt", "to": "llm.prompt" },
    { "from": "llm.result", "to": "output.input" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run test_t8_thinking.json \
  --agent-session-id test_t8_thinking
```

**Resultado esperado:**
- No hay error sobre "unknown field thinking_budget"
- `extra_info.usage.thinking_tokens > 0` (verificable solo si el modelo retorna thinking tokens)
- `result` es válido

**Cómo verificar pass/fail:**
- No error "unrecognized field"
- `result` es coherente con el prompt
- Si el modelo soporta thinking tokens, `usage` lo registra

---

### Caso T9: Conditional execution — prompt: null skip

**Objetivo:** Verificar que cuando prompt resuelve a null/vacío, el nodo se salta (retorna null).

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "id": "input",
      "type": "input",
      "config": { "data": {} }
    },
    {
      "id": "llm",
      "type": "llm_call",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "prompt": ""
      }
    },
    {
      "id": "output",
      "type": "output"
    }
  ],
  "edges": [
    { "from": "llm.result", "to": "output.input" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run test_t9_skip.json \
  --agent-session-id test_t9_skip 2>&1 | grep -E "result|Skipped"
```

**Resultado esperado:**
- `llm.result` es `null` (no se ejecuta LLM)
- Stderr contiene: "⚠️ [LlmNode] Skipped (prompt resolved to empty)"
- `output.result` es `null` (propagates the skip)
- No error, execution succeeds with null result

**Cómo verificar pass/fail:**
- `result` es null (not error, not empty string)
- Log message says "Skipped"
- No LLM call was made (verificable by absence of token usage)

---

### Caso T10: lazy_tool_loading — describe_tool dispatch

**Objetivo:** Verificar que lazy loading expone herramientas en un describe_tool y luego son reveladas on-demand.

**Grafo JSON:**
```json
{
  "nodes": [
    {
      "id": "trigger",
      "type": "trigger_webhook",
      "config": { "test_payload": { "prompt": "¿Qué hora es?" } }
    },
    {
      "id": "llm",
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "enabled_tools": ["current_time"],
        "lazy_tool_loading": true
      }
    },
    {
      "id": "output",
      "type": "output"
    }
  ],
  "edges": [
    { "from": "trigger.output.prompt", "to": "llm.prompt" },
    { "from": "llm.result", "to": "output.input" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run test_t10_lazy.json \
  --agent-session-id test_t10_lazy 2>&1 | tee /tmp/test_t10_lazy.sse
```

**Resultado esperado:**
- `extra_info.tools_discovered` es array con al menos ["current_time"]
- SSE contains "tool-described" events (tools are revealed on-demand, not upfront)
- LLM still successfully calls the tool

**Cómo verificar pass/fail:**
- `extra_info.tools_discovered` is non-empty
- Grep SSE for `"type": "tool-described"` — must have events
- No error about unknown tools

---

## Resumen de auditoría

**Campos hallados en código pero NO documentados:**
1. `thinking_budget` — usado para control de thinking tokens (Anthropic)
2. `documents` — campo para DocumentRuntime integration
3. `skills_path` — forma legada de declarar skills (vs. `skills.paths`)

**Campos de output condicional NO documentados:**
1. `extra_info.tools_discovered` — solo cuando lazy_tool_loading=true
2. `extra_info.skills_used` — solo cuando skills se cargan

**Comportamientos no explícitamente documentados:**
1. Precedencia de prompt vs. task con `is_present()` filter
2. Inyección de `__colmena_resume_answer` para resume flow
3. Mensajes de error específicos para campos requeridos

**Hallazgos de riesgo QA bajo:** Las discrepancias encontradas son principalmente campos funcionales bien soportados por el código pero documentados de manera incompleta. No hay desviaciones que causen errores de runtime o comportamiento inesperado a nivel de user-facing output.
