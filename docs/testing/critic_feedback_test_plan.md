# Plan de Testing: Critic Feedback Injection

## Contexto

Se implementaron los siguientes cambios al Critic y al Orquestador:

1. **`critic.rs`** — El campo `add_tasks` fue reemplazado por `feedback` (string). Cuando `task_ok=false`, el Critic debe escribir qué falló y qué debe hacer diferente el agente.
2. **`orchestrator.rs`** — El feedback del Critic se guarda en `_state` como `__orch_critic_feedback_<task_id>`. En el siguiente intento, se inyecta en el prompt del agente como una nueva sección `=== INTENTO ANTERIOR — POR QUÉ FALLÓ ===`, justo antes de `=== LO QUE TIENES QUE HACER AHORA TÚ ===`.
3. **Limpieza** — El `feedback_key` se elimina de state cuando la tarea es aprobada, aceptada por el usuario (accept), saltada (skip) o cancelada (cancel).

## Estructura de prompt resultante (en reintentos)

```
=== USER CLARIFICATION ===         ← solo si el Critic suspendió y el usuario respondió
<respuesta del usuario>

=== CONTEXTO DE ESTA TAREA ===
<contexto del planner>

=== LO QUE HA OCURRIDO HASTA AHORA ===
Fase 1: <resumen>

=== INTENTO ANTERIOR — POR QUÉ FALLÓ ===   ← NUEVO — solo en reintentos
<feedback del Critic: qué faltó, qué hacer diferente>

=== LO QUE TIENES QUE HACER AHORA TÚ ===
<instrucción de tarea>
```

## Formato de referencia para los grafos

```json
{
  "nodes": {
    "trigger": {
      "type": "input",
      "config": { "prompt": "<prompt del usuario>" }
    },
    "orchestrator_node": {
      "type": "orchestrator",
      "config": {
        "verbose": true,
        "max_phases": 3,
        "planner": {
          "provider": "google",
          "model": "gemini-2.5-flash",
          "api_key": "${GEMINI_API_KEY}",
          "allow_suspend": false,
          "system_message": "<instrucciones del planner>"
        },
        "agents": {
          "<agent_id>": {
            "provider": "google",
            "model": "gemini-2.5-flash",
            "api_key": "${GEMINI_API_KEY}",
            "system_message": "<system message del agente>"
          }
        },
        "critic": {
          "provider": "google",
          "model": "gemini-2.5-flash",
          "api_key": "${GEMINI_API_KEY}",
          "allow_suspend": true,
          "max_retries": 3,
          "system_message": "<instrucciones del critic>"
        },
        "phase_reactor": {
          "provider": "google",
          "model": "gemini-2.5-flash",
          "api_key": "${GEMINI_API_KEY}",
          "allow_suspend": false
        },
        "final_reactor": {
          "provider": "google",
          "model": "gemini-2.5-flash",
          "api_key": "${GEMINI_API_KEY}",
          "allow_suspend": false
        }
      }
    },
    "final_output": {
      "type": "output",
      "trigger_on": "FINISHED"
    }
  },
  "edges": [
    { "from": "trigger", "to": "orchestrator_node" },
    { "from": "orchestrator_node", "to": "final_output" }
  ]
}
```

---

## Test 1 — `critic_feedback_injection_test.json`

**Objetivo:** Verificar que `=== INTENTO ANTERIOR — POR QUÉ FALLÓ ===` aparece en el prompt del agente en el segundo intento con el texto de feedback del Critic.

**Ruta:** `tests/graphs/advanced/critic_feedback_injection_test.json`

**Prompt del usuario:** `"Provide a weather report for Madrid."`

**Planner system_message:**
```
Create exactly ONE task: assign 'Write a detailed weather report for Madrid' to weather_agent in phase 1.
Output ONLY a JSON array with one item.
```

**Agent (`weather_agent`) system_message:**
```
You are a weather assistant. Write a brief weather report for Madrid.
Keep it to 2-3 sentences. Do not include specific numbers unless explicitly asked.
```
*(El agente es vago intencionalmente para que el Critic lo rechace en el primer intento)*

**Critic system_message:**
```
You are a strict weather data critic. A weather report is ONLY acceptable if it contains ALL of the following as explicit numbers:
1. Temperature in degrees Celsius (e.g. "15°C" or "15 degrees Celsius")
2. Wind speed in km/h (e.g. "20 km/h")
3. Humidity as a percentage (e.g. "65%")

If ANY of these three values is missing or not expressed as a number:
- Set task_ok=false
- Set suspend=false
- Write feedback listing exactly which values are missing and instructing: "On your next attempt you MUST include: [list missing items] as explicit numbers in your report."

If ALL three values are present as numbers: set task_ok=true, feedback="".
Never suspend.
```

**max_retries:** 3  
**allow_suspend (critic):** false

**Cómo ejecutar:**
```bash
cargo run --bin dag_engine -- run tests/graphs/advanced/critic_feedback_injection_test.json
```

**Qué verificar en los logs (`verbose: true`):**

1. **Primer intento** — La línea `📨 [OrchestratorNode] PROMPT → agent 'weather_agent'` NO contiene `=== INTENTO ANTERIOR`.
2. **El Critic rechaza** — Log: `🔎 [CriticNode] Decision → task_ok=false, has_feedback=true, suspend=false`.
3. **Segundo intento** — La línea `📨 [OrchestratorNode] PROMPT → agent 'weather_agent'` contiene:
   ```
   === INTENTO ANTERIOR — POR QUÉ FALLÓ ===
   On your next attempt you MUST include: ...
   ```
4. **El Critic aprueba** — Log: `🔎 [CriticNode] Decision → task_ok=true, has_feedback=false, suspend=false`.
5. **El grafo termina** — Log: `__colmena_loop_status: FINISHED`.

**Resultado esperado:** El agente mejora su respuesta en el segundo intento al recibir el feedback. El grafo termina con `FINISHED`.

---

## Test 2 — `critic_feedback_multiretry_test.json`

**Objetivo:** Verificar que el feedback se ACUMULA correctamente en reintentos consecutivos (el último feedback siempre sobreescribe el anterior, no se apilan).

**Ruta:** `tests/graphs/advanced/critic_feedback_multiretry_test.json`

**Prompt del usuario:** `"Analyze the performance of Tesla stock in 2024."`

**Planner system_message:**
```
Create exactly ONE task: assign 'Analyze Tesla stock performance in 2024' to analyst_agent in phase 1.
Output ONLY a JSON array with one item.
```

**Agent (`analyst_agent`) system_message:**
```
You are a financial analyst. Analyze Tesla stock in 2024.
Write 3-4 sentences. Focus on qualitative trends, not specific numbers.
```

**Critic system_message:**
```
You are a strict financial data critic. Accept a stock analysis ONLY if it contains ALL of:
1. At least one specific price figure (e.g. "$200", "$180.50")
2. At least one percentage change (e.g. "+15%", "-8.3%")
3. The word "Q1", "Q2", "Q3", or "Q4" referencing a specific quarter

For each missing element, list it explicitly in feedback. For example:
- "Missing: specific price figure. Include the stock price at start or end of 2024."
- "Missing: percentage change. Include the annual gain or loss as a percentage."
- "Missing: quarterly reference. Mention at least one specific quarter."

Set task_ok=false, suspend=false when anything is missing.
Set task_ok=true, feedback="" only when ALL three are present.
```

**max_retries:** 5  
**allow_suspend (critic):** false

**Cómo ejecutar:**
```bash
cargo run --bin dag_engine -- run tests/graphs/advanced/critic_feedback_multiretry_test.json
```

**Qué verificar:**
1. Primer intento: sin sección `=== INTENTO ANTERIOR`.
2. Cada intento sucesivo: la sección `=== INTENTO ANTERIOR — POR QUÉ FALLÓ ===` contiene solo el feedback del ÚLTIMO critic (no concatenados).
3. El feedback cambia entre intentos a medida que el agente va corrigiendo cosas.
4. En algún intento el agente pasa todos los criterios y el Critic aprueba.

---

## Test 3 — `critic_feedback_with_suspend_test.json`

**Objetivo:** Verificar la coexistencia de `=== INTENTO ANTERIOR ===` y `=== USER CLARIFICATION ===` en el mismo prompt cuando el Critic rechaza primero y luego suspende.

**Ruta:** `tests/graphs/advanced/critic_feedback_with_suspend_test.json`

**Prompt del usuario:** `"Write a travel guide for Tokyo."`

**Planner system_message:**
```
Create exactly ONE task: assign 'Write a travel guide for Tokyo' to travel_agent in phase 1.
Output ONLY a JSON array with one item.
```

**Agent (`travel_agent`) system_message:**
```
You are a travel writer. Write a brief travel guide for Tokyo in 4-5 sentences.
Do not ask about traveler preferences unless specifically instructed.
```

**Critic system_message:**
```
You are a travel guide critic. Follow these rules in order:

RULE 1 — First review (no USER CLARIFICATION section present):
  Always reject the first attempt. Set task_ok=false, suspend=false.
  feedback="Your guide must include: (1) the best time of year to visit Tokyo in months, (2) at least one specific neighborhood name, (3) estimated daily budget in USD. Add these to your next attempt."

RULE 2 — Second review (USER CLARIFICATION section IS present):
  Set task_ok=true, feedback="". Approve regardless of content.

RULE 3 — If the guide includes all three elements from RULE 1 (months, neighborhood, USD budget):
  Set task_ok=true, feedback="".

Use suspend=true ONLY if you need to ask the user something before continuing.
For this test: after the first rejection, if the agent still does not include the required elements, set suspend=true with question="The travel agent needs guidance: should the guide focus on budget travelers or luxury travelers? This will determine the recommended budget range."
```

**max_retries:** 5  
**allow_suspend (critic):** true

**Flujo esperado:**
1. Intento 1 → Critic rechaza (task_ok=false, feedback sobre los 3 elementos faltantes)
2. Intento 2 → agente recibe `=== INTENTO ANTERIOR ===` con el feedback → puede mejorar o no
3. Si no mejora suficiente → Critic suspende con pregunta al usuario
4. Usuario responde → agente recibe `=== USER CLARIFICATION ===` + `=== INTENTO ANTERIOR ===`
5. Intento final → Critic aprueba

**Cómo ejecutar (paso a paso):**
```bash
# Paso 1 — primera ejecución
cargo run --bin dag_engine -- run tests/graphs/advanced/critic_feedback_with_suspend_test.json

# Si suspende con question sobre budget vs luxury:
# Paso 2 — responder
cargo run --bin dag_engine -- run tests/graphs/advanced/critic_feedback_with_suspend_test.json \
  --session-id <SESSION_ID_DEL_PASO_1> \
  --answer "Focus on budget travelers, daily budget around 80-100 USD"
```

**Qué verificar en logs:**
1. Intento 1: prompt sin `=== INTENTO ANTERIOR ===`.
2. Intento 2: prompt CON `=== INTENTO ANTERIOR — POR QUÉ FALLÓ ===`.
3. Si hay suspend → el usuario responde → intento siguiente tiene AMBAS secciones:
   - `=== USER CLARIFICATION ===`
   - `=== INTENTO ANTERIOR — POR QUÉ FALLÓ ===`

---

## Test 4 — `critic_feedback_cleanup_test.json`

**Objetivo:** Verificar que el `feedback_key` se limpia del state cuando la tarea es resuelta manualmente (accept / skip / cancel tras max_retries).

**Ruta:** `tests/graphs/advanced/critic_feedback_cleanup_test.json`

**Usar el mismo grafo del Test 2** pero con `max_retries: 2` para forzar rápido la suspensión de max_retries.

**Cómo ejecutar:**
```bash
# Paso 1 — ejecutar hasta max_retries
cargo run --bin dag_engine -- run tests/graphs/advanced/critic_feedback_cleanup_test.json

# Paso 2A — accept (verificar que la siguiente tarea en otra fase no hereda feedback)
cargo run --bin dag_engine -- run tests/graphs/advanced/critic_feedback_cleanup_test.json \
  --session-id <ID> --answer 'accept'

# Paso 2B — skip
cargo run --bin dag_engine -- run tests/graphs/advanced/critic_feedback_cleanup_test.json \
  --session-id <ID> --answer 'skip'

# Paso 2C — cancel  
cargo run --bin dag_engine -- run tests/graphs/advanced/critic_feedback_cleanup_test.json \
  --session-id <ID> --answer 'cancel'

# Paso 2D — retry con instrucciones del usuario
cargo run --bin dag_engine -- run tests/graphs/advanced/critic_feedback_cleanup_test.json \
  --session-id <ID> --answer 'retry Include specific Q1 2024 opening price of $248 and the annual drop of -50%'
```

**Qué verificar (con `--include-extra-info`):**
- En accept/skip/cancel: el grafo termina con `FINISHED` sin `=== INTENTO ANTERIOR ===` en prompts de otras tareas de la misma sesión.
- En retry: el agente recibe `=== USER CLARIFICATION ===` con las instrucciones del usuario. El `=== INTENTO ANTERIOR ===` del critic previo NO aparece (fue limpiado al hacer `retry` desde max_retries — el retry del usuario reemplaza el feedback).

---

## Notas para el agente que cree los grafos

- Todos los grafos usan `"provider": "google"` y `"model": "gemini-2.5-flash"` con `"api_key": "${GEMINI_API_KEY}"`.
- `"verbose": true` es obligatorio para ver los prompts en los logs.
- El `_comment` y `_test_instructions` deben incluirse como arrays de strings en el JSON (igual que los grafos existentes en `tests/graphs/advanced/`).
- Guardar en `tests/graphs/advanced/` con los nombres indicados arriba.
- Los grafos existentes de referencia (mismo formato): `hitl_critic_answer_rerun_test.json`, `hitl_critic_max_retries_test.json`.
