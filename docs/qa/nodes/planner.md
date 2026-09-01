# QA — Nodo `planner`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/planner.rs`

Fuentes de doc revisadas:
- `docs/node_configurations.json` (sección `"planner"`)
- `docs/agent_context/node_ports_reference.md`
- `docs/developer_guide/20_orchestrator_architecture.md`
- `src/libs/colmena/text/prompts/planner_system.md`

---

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas. El código implementa todos los campos y comportamientos descritos en la documentación.

---

## 2) Código NO documentado

### Hallazgo 2.1: Campo `thinking_budget` ausente en `node_configurations.json`

**Qué dice la doc:** La sección de configuración en `docs/node_configurations.json` NO lista un campo `thinking_budget`.

**Qué hace el código:** Línea 307-309 en `planner.rs`:
```rust
if let Some(budget) = config.get("thinking_budget").and_then(|v| v.as_u64()) {
    llm_config = llm_config.with_thinking_budget(budget as u32);
}
```

El nodo acepta un campo opcional `thinking_budget` (tipo integer/u64) y lo pasa al LLM mediante `llm_config.with_thinking_budget()`. Este campo es útil para proveedores que soportan reasoning tokens (ej. Claude o Gemini con modo thinking).

**Impacto para QA:** Los operadores pueden usar `"thinking_budget": 10000` en la config de un planner para habilitar pensamiento extendido, pero NO sabrían de esta opción leyendo la doc oficial.

---

### Hallazgo 2.2: Campo `streaming` ausente en `node_configurations.json`

**Qué dice la doc:** No hay mención de un campo `streaming` en la config del planner.

**Qué hace el código:** Líneas 344-376 en `planner.rs`:
```rust
let streaming = config
    .get("streaming")
    .and_then(|v| v.as_bool())
    .unwrap_or(false);
// ... se construye un on_token callback que emite eventos SSE
// LlmStreamPart::Content(token) -> NodeEvent::LlmToken { token }
// LlmStreamPart::Usage(usage) -> NodeEvent::LlmUsage { ... }
// etc.
```

Cuando `"streaming": true`, el nodo emite eventos `LlmToken`, `LlmUsage`, `LlmMessageStart`, `LlmMessageFinish` en tiempo real durante la generación. Default es `false`.

**Impacto para QA:** Sin documentación, operadores no saben que pueden habilitar streaming para ver tokens en vivo en grafos que ejecutan un planner.

---

### Hallazgo 2.3: Comportamiento de skip-if-plan-exists NO documentado completamente

**Qué dice la doc:** `docs/node_configurations.json` menciona "If tasks already exist in the database for the current session (Turn 2+), the node is silently skipped to avoid redundant LLM calls." (en el campo `description`).

**Qué hace el código:** Líneas 86-102 en `planner.rs`:
```rust
let session_id = state
    .get("session_id")
    .and_then(|v| v.as_str())
    .unwrap_or("")
    .to_string();
if !session_id.is_empty() {
    if let Some(repo) = &self.task_memory_repo {
        let existing = repo.get_tasks_for_run(&session_id).await?;
        if !existing.is_empty() {
            colmena_log!("⏭️  [PlannerNode] Plan already exists in DB ({} tasks) — skipping LLM call.");
            return Ok(Value::Null);  // <-- devuelve NULL, no un plan vacío
        }
    }
}
```

El nodo devuelve `Value::Null` cuando salta, NO un plan estructurado. Esto podría sorprender a downstream si esperan un `{ result: { items: [...] } }`.

**Impacto para QA:** Es necesario probar que downstream maneja correctamente un output nulo cuando el planner salta.

---

### Hallazgo 2.4: Resolución de env vars en `api_key` y en `agents` object `name` field

**Qué dice la doc:** En `node_configurations.json` se menciona que `api_key` soporta `${VAR_NAME}`. No se menciona env var resolution en otros campos.

**Qué hace el código:** Línea 66-74 en `planner.rs`:
```rust
fn resolve_env_var(value: &str) -> Result<String, String> {
    if value.starts_with("${") && value.ends_with('}') {
        let var_name = &value[2..value.len() - 1];
        std::env::var(var_name)
            .map_err(|_| format!("Environment variable '{}' not found", var_name))
    } else {
        Ok(value.to_string())
    }
}
```

El método `resolve_env_var` se invoca SOLO en `api_key` (línea 121). **No se aplica a `agents` names, descriptions, o `system_message`.** Esto está correctamente documentado.

**Impacto para QA:** Confirmado que env vars se resuelven SOLO en api_key; no es un hallazgo, pero requiere prueba.

---

### Hallazgo 2.5: Schema dinámico con enum `assigned_to` cuando hay agents

**Qué dice la doc:** En `node_configurations.json` se menciona que el schema es "built-in" y "fixed" (implícitamente). No se documenta que el schema se altera dinámicamente según `agents`.

**Qué hace el código:** Líneas 200-238 en `planner.rs`:
```rust
let schema = if !agents.is_empty() {
    let agent_values: Vec<Value> = agents
        .iter()
        .map(|(name, _)| Value::String(name.clone()))
        .collect();
    json!({
        // ... schema con "enum": agent_values
        "assigned_to": {
            "enum": agent_values,
            "description": "The agent node ID responsible for this task."
        }
    })
} else {
    default_planner_schema()
};
```

Cuando `agents` array está vacío, usa `default_planner_schema()` (líneas 18-48), que **no tiene enum** en `assigned_to`. Cuando hay agents, el schema es **dinámico** con un enum constrained a los names reales.

**Impacto para QA:** El comportamiento del schema cambia radicalmente según si hay agents o no; esto requiere pruebas en ambas rutas.

---

### Hallazgo 2.6: Output null vs. normal output discrepancy

**Qué dice la doc:** `docs/agent_context/node_ports_reference.md` dice: "Payload: `result` = `{ items: [{task, assigned_to, completed, phase, parallel}] }`; sibling top-level `extra_info.raw_response` (raw LLM text). Suspend branch instead emits top-level `__colmena_status: "SUSPENDED"` + `result: { questions: [...] }`. Not wrapped in `{ output: ... }`."

**Qué hace el código:** Hay 4 rutas de salida:
1. Línea 99: `return Ok(Value::Null)` cuando salta por plan existente → **OUTPUT NULO, NO DOCUMENTADO**
2. Línea 285: `return Ok(Value::Null)` cuando no hay inputs → **OUTPUT NULO, NO DOCUMENTADO**
3. Líneas 449-457 (suspend branch): `Ok(json!({ "__colmena_status": "SUSPENDED", "result": { "questions": questions }, "extra_info": { "raw_response": raw } }))`
4. Líneas 470-477 (normal/happy path): `Ok(json!({ "result": { "items": items }, "extra_info": { "raw_response": raw } }))`

**Impacto para QA:** El comportamiento nulo (rutas 1 y 2) NO está documentado y puede sorprender a downstream nodes que esperan siempre un `result.items` array.

---

## 3) Plan de pruebas QA

### Caso 3.1: Happy path — Planner con request sencillo y agents constrained

**Objetivo:** Verificar que el planner genera un plan estructurado cuando hay agents definidos y un request claro.

**Grafo mínimo (JSON):**
```json
{
  "nodes": [
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "agents": [
          { "name": "flight_search", "description": "Searches for flights" },
          { "name": "hotel_search", "description": "Searches for hotels" }
        ]
      },
      "inputs": {
        "request": "Plan a trip to Rome for 3 days with budget $3000"
      }
    }
  ],
  "edges": []
}
```

**Ejecución:**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_happy_path.json
```

**Resultado esperado:**
- Output contiene `result.items` (array de tareas)
- Cada tarea es `{ task: string, assigned_to: string, completed: false, phase: integer, parallel: boolean }`
- `assigned_to` values están SOLO dentro del enum: `flight_search` o `hotel_search`
- `extra_info.raw_response` contiene el text raw del LLM
- No hay `__colmena_status: "SUSPENDED"` en top-level

**Cómo se verifica:**
- Parsear JSON output y confirmar estructura
- Grep `"assigned_to"` values contra la lista de agents
- Verificar que no hay `__colmena_status` en top-level

---

### Caso 3.2: Sin agents — Planner asigna tasks libremente

**Objetivo:** Verificar que cuando NO hay agents, el schema se relaja y el planner asigna a nombres arbitrarios.

**Grafo mínimo (JSON):**
```json
{
  "nodes": [
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash"
      },
      "inputs": {
        "request": "Create a marketing campaign. Assign to whoever should do it."
      }
    }
  ],
  "edges": []
}
```

**Ejecución:**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_no_agents.json
```

**Resultado esperado:**
- Output contiene `result.items` (array de tareas)
- `assigned_to` puede ser CUALQUIER string (ej. "content_writer", "designer", "social_media_manager")
- No hay constrains enum en schema enviado al LLM
- `extra_info.raw_response` presente

**Cómo se verifica:**
- Confirmar que tareas están generadas
- Verificar que NO hay restricción enum en `assigned_to`

---

### Caso 3.3: Plan ya existe en DB — Node salta silenciosamente

**Objetivo:** Verificar que cuando el plan ya existe en DB para la sesión, el nodo devuelve NULL sin hacer una llamada LLM.

**Grafo mínimo (JSON):**
```json
{
  "nodes": [
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "agents": [
          { "name": "agent_1", "description": "Test agent" }
        ]
      },
      "inputs": {
        "request": "Some request"
      }
    }
  ],
  "edges": []
}
```

**Ejecución (Turn 1 — plan creado):**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_skip.json --agent-session-id planner_skip_test_001
```

**Ejecución (Turn 2 — plan debe saltar):**
```bash
# Same graph, same agent-session-id
cargo run --bin dag_engine -- run test_planner_skip.json --agent-session-id planner_skip_test_001
```

**Resultado esperado (Turn 1):**
- Output es `{ result: { items: [...] }, extra_info: { ... } }`
- Log contiene "🗂️ [PlannerNode] Planning tasks"

**Resultado esperado (Turn 2):**
- Output es **NULL** (not `{ result: null }`, just `null`)
- Log contiene "⏭️  [PlannerNode] Plan already exists in DB"
- NO hay llamada LLM (se puede verificar con `--agent-session-id` + checking token counts)

**Cómo se verifica:**
- Comparar outputs entre Turn 1 y 2
- Buscar el log pattern `"Plan already exists in DB"`
- Confirmar que output es NULL en Turn 2

---

### Caso 3.4: No hay inputs — Node salta y devuelve NULL

**Objetivo:** Verificar que si no se proporcionan inputs (ni en config ni en inputs), el nodo devuelve NULL.

**Grafo mínimo (JSON):**
```json
{
  "nodes": [
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash"
      },
      "inputs": {}
    }
  ],
  "edges": []
}
```

**Ejecución:**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_no_inputs.json
```

**Resultado esperado:**
- Output es **NULL**
- Log contiene "⚠️ [PlannerNode] Skipped execution because no input text was provided."

**Cómo se verifica:**
- Confirmar que output es NULL
- Grep log para el mensaje de skip

---

### Caso 3.5: Planner solicita clarificación (suspend)

**Objetivo:** Verificar que cuando el planner no tiene suficiente información, devuelve un JSON con `questions` y `__colmena_status: "SUSPENDED"`.

**Grafo mínimo (JSON):**
```json
{
  "nodes": [
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash"
      },
      "inputs": {
        "request": "Plan a trip" 
      }
    }
  ],
  "edges": []
}
```

**Ejecución:**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_suspend.json
```

**Resultado esperado (si modelo decide suspender):**
- Output top-level tiene `__colmena_status: "SUSPENDED"`
- Output top-level tiene `result: { questions: [ { id, question, type, options? }, ... ] }`
- Log contiene "⏸️  [PlannerNode] Planner requested clarification"

**Cómo se verifica:**
- Parsear JSON y confirmar `__colmena_status: "SUSPENDED"`
- Confirmar array `result.questions` no está vacío
- Verificar estructura de cada question object

---

### Caso 3.6: Verbose mode habilita logging de prompt y response

**Objetivo:** Verificar que cuando `verbose: true`, el nodo imprime el system prompt completo, inputs, y raw LLM response.

**Grafo mínimo (JSON):**
```json
{
  "nodes": [
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "verbose": true,
        "agents": [
          { "name": "test_agent", "description": "Test" }
        ]
      },
      "inputs": {
        "request": "Quick test"
      }
    }
  ],
  "edges": []
}
```

**Ejecución:**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_verbose.json 2>&1 | tee /tmp/planner_verbose.log
```

**Resultado esperado:**
- Log contiene "═══════════════════════════════════════"
- Log contiene "🗂️  [PlannerNode] VERBOSE — System Prompt Sent:"
- Log contiene "📥 User Input Texts:"
- Log contiene "🗂️  [PlannerNode] VERBOSE — Raw LLM Response:"
- System prompt incluye agent descriptions

**Cómo se verifica:**
- Grep `/tmp/planner_verbose.log` para los separadores "═══"
- Confirmar que el system prompt contiene `"test_agent": Test` section
- Verificar que raw response está legible

---

### Caso 3.7: Thinking budget se propaga al LLM (si soportado)

**Objetivo:** Verificar que cuando se especifica `thinking_budget`, se pasa correctamente a `llm_config.with_thinking_budget()`.

**Grafo mínimo (JSON) — Anthropic con Claude 3.7+ (soporta thinking):**
```json
{
  "nodes": [
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "anthropic",
        "api_key": "${ANTHROPIC_API_KEY}",
        "model": "claude-3-7-sonnet-20250219",
        "thinking_budget": 5000,
        "agents": [
          { "name": "agent_1", "description": "Test" }
        ]
      },
      "inputs": {
        "request": "Complex planning task"
      }
    }
  ],
  "edges": []
}
```

**Ejecución:**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_thinking.json
```

**Resultado esperado:**
- Output contiene plan estructurado
- SSE stream (si se captura) mostraría thinking tokens antes del plan

**Cómo se verifica:**
- Capturar SSE a archivo: `... > /tmp/thinking.sse`
- Grep para `thinking-delta` frames (si el provider emite)
- Confirmar que plan se genera normalmente

---

### Caso 3.8: Streaming mode emite tokens en vivo

**Objetivo:** Verificar que cuando `streaming: true`, el nodo emite eventos `LlmToken` y `LlmUsage` en tiempo real.

**Grafo mínimo (JSON):**
```json
{
  "nodes": [
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "streaming": true,
        "agents": [
          { "name": "agent_1", "description": "Test" }
        ]
      },
      "inputs": {
        "request": "Plan a simple task"
      }
    }
  ],
  "edges": []
}
```

**Ejecución:**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_streaming.json 2>&1 | tee /tmp/planner_stream.sse
```

**Resultado esperado:**
- SSE stream contiene múltiples `llm-token-delta` events (uno por token)
- SSE stream contiene `llm-message-start` y `llm-message-finish`
- SSE stream contiene `llm-usage` event con token counts

**Cómo se verifica:**
- Parsear `/tmp/planner_stream.sse` como líneas SSE
- Contar eventos `llm-token-delta` — debe haber muchos (>10 típicamente)
- Buscar `llm-usage` event

---

### Caso 3.9: Agents como strings (bare node IDs)

**Objetivo:** Verificar que agents puede ser un array de strings (node IDs) en lugar de objetos, y que el código busca descriptions en `__graph_nodes`.

**Grafo mínimo (JSON):**
```json
{
  "nodes": [
    {
      "id": "agent_1",
      "type": "log",
      "config": {
        "message": "Dummy agent 1"
      },
      "inputs": {}
    },
    {
      "id": "agent_2",
      "type": "log",
      "config": {
        "message": "Dummy agent 2"
      },
      "inputs": {}
    },
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "agents": ["agent_1", "agent_2"]
      },
      "inputs": {
        "request": "Assign tasks to agent_1 or agent_2"
      }
    }
  ],
  "edges": []
}
```

**Ejecución:**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_string_agents.json
```

**Resultado esperado:**
- Plan generado exitosamente
- `assigned_to` constrained a `["agent_1", "agent_2"]`
- Si las descriptions están en `__graph_nodes`, se usan; si no, "No description provided." aparece en el prompt

**Cómo se verifica:**
- Parsear output y confirmar tareas tienen `assigned_to` en la lista de strings
- Verbose mode para verificar descriptions en el prompt

---

### Caso 3.10: Model default por provider

**Objetivo:** Verificar que cuando `model` no se especifica, el código usa un default según el provider.

**Grafo mínimo (JSON):**
```json
{
  "nodes": [
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "agents": [
          { "name": "agent_1", "description": "Test" }
        ]
      },
      "inputs": {
        "request": "Simple plan"
      }
    }
  ],
  "edges": []
}
```

**Ejecución:**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_default_model.json
```

**Resultado esperado:**
- Plan generado exitosamente (sin error de missing model)
- Provider usa su model default (Google → gemini-2.5-flash por defecto, etc.)

**Cómo se verifica:**
- Confirmar que no hay error "Model is required" o similar
- Output es válido con plan

---

### Caso 3.11: Extra system_message appended a built-in prompt

**Objetivo:** Verificar que `system_message` (config) o `system_message` (input) se añaden al built-in planner prompt.

**Grafo mínimo (JSON):**
```json
{
  "nodes": [
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "system_message": "Always prioritize tasks by budget efficiency.",
        "agents": [
          { "name": "agent_1", "description": "Budget optimizer" }
        ]
      },
      "inputs": {
        "request": "Plan a project"
      }
    }
  ],
  "edges": []
}
```

**Ejecución (with verbose):**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_extra_system.json --verbose 2>&1 | grep -A 50 "System Prompt Sent"
```

**Resultado esperado:**
- System prompt contiene "Always prioritize tasks by budget efficiency."
- System prompt contiene el built-in planner rules
- Plan generado según el custom guidance

**Cómo se verifica:**
- Grep para "Always prioritize tasks by budget efficiency"
- Verificar que plan generado refleja la instrucción

---

### Caso 3.12: Invalid provider error

**Objetivo:** Verificar que si `provider` es inválido, el nodo falla con un error claro.

**Grafo mínimo (JSON):**
```json
{
  "nodes": [
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "invalid_provider",
        "api_key": "${GEMINI_API_KEY}",
        "agents": [
          { "name": "agent_1", "description": "Test" }
        ]
      },
      "inputs": {
        "request": "Plan"
      }
    }
  ],
  "edges": []
}
```

**Ejecución:**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_bad_provider.json 2>&1
```

**Resultado esperado:**
- Execution fails with error message: "PlannerNode: Invalid provider 'invalid_provider'."
- Error es fail-closed (node does NOT return NULL or partial plan)

**Cómo se verifica:**
- Grep for "Invalid provider"
- Confirm exit code is non-zero

---

### Caso 3.13: Missing required fields (provider, api_key)

**Objetivo:** Verificar fail-closed behavior cuando faltan campos requeridos.

**Grafo mínimo (JSON) — missing api_key:**
```json
{
  "nodes": [
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "google"
      },
      "inputs": {
        "request": "Plan"
      }
    }
  ],
  "edges": []
}
```

**Ejecución:**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_missing_api_key.json 2>&1
```

**Resultado esperado:**
- Error: "PlannerNode: Missing 'api_key' in config"
- Fail-closed (no partial output)

**Cómo se verifica:**
- Grep for "Missing 'api_key'"
- Confirm exit code is non-zero

---

### Caso 3.14: LLM echoes schema back (defensive parsing)

**Objetivo:** Verificar que cuando el LLM (ej. gpt-4o-mini) devuelve `{ "type": "array", "items": [...] }` en lugar de una array simple, el código lo desenvuelve.

**Grafo mínimo (JSON):**
```json
{
  "nodes": [
    {
      "id": "planner_node",
      "type": "planner",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-4o-mini",
        "agents": [
          { "name": "agent_1", "description": "Test" }
        ]
      },
      "inputs": {
        "request": "Simple plan (choose a provider known to sometimes echo schema)"
      }
    }
  ],
  "edges": []
}
```

**Ejecución:**
```bash
source .env
cargo run --bin dag_engine -- run test_planner_schema_echo.json 2>&1
```

**Resultado esperado:**
- Si OpenAI echa schema, código detecta `{ "type": "array", "items": [...] }` patrón (líneas 433-441)
- Desenvuelve `items` array y lo trata como task list
- Log contiene "⚠️  [PlannerNode] LLM echoed schema wrapper"
- Output es plan válido, NO schema

**Cómo se verifica:**
- Grep for "echoed schema wrapper" (si ocurre)
- Confirmar que output es array de tasks, no schema

