# critic — Auditoría QA (Documentación vs Código)

**Nodo:** `critic`  
**Código fuente:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/critic.rs`  
**Documentación primaria:** `docs/developer_guide/20_orchestrator_architecture.md` (Critic Feedback Loop)  
**Configuración canónica:** `docs/node_configurations.json` → `node_types.critic`  
**Fecha de auditoría:** 2026-08-30

---

## 1. Hallazgos: Documentación

### 1.1 node_configurations.json — campos de config incompletos

**Problema:** `docs/node_configurations.json` documenta los campos `provider`, `api_key`, `model`, `system_message`, `verbose`, `texts` en la sección `critic.config_fields`.

**Realidad en código:** `critic.rs` líneas 188-190 y 224-227 revelan que el nodo TAMBIÉN soporta:
- `thinking_budget` (u64, opcional): línea 188-190 llama `llm_config.with_thinking_budget(budget as u32)`
- `streaming` (boolean, default false): línea 224-227 controla si el nodo emite tokens en tiempo real

**Impacto:** Alto. Un operador que quiera usar `thinking_budget` para habilitar pensamiento extendido en el Critic no encuentra el campo documentado en `node_configurations.json` y debe leer el código fuente o adivinar.

**Remediación:** Agregar `thinking_budget` y `streaming` a `docs/node_configurations.json` → `node_types.critic.config_fields` con descripciones y ejemplos.

---

### 1.2 node_configurations.json — temperatura fija NO documentada

**Problema:** `docs/node_configurations.json` describe los 6 campos de config pero NO menciona que la temperatura está fija a 0.1.

**Realidad en código:** `critic.rs` línea 187: `llm_config = llm_config.with_temperature(0.1)?;`

**Verificación en docs:** La descripción en `node_configurations.json` dice "Temperature is forced to 0.1 for consistent evaluations" (en la descripción general del nodo), pero no está en un campo `config_fields` visible como algo que el usuario DEBA SABER.

**Impacto:** Bajo-Medio. Un usuario que intente pasar `temperature` en config recibirá un error silencioso (se ignora). Mejor sería documentar explícitamente que la temperatura NO es configurable.

**Recomendación:** Agregar una nota en la descripción general: "⚠️ Temperature is forced to 0.1; custom temperatures are not supported for consistent evaluation behavior."

---

### 1.3 node_ports_reference.md — descripción superficial

**Problema:** `docs/agent_context/node_ports_reference.md` línea 47 describe el critic como "LLM reviews outputs; returns pass/fail assessment" y la tabla detallada (línea donde dice "`critic`") menciona que `result` es boolean y `extra_info` lleva los detalles, pero NO explica el comportamiento cuando no hay textos.

**Realidad en código:** `critic.rs` líneas 145-148:
```rust
if formatted_texts.is_empty() {
    colmena_log!("⚠️ [CriticNode] Skipped — no input texts provided.");
    return Ok(Value::Null);
}
```
El nodo **retorna `null`** cuando no hay textos que revisar (fail-closed silencioso).

**Impacto:** Medio. Un grafo que confíe en que el critic siempre devuelve `{ result: boolean, extra_info }` pero olvide pasar textos recibirá `null` en lugar del esperado objeto, lo que puede romper downstream edges que esperan un boolean en `result`.

**Remediación:** Documentar en `node_ports_reference.md` que si no se proporcionan textos (`texts.*` inputs y config `texts`), el nodo retorna `null` (no `{ result: null }`).

---

### 1.4 node_as_tools_reference.json — SIN entrada para critic

**Problema:** `docs/node_as_tools_reference.json` tiene 0 caracteres de contenido referente a `critic` (confirmado: `grep '"critic"' docs/node_as_tools_reference.json` retorna vacío).

**Realidad:** El nodo crítico se usa frecuentemente como una herramienta dentro de un `llm_call` para que el LLM revise sus propio resultado, y como nodo embebido en el orchestrator. Ambas son configuraciones como herramienta.

**Impacto:** Alto. Operadores que busquen "¿cómo configuro `critic` en `tool_configurations`?" no encuentran ejemplo canónico y deben inferir desde `20_orchestrator_architecture.md`.

**Remediación:** Agregar `critic` a `docs/node_as_tools_reference.json` con ejemplos de dos patrones: (A) fijo en config, (B) dinámico con textos de entrada.

---

### 1.5 developer_guide/20_orchestrator_architecture.md — JSON de ejemplo incompleto

**Problema:** `docs/developer_guide/20_orchestrator_architecture.md` línea ~200-210 (sección "Critic Feedback Loop") muestra un ejemplo JSON:
```json
{
  "task_ok": false,
  "feedback": "El itinerario no incluye precios específicos...",
  "suspend": false
}
```

pero el código `critic.rs` líneas 318 muestra que también retorna un campo `question` (incluso cuando `suspend=false`, puede estar presente).

**Realidad en código:** `critic.rs` línea 318: `let question = parsed.get("question").cloned().unwrap_or(Value::Null);` 
El nodo ESPERA un campo `question` en el schema (línea 45: `"required": ["task_ok", "feedback", "suspend", "question"]`).

**Impacto:** Bajo. El ejemplo funciona, pero un usuario que intente reproducirlo exactamente sin el campo `question` recibirá un valor `null` en ese campo del output (no un error, porque el JSON parsing es lenient).

**Recomendación:** Actualizar el ejemplo para incluir `question` (puede ser string vacío si `suspend=false`).

---

### 1.6 Documentación del stripping de markdown — NO mencionado

**Problema:** `critic.rs` líneas 287-296 implementa automáticamente el stripping de bloques de código markdown:
```rust
if clean.starts_with("```json") {
    clean = clean.trim_start_matches("```json");
} else if clean.starts_with("```") {
    clean = clean.trim_start_matches("```");
}
if clean.ends_with("```") {
    clean = clean.trim_end_matches("```");
}
```

Ninguna de las documentaciones menciona que el nodo limpia automáticamente markdown fences de la respuesta LLM antes de parserarla como JSON.

**Impacto:** Bajo-Medio. Un usuario que ve que el LLM devuelve `\`\`\`json\n{...}\n\`\`\`` y se preocupa por un fallo de parsing puede estar sorprendido (agradablemente) de que funcione. Pero es un comportamiento de defensa que merece ser documentado.

**Recomendación:** Agregar nota en `docs/developer_guide/20_orchestrator_architecture.md`: "El Critic automáticamente limpia bloques markdown (` ```json ... ``` `) de la respuesta LLM antes de parsear el JSON; el LLM puede devolver código bloqueado sin que falle."

---

## 2. Hallazgos: Código

### 2.1 Validación fail-closed: provider, api_key, formatted_texts

**Validación:** `critic.rs` líneas 86-96, 98-102, 145-148 implementan tres gates fail-closed:
1. Provider debe ser uno de: `openai`, `google`, `anthropic` (case-insensitive matching línea 91)
2. API key DEBE estar presente (no hay fallback a env vars automático)
3. Sin textos → retorna `null` (no error, comportamiento defensivo)

**Verificación:** Línea 89 lanza `"CriticNode: Missing 'provider' in config"`, línea 101 lanza `"CriticNode: Missing 'api_key' in config"`. Coherentes con la documentación que marca ambos como `required: true`.

**Estado:** OK. El comportamiento es correcto y alineado con las validaciones esperadas.

---

### 2.2 Resolución de env vars — patrón ${...} documentado

**Código:** `critic.rs` líneas 65-73 implementan `resolve_env_var`, llamado en línea 102. Soporta sintaxis `${VAR_NAME}` únicamente (ej. `${OPENAI_API_KEY}`).

**Verificación:** `docs/node_configurations.json` campo `api_key` menciona "Supports '${VAR_NAME}'." Correcto.

**Estado:** OK.

---

### 2.3 default_output es "result" — alineado con documentación

**Código:** `critic.rs` líneas 343-345:
```rust
fn default_output(&self) -> Option<&str> {
    Some("result")
}
```

**Verificación:** `docs/node_configurations.json` → `critic.default_output` = `"result"`. Coherente.

**Estado:** OK.

---

### 2.4 Output structure — `{ result, extra_info }` documentado

**Código:** `critic.rs` líneas 327-336:
```rust
Ok(json!({
    "result": task_ok,
    "extra_info": {
        "task_ok":  task_ok,
        "feedback": feedback,
        "suspend":  suspend,
        "question": question,
        "__colmena_status": if suspend { "SUSPENDED" } else { "OK" }
    }
}))
```

El resultado es SIEMPRE un objeto (nunca NULL aquí; solo retorna NULL si no hay textos línea 147).

**Verificación:** `docs/node_configurations.json` → `critic.output_ports` describe:
- `result`: boolean (task_ok decision)
- `extra_info`: object con los 5 campos

Correcto.

**Estado:** OK.

---

### 2.5 Schema siempre requerido — no configurable

**Código:** `critic.rs` línea 163 crea un schema fijo via `critic_schema()` (líneas 24-47). Este schema está hardcoded y no es modificable por config.

**Verificación:** La documentación (node_configurations.json) no menciona ningún campo de config para "custom schema" — correctamente, porque está hardcoded.

**Nota:** A diferencia de `information_extraction` (que permite un `schema` dinámico en config), `critic` tiene un schema fijo. Esto es intencional: el Critic siempre devuelve `task_ok`, `feedback`, `suspend`, `question`.

**Estado:** OK. Comportamiento esperado y bien documentado implícitamente.

---

### 2.6 Conversación en memoria efímera — no persiste

**Código:** `critic.rs` líneas 193-200 crean una `InMemoryConversationRepository` (no persistida) y un `ConversationKey` con UUIDs generados aleatoriamente:
```rust
let conversation_repo = Arc::new(InMemoryConversationRepository::new());
let tid_str = uuid::Uuid::new_v4().to_string();
let tid = ConversationKey {
    session_id: SessionId(tid_str.clone()),
    agent_session_id: None,
    node_id: NodeIdPath(tid_str),
};
```

Cada llamada al Critic crea una conversación NUEVA, aislada, sin historial previo.

**Verificación:** Documentación (20_orchestrator_architecture.md) no menciona persistencia de historia en el Critic. Correcto — el Critic no mantiene memoria entre llamadas; es stateless.

**Estado:** OK. Comportamiento intencional.

---

## 3. Casos de Prueba Ejecutables

Todos los casos usan `cargo run --bin dag_engine -- run <graph.json>` con `--agent-session-id` para keying de estado.

### 3.1 Test A: Crítica aprobatoria (task_ok=true)

**Archivo:** `tests/graphs/agents/critic_approval.json`

```json
{
  "nodes": {
    "input": {
      "type": "mock_input",
      "config": {
        "task_description": "Crear un plan de viaje a Roma",
        "agent_result": "Plan: Día 1 - Coliseo, Día 2 - Vaticano, Día 3 - Foro Romano"
      }
    },
    "reviewer": {
      "type": "critic",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "verbose": false
      }
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "input.task_description", "to": "reviewer.texts.task" },
    { "from": "input.agent_result", "to": "reviewer.texts.result" },
    { "from": "reviewer", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
source .env && cargo run --bin dag_engine -- run tests/graphs/agents/critic_approval.json --agent-session-id test_a_critic_001
```

**Validación esperada:**
- Critic recibe dos textos: tarea y resultado del agente
- Output: `{ result: true, extra_info: { task_ok: true, feedback: "", suspend: false, question: null, __colmena_status: "OK" } }`
- Log emite la decisión: `task_ok=true`

---

### 3.2 Test B: Crítica rechaza con feedback

**Archivo:** `tests/graphs/agents/critic_feedback_loop.json`

```json
{
  "nodes": {
    "input": {
      "type": "mock_input",
      "config": {
        "task_description": "Proporciona precios en EUR",
        "agent_result": "El plan incluye visitas pero SIN precios específicos."
      }
    },
    "critic_eval": {
      "type": "critic",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "system_message": "Rechaza cualquier resultado que no incluya información de precios. Sé muy estricto."
      }
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "input.task_description", "to": "critic_eval.texts.requirement" },
    { "from": "input.agent_result", "to": "critic_eval.texts.result" },
    { "from": "critic_eval", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
source .env && cargo run --bin dag_engine -- run tests/graphs/agents/critic_feedback_loop.json --agent-session-id test_b_critic_001
```

**Validación esperada:**
- Critic rechaza porque el resultado no tiene precios
- Output: `{ result: false, extra_info: { task_ok: false, feedback: "Agrega precios específicos en EUR para cada actividad", suspend: false, question: null, __colmena_status: "OK" } }`
- El campo `feedback` contiene instrucciones accionables para reintento

---

### 3.3 Test C: Crítica suspende para aclaración del usuario

**Archivo:** `tests/graphs/agents/critic_suspend_question.json`

```json
{
  "nodes": {
    "input": {
      "type": "mock_input",
      "config": {
        "task_description": "Proporciona opciones de alojamiento",
        "agent_result": "Opciones: Hotel 5 estrellas (250€/noche) o Airbnb económico (50€/noche)"
      }
    },
    "critic_ask": {
      "type": "critic",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "system_message": "El resultado tiene dos opciones de muy diferentes precios. Si no está clara la preferencia del usuario, suspende y pregunta qué rango presupuestario prefiere (económico, medio, lujo)."
      }
    },
    "log": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "input.task_description", "to": "critic_ask.texts.task" },
    { "from": "input.agent_result", "to": "critic_ask.texts.options" },
    { "from": "critic_ask", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
source .env && cargo run --bin dag_engine -- run tests/graphs/agents/critic_suspend_question.json --agent-session-id test_c_critic_001
```

**Validación esperada:**
- Critic decide que necesita aclaración del usuario
- Output: `{ result: false, extra_info: { task_ok: false, suspend: true, question: "¿Cuál es tu rango presupuestario: económico, medio o lujo?", __colmena_status: "SUSPENDED" } }`
- El campo `__colmena_status` es `"SUSPENDED"` (señal para el orchestrator)

---

## Resumen de Hallazgos

| # | Tipo | Severidad | Descripción |
|---|------|-----------|-------------|
| 1.1 | Docs | Alta | `node_configurations.json` no documenta `thinking_budget` ni `streaming` |
| 1.2 | Docs | Media | Temperatura fija (0.1) NO está explícitamente documentada como no-configurable |
| 1.3 | Docs | Media | `node_ports_reference.md` no explica comportamiento cuando no hay textos (retorna `null`) |
| 1.4 | Docs | Alta | `node_as_tools_reference.json` SIN entrada para `critic` |
| 1.5 | Docs | Baja | Ejemplo JSON en 20_orchestrator_architecture.md falta el campo `question` |
| 1.6 | Docs | Baja | Stripping automático de markdown fences NO mencionado en documentación |
| 2.1 | Código | OK | Validaciones fail-closed (provider, api_key, textos) son correctas |
| 2.2 | Código | OK | Resolución de env vars soporta `${VAR_NAME}` como documentado |
| 2.3 | Código | OK | `default_output = "result"` alineado |
| 2.4 | Código | OK | Estructura de output `{ result, extra_info }` documentada |
| 2.5 | Código | OK | Schema fijo (no configurable) es intencional |
| 2.6 | Código | OK | Conversación efímera (in-memory) es correcta |

---

## Remediaciones Recomendadas

### Prioridad ALTA (bloquea desarrollo)

1. **Agregar `thinking_budget` y `streaming` a `docs/node_configurations.json`** → sección `critic.config_fields`:
   - `thinking_budget`: "Optional budget (tokens) for extended thinking on the critic evaluation. Enables Claude models to think longer for complex quality reviews."
   - `streaming`: "Enable real-time token streaming for critic output. When true, tokens are emitted via the DAG observer."

2. **Agregar `critic` a `docs/node_as_tools_reference.json`** con ejemplos de configuración como herramienta (fijo en config vs dinámico con textos).

### Prioridad MEDIA (afecta discovery)

3. **Actualizar `docs/agent_context/node_ports_reference.md`** línea 47 (descripción de critic) para mencionar: "Returns `null` if no input texts are provided."

4. **Agregar nota en temperatura fija** en `docs/node_configurations.json` → descripción general de critic: "⚠️ Temperature is forced to 0.1 for consistent evaluation; custom temperatures are not supported."

### Prioridad BAJA (legibilidad)

5. **Documentar stripping de markdown** en `docs/developer_guide/20_orchestrator_architecture.md` (sección Critic Feedback Loop): mencionar que el nodo automáticamente limpia ` ```json ... ``` ` fences.

6. **Actualizar ejemplo JSON** en `docs/developer_guide/20_orchestrator_architecture.md` para incluir el campo `question` (vacío cuando `suspend=false`).

---

**Auditoría completada:** 6 hallazgos en documentación (2 alta, 2 media, 2 baja) + 6 aspectos de código validados (todos OK) + 3 casos de prueba ejecutables.
