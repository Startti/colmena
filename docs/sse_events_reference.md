# Referencia de Eventos SSE

El motor DAG emite un stream de eventos SSE (o líneas a stdout en modo `run`). Cada línea tiene el formato:

```
data: {JSON}\n\n
```

Al final del stream siempre aparece `data: [DONE]`.

---

## Modelo de dos niveles

El stream tiene **dos niveles de eventos**:

| Nivel | Prefijo | Qué representa |
|-------|---------|---------------|
| **Top-level** | (sin prefijo) | Nodos del grafo principal |
| **Subgrafo** | `subgraph-` | Nodos dentro de un nodo `subgraph` o agentes-tarea del `orchestrator` |

Los eventos de subgrafo nunca se mezclan con los de top-level: si estás dentro de un agente del orchestrator verás `subgraph-text-delta`, nunca `text-delta`.

---

## Eventos top-level

### Ciclo de vida — todos los nodos

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `node-start` | `node_id`, `node_type`, `config`, `inputs` | Antes de ejecutar el nodo |
| `node-end` | `node_id`, `node_type`, `output` | Después de que el nodo completa |

> `inputs` filtra automáticamente claves `__*` y `session_id`.

```json
{ "type": "node-start", "node_id": "llm_1", "node_type": "llm_call", "config": {}, "inputs": { "prompt": "Hola" } }
{ "type": "node-end",   "node_id": "llm_1", "node_type": "llm_call", "output": { "result": "Hola!" } }
```

---

### Texto LLM — nodo `llm_call` con streaming

Emitido cuando un nodo LLM genera texto en streaming.

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `text-start` | `id` | Al primer token de texto del nodo |
| `text-delta` | `id`, `delta` | Por cada token de texto generado |
| `text-end` | `id` | Al finalizar el nodo (node-end) |

El `id` es un UUID que identifica el bloque de texto. Se emite una sola vez en `text-start` y se reutiliza en todos los `text-delta` y en el `text-end`.

```json
{ "type": "text-start", "id": "txt_a1b2c3" }
{ "type": "text-delta", "id": "txt_a1b2c3", "delta": "Hola, " }
{ "type": "text-delta", "id": "txt_a1b2c3", "delta": "¿en qué puedo ayudarte?" }
{ "type": "text-end",   "id": "txt_a1b2c3" }
```

---

### Razonamiento — modelos con `thinking_budget`

Emitido cuando el modelo emite un bloque de razonamiento interno (extended thinking). Aplica a `llm_call` con `thinking_budget` configurado.

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `reasoning-start` | `id` | Apertura de bloque de razonamiento |
| `reasoning-delta` | `id`, `delta` | Token de razonamiento |
| `reasoning-end` | `id` | Cierre del bloque |

> El razonamiento se emite **antes** del texto de respuesta. El frontend puede mostrarlo colapsado o en un panel separado.

```json
{ "type": "reasoning-start", "id": "rsn_x9y8z7" }
{ "type": "reasoning-delta", "id": "rsn_x9y8z7", "delta": "El usuario pregunta sobre..." }
{ "type": "reasoning-end",   "id": "rsn_x9y8z7" }
{ "type": "text-start",      "id": "txt_a1b2c3" }
{ "type": "text-delta",      "id": "txt_a1b2c3", "delta": "Aquí la respuesta..." }
```

---

### Tool calling — nodo `llm_call` con herramientas

Emitido cuando el LLM llama a una herramienta.

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `tool-input-start` | `toolCallId`, `toolName` | Primer chunk de argumentos (una vez por `toolCallId`) |
| `tool-input-delta` | `toolCallId`, `inputTextDelta` | Chunk de argumentos en streaming |
| `tool-input-available` | `toolCallId`, `toolName`, `input` | Argumentos completos y parseados |
| `tool-output-available` | `toolCallId`, `output` | Resultado de ejecutar la herramienta |

Secuencia completa:

```json
{ "type": "tool-input-start",     "toolCallId": "call_abc", "toolName": "search" }
{ "type": "tool-input-delta",     "toolCallId": "call_abc", "inputTextDelta": "{\"q\"" }
{ "type": "tool-input-delta",     "toolCallId": "call_abc", "inputTextDelta": ":\"Rust\"}" }
{ "type": "tool-input-available", "toolCallId": "call_abc", "toolName": "search", "input": { "q": "Rust" } }
{ "type": "tool-output-available","toolCallId": "call_abc", "output": { "results": [...] } }
```

---

### Skill — herramienta `load_skill`

Emitido cuando el LLM invoca la herramienta sintética `load_skill`.

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `skill-loaded` | `nodeId`, `toolCallId`, `skillName`, `reference?`, `source`, `sizeBytes` | Skill cargada exitosamente |

---

### Thinking — LLMs internos del nodo `orchestrator`

Los sub-componentes internos del `orchestrator` (`planner`, `phase_reactor`, `critic`, `final_reactor`) emiten sus tokens como `thinking-delta`. **No** son eventos de subgrafo.

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `thinking-delta` | `node_id`, `node_type`, `delta` | Token de un LLM interno del orchestrator |

El `node_id` identifica cuál sub-componente está pensando y **coincide exactamente** con el `node_id` del `subgraph-node-start`/`subgraph-node-end` que lo enmarca:

```json
{ "type": "subgraph-node-start", "node_id": "planner",       "node_type": "planner"  }
{ "type": "thinking-delta",      "node_id": "planner",       "node_type": "planner",  "delta": "[{\"task\"..." }
{ "type": "thinking-delta",      "node_id": "planner",       "node_type": "planner",  "delta": "\"assigned_to\"..." }
{ "type": "subgraph-node-end",   "node_id": "planner",       "node_type": "planner",  "output": { ... } }
```

Los `node_id` y `node_type` posibles para thinking-delta:

| `node_id` | `node_type` | Sub-componente |
|-----------|-------------|----------------|
| `"planner"` | `"planner"` | Planificador de tareas |
| `"phase_reactor"` | `"reactor"` | Reactor de fin de fase |
| `"final_reactor"` | `"llm_call"` | Reactor final (respuesta al usuario) |
| `"critic_<agente>"` | `"critic"` | Crítico para un agente específico |

> `thinking-delta` **solo** se emite si el sub-componente tiene `"streaming": true` en su config. Sin streaming, el token llega completo en el `output` del `subgraph-node-end`.

---

### Cierre del grafo

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `usage-summary` | `nodes` (array) | Justo antes de `finish` |
| `finish` | `finishReason`, `usage`, `output` | Fin de la ejecución |
| `error` | `errorText` | Error irrecuperable |

#### `usage-summary.nodes`

```json
{
  "node_id": "llm_1",
  "node_type": "llm_call",
  "model": "gpt-4o",
  "provider": "openai",
  "prompt_tokens": 1200,
  "completion_tokens": 340,
  "thinking_tokens": 150,
  "cache_read_tokens": 800,
  "cache_write_tokens": 400,
  "total_tokens": 1690
}
```

> `thinking_tokens`, `cache_read_tokens` y `cache_write_tokens` solo aparecen si su valor es > 0.

#### `finish`

```json
{
  "type": "finish",
  "finishReason": "stop",
  "usage": {
    "promptTokens": 2400,
    "completionTokens": 680,
    "totalTokens": 3330,
    "thinkingTokens": 250,
    "cacheReadTokens": 1600,
    "cacheWriteTokens": 800
  },
  "output": { ... }
}
```

`finishReason`:

| Valor | Cuándo |
|-------|--------|
| `"stop"` | Ejecución completada normalmente |
| `"suspended"` | El grafo se pausó esperando respuesta del usuario |

En caso de `suspended`, `output` contiene:

```json
{
  "__colmena_status": "SUSPENDED",
  "questions": [
    { "id": "origin", "question": "¿Desde dónde viajas?", "type": "open" },
    { "id": "mode",   "question": "¿Avión o tren?",       "type": "choice", "options": ["avión", "tren"] }
  ],
  "session_id": "abc-123"
}
```

---

## Eventos de subgrafo

Todos los eventos dentro de un nodo `subgraph` o de un agente-tarea del `orchestrator` tienen el prefijo `subgraph-`. La semántica es idéntica a los top-level.

### Ciclo de vida

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-node-start` | `node_id`, `node_type`, `config`, `inputs` | Antes de ejecutar un nodo interno |
| `subgraph-node-end` | `node_id`, `node_type`, `output` | Después de ejecutar un nodo interno |

### Texto

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-text-start` | `id` | Primer token de un LLM interno |
| `subgraph-text-delta` | `id`, `delta` | Token de texto interno |
| `subgraph-text-end` | `id` | Fin del bloque de texto interno |

### Razonamiento

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-reasoning-start` | `id` | Apertura de bloque de razonamiento interno |
| `subgraph-reasoning-delta` | `id`, `delta` | Token de razonamiento interno |
| `subgraph-reasoning-end` | `id` | Cierre del bloque |

### Tool calling

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-tool-input-start` | `toolCallId`, `toolName` | Primer chunk de args de tool interno |
| `subgraph-tool-input-delta` | `toolCallId`, `inputTextDelta` | Chunk de args en streaming |
| `subgraph-tool-input-available` | `toolCallId`, `toolName`, `input` | Args completos del tool |
| `subgraph-tool-output-available` | `toolCallId`, `output` | Resultado del tool |

### Skill

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-skill-loaded` | `nodeId`, `toolCallId`, `skillName`, `reference?`, `source`, `sizeBytes` | Skill cargada dentro del subgrafo |

### Resumen y error

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-usage-summary` | `nodes` (array) | Resumen de tokens del subgrafo al finalizar |
| `subgraph-error` | `errorText` | Error dentro del subgrafo |

---

## Eventos por tipo de nodo

### `trigger_webhook` / `input`

Solo emiten ciclo de vida básico.

```
node-start  { node_id, node_type, config, inputs }
node-end    { node_id, node_type, output }
```

---

### `llm_call` (top-level, sin tools, sin thinking)

```
node-start        { node_id: "llm_1", node_type: "llm_call" }
  text-start      { id: "txt_abc" }
  text-delta      { id: "txt_abc", delta: "Hola" }
  text-delta      { id: "txt_abc", delta: " mundo" }
  text-end        { id: "txt_abc" }
node-end          { node_id: "llm_1", node_type: "llm_call", output: { result: "Hola mundo" } }
```

---

### `llm_call` con tool calling

```
node-start              { node_id: "llm_1", node_type: "llm_call" }
  tool-input-start      { toolCallId: "tc_1", toolName: "search" }
  tool-input-delta      { toolCallId: "tc_1", inputTextDelta: "{\"q\"" }
  tool-input-delta      { toolCallId: "tc_1", inputTextDelta: ":\"Rust\"}" }
  tool-input-available  { toolCallId: "tc_1", toolName: "search", input: { q: "Rust" } }
  tool-output-available { toolCallId: "tc_1", output: { results: [...] } }
  text-start            { id: "txt_abc" }
  text-delta            { id: "txt_abc", delta: "Encontré lo siguiente..." }
  text-end              { id: "txt_abc" }
node-end                { node_id: "llm_1", node_type: "llm_call", output: { result: "..." } }
```

---

### `llm_call` con `thinking_budget` (top-level)

El razonamiento precede al texto de respuesta:

```
node-start        { node_id: "llm_1", node_type: "llm_call" }
  reasoning-start { id: "rsn_xyz" }
  reasoning-delta { id: "rsn_xyz", delta: "El usuario pregunta..." }
  reasoning-delta { id: "rsn_xyz", delta: "Debo calcular primero..." }
  reasoning-end   { id: "rsn_xyz" }
  text-start      { id: "txt_abc" }
  text-delta      { id: "txt_abc", delta: "El resultado es 84." }
  text-end        { id: "txt_abc" }
node-end          { node_id: "llm_1", node_type: "llm_call", output: { result: "..." } }
```

---

### `llm_call` dentro de un `subgraph` (o agente-tarea del orchestrator)

Todos los eventos llevan el prefijo `subgraph-`:

```
subgraph-node-start              { node_id: "inner_llm", node_type: "llm_call" }
  subgraph-tool-input-start      { toolCallId: "tc_2", toolName: "getWeather" }
  subgraph-tool-input-delta      { toolCallId: "tc_2", inputTextDelta: "{\"city\"" }
  subgraph-tool-input-available  { toolCallId: "tc_2", toolName: "getWeather", input: { city: "Madrid" } }
  subgraph-tool-output-available { toolCallId: "tc_2", output: { temp: "22°C" } }
  subgraph-text-start            { id: "txt_def" }
  subgraph-text-delta            { id: "txt_def", delta: "En Madrid hace 22°C." }
  subgraph-text-end              { id: "txt_def" }
subgraph-node-end                { node_id: "inner_llm", node_type: "llm_call", output: { result: "..." } }
subgraph-usage-summary           { nodes: [...] }
```

---

### `orchestrator`

El orchestrator emite tres capas de eventos:

**1. Su propio ciclo de vida (top-level):**
```
node-start  { node_id: "orch_1", node_type: "orchestrator" }
...
node-end    { node_id: "orch_1", node_type: "orchestrator", output: { final_response: "..." } }
```

**2. Sus LLMs internos (`planner`, `critic`, `phase_reactor`, `final_reactor`) como subgrafos con thinking-delta:**

```
subgraph-node-start  { node_id: "planner", node_type: "planner", ... }
thinking-delta       { node_id: "planner", node_type: "planner", delta: "[{\"task\"..." }
thinking-delta       { node_id: "planner", node_type: "planner", delta: "\"assigned_to\"..." }
subgraph-node-end    { node_id: "planner", node_type: "planner", output: { result: { items: [...] } } }
```

> El `node_id` en `thinking-delta` **siempre coincide** con el del `subgraph-node-start` que lo enmarca. Esto permite al frontend correlacionarlos.

**3. Sus agentes-tarea como subgrafos completos:**

```
subgraph-node-start              { node_id: "experto_vuelos", node_type: "llm_call" }
  subgraph-tool-input-start      { toolCallId: "tc_3", toolName: "search_flights" }
  subgraph-tool-input-available  { ... }
  subgraph-tool-output-available { ... }
  subgraph-text-start            { id: "txt_ghi" }
  subgraph-text-delta            { id: "txt_ghi", delta: "Vuelo Madrid-BCN..." }
  subgraph-text-end              { id: "txt_ghi" }
subgraph-node-end                { node_id: "experto_vuelos", node_type: "llm_call", output: { result: "..." } }
subgraph-usage-summary           { nodes: [...] }
```

---

## Flujos completos de ejemplo

### Grafo simple: trigger → llm_call

```
node-start    { node_id: "trigger", node_type: "trigger_webhook" }
node-end      { node_id: "trigger", node_type: "trigger_webhook", output: { prompt: "Hola" } }

node-start    { node_id: "llm_1", node_type: "llm_call" }
  text-start  { id: "txt_abc" }
  text-delta  { id: "txt_abc", delta: "¡Hola! ¿En qué puedo ayudarte?" }
  text-end    { id: "txt_abc" }
node-end      { node_id: "llm_1", node_type: "llm_call", output: { result: "¡Hola!..." } }

usage-summary { nodes: [{ node_id: "llm_1", prompt_tokens: 10, ... }] }
finish        { finishReason: "stop", usage: {...}, output: { llm_1: { result: "..." } } }
```

---

### Orchestrator completo (planner → agentes → final_reactor)

```
node-start              { node_id: "orch_1", node_type: "orchestrator" }

  ── Planner ──────────────────────────────────────────────────
  subgraph-node-start   { node_id: "planner", node_type: "planner" }
  thinking-delta        { node_id: "planner", node_type: "planner", delta: "[{\"task\"..." }
  thinking-delta        { node_id: "planner", node_type: "planner", delta: "...}]" }
  subgraph-node-end     { node_id: "planner", node_type: "planner", output: { result: { items: [...] } } }

  ── Agente-tarea (en paralelo con otros agentes) ─────────────
  subgraph-node-start              { node_id: "experto_vuelos", node_type: "llm_call" }
    subgraph-tool-input-start      { toolCallId: "tc_1", toolName: "search_flights" }
    subgraph-tool-input-available  { toolCallId: "tc_1", input: { origin: "MAD", ... } }
    subgraph-tool-output-available { toolCallId: "tc_1", output: { flights: [...] } }
    subgraph-text-start            { id: "txt_001" }
    subgraph-text-delta            { id: "txt_001", delta: "Opción 1: Iberia..." }
    subgraph-text-end              { id: "txt_001" }
  subgraph-node-end                { node_id: "experto_vuelos", node_type: "llm_call", output: {...} }
  subgraph-usage-summary           { nodes: [...] }

  ── Reactor final ────────────────────────────────────────────
  subgraph-node-start   { node_id: "final_reactor", node_type: "llm_call" }
  thinking-delta        { node_id: "final_reactor", node_type: "llm_call", delta: "Plan de viaje..." }
  subgraph-node-end     { node_id: "final_reactor", node_type: "llm_call", output: { result: "..." } }

node-end        { node_id: "orch_1", node_type: "orchestrator", output: { final_response: "..." } }

usage-summary   { nodes: [...] }
finish          { finishReason: "stop", usage: {...}, output: {...} }
```

---

### Orchestrator con suspend (planner pide aclaración)

El planner no tiene información suficiente y emite preguntas al usuario:

```
node-start            { node_id: "orch_1", node_type: "orchestrator" }
  subgraph-node-start { node_id: "planner", node_type: "planner" }
  thinking-delta      { node_id: "planner", node_type: "planner", delta: "Necesito más info..." }
  subgraph-node-end   { node_id: "planner", node_type: "planner",
                        output: { "__colmena_status": "SUSPENDED",
                                  "questions": [
                                    { "id": "origin",      "question": "¿Desde dónde viajas?",   "type": "open" },
                                    { "id": "destination", "question": "¿A dónde quieres ir?",   "type": "open" },
                                    { "id": "dates",       "question": "¿Cuándo viajas?",        "type": "open" }
                                  ] } }
node-end              { node_id: "orch_1", node_type: "orchestrator",
                        output: { "__colmena_status": "SUSPENDED", "questions": [...] } }

finish                { finishReason: "suspended",
                        usage: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
                        output: { "__colmena_status": "SUSPENDED",
                                  "questions": [...],
                                  "session_id": "abc-123" } }
```

Para reanudar, el cliente envía las respuestas con el mismo `session_id`. El planner recibe las respuestas en su contexto y genera el plan completo sin volver a preguntar.

---

## Tabla resumen — todos los eventos

| Evento | Nivel | Campos obligatorios | Campos opcionales |
|--------|-------|--------------------|--------------------|
| `node-start` | top | `node_id`, `node_type`, `config`, `inputs` | — |
| `node-end` | top | `node_id`, `node_type`, `output` | — |
| `text-start` | top | `id` | — |
| `text-delta` | top | `id`, `delta` | — |
| `text-end` | top | `id` | — |
| `reasoning-start` | top | `id` | — |
| `reasoning-delta` | top | `id`, `delta` | — |
| `reasoning-end` | top | `id` | — |
| `tool-input-start` | top | `toolCallId`, `toolName` | — |
| `tool-input-delta` | top | `toolCallId`, `inputTextDelta` | — |
| `tool-input-available` | top | `toolCallId`, `toolName`, `input` | — |
| `tool-output-available` | top | `toolCallId`, `output` | — |
| `skill-loaded` | top | `nodeId`, `toolCallId`, `skillName`, `source`, `sizeBytes` | `reference` |
| `thinking-delta` | top | `node_id`, `node_type`, `delta` | — |
| `usage-summary` | top | `nodes` | — |
| `finish` | top | `finishReason`, `usage`, `output` | — |
| `error` | top | `errorText` | — |
| `subgraph-node-start` | sub | `node_id`, `node_type`, `config`, `inputs` | — |
| `subgraph-node-end` | sub | `node_id`, `node_type`, `output` | — |
| `subgraph-text-start` | sub | `id` | — |
| `subgraph-text-delta` | sub | `id`, `delta` | — |
| `subgraph-text-end` | sub | `id` | — |
| `subgraph-reasoning-start` | sub | `id` | — |
| `subgraph-reasoning-delta` | sub | `id`, `delta` | — |
| `subgraph-reasoning-end` | sub | `id` | — |
| `subgraph-tool-input-start` | sub | `toolCallId`, `toolName` | — |
| `subgraph-tool-input-delta` | sub | `toolCallId`, `inputTextDelta` | — |
| `subgraph-tool-input-available` | sub | `toolCallId`, `toolName`, `input` | — |
| `subgraph-tool-output-available` | sub | `toolCallId`, `output` | — |
| `subgraph-skill-loaded` | sub | `nodeId`, `toolCallId`, `skillName`, `source`, `sizeBytes` | `reference` |
| `subgraph-usage-summary` | sub | `nodes` | — |
| `subgraph-error` | sub | `errorText` | — |

---

## Reglas de correlación para el frontend

1. **Texto**: el `id` de `text-start` / `text-delta` / `text-end` es el identificador del bloque. Un nodo solo tiene un bloque de texto activo a la vez.

2. **Tools**: el `toolCallId` conecta `tool-input-start` → `tool-input-delta*` → `tool-input-available` → `tool-output-available`. Los `toolCallId` de top-level y subgrafo son independientes (pueden repetirse sin colisión).

3. **Thinking del orchestrator**: el `node_id` en `thinking-delta` siempre coincide con el `node_id` del `subgraph-node-start` / `subgraph-node-end` que lo envuelve. La tripleta siempre aparece en orden:
   ```
   subgraph-node-start  { node_id: "X" }
   thinking-delta       { node_id: "X", ... }  ← solo si streaming: true
   subgraph-node-end    { node_id: "X" }
   ```

4. **Suspended**: cuando `finish.finishReason === "suspended"`, el frontend debe mostrar las preguntas de `output.questions` y conservar `output.session_id` para reanudar.

5. **Razonamiento vs thinking**: son canales distintos:
   - `reasoning-*` → razonamiento interno del modelo (extended thinking del LLM, e.g. Gemini con `thinking_budget`)
   - `thinking-delta` → tokens de los LLMs internos del orchestrator (planner, critic, etc.)
