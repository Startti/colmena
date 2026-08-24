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

> El prefijo distingue **anidado vs. top-level**, no la profundidad. Un evento de nivel 1 y uno de nivel 4 son ambos `subgraph-*`; lo único que los separa es el campo `level`. Y la anidación **no tiene tope** desde 2026-08-21 (ver [nota de migración](adp_migration/2026-08-21-unbounded-subgraph-nesting.md)), así que `level` puede crecer sin cota: un consumidor que indente por nivel necesita un tope visual propio.

### Campos `level` / `path` en todos los eventos

`SseMapper::map()` etiqueta **todas** las líneas emitidas (top-level y de subgrafo) con dos campos adicionales antes de devolverlas: `level` (profundidad de anidamiento, `u32`) y `path` (linaje como string, p.ej. `"parent>child"`). Son aditivos — no aparecen en las tablas de campos de este documento por brevedad, pero están presentes en absolutamente todos los frames. Ver `src/libs/colmena/src/dag_engine/sse_mapper.rs:629-639`.

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

### Nodo no ejecutado

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `node-skipped` | `node_id`, `reason` | El motor decidió no ejecutar el nodo |

Un nodo que no corre ya no es invisible. Antes, las tres rutas que descartan un
nodo lo hacían sin emitir nada: un checkpoint `suspend` mal cableado se veía
exactamente igual que un grafo que corrió limpio.

| `reason` | Significado | ¿Es un problema? |
|----------|-------------|------------------|
| `upstream_never_fired` | El nodo nunca tuvo sus dependencias resueltas y ningún upstream quedaba en cola | Casi siempre sí — grafo mal cableado |
| `upstream_null_output` | El nodo upstream emitió `null` (*skip stub*), por lo que la rama se detiene | No — control de flujo deliberado |
| `pointer_unresolved` | La arista entrante usa `from: "nodo.campo"` y ese campo no existe en el output | No — así funciona el ruteo condicional |
| `unknown_target` | La arista apunta a un `node_id` ausente de `nodes` | Sí — error de cableado |
| `never_reached` | El nodo no produjo output y ninguna arista pasó por él, así que no se observó una causa más precisa | Depende — típico en nodos detrás de otro ya salteado |
| `run_stopped_early` | La corrida abandonó su cola al alcanzar un límite de ejecución (`max_total_calls` / `max_calls_from`) | Sí — la corrida quedó truncada |

Es un evento **informativo**: no aborta la corrida.

**Cuándo se emite.** Al **final** de la corrida, comparando el grafo completo
contra los nodos que produjeron output. Sale **un evento por nodo**, nunca uno
por arista.

Se decide así, y no en el momento en que una arista pasa de largo, por dos
motivos: un nodo con varias aristas entrantes puede ser salteado por una y
ejecutarse igual por otra (reportarlo ahí sería mentira), y un nodo que está
detrás de otro ya salteado nunca llega a ser marcado por ninguna arista
(reportarlo solo desde las marcas lo dejaría invisible). El `reason` que queda es
la **primera** causa observada, o `never_reached` si no se observó ninguna.

En una corrida que termina suspendida (`finishReason: "suspended"`) o cancelada
no se emite ninguno: esos nodos están pendientes, no salteados.

```json
{ "type": "node-skipped", "node_id": "ask_user", "reason": "pointer_unresolved" }
```

Dentro de un subgrafo llega igual que los demás eventos de nodo, con sus campos
`level` y `path`.

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
| `tool-described` | `nodeId`, `toolCallId`, `toolName` | Emitido cuando una invocación de `describe_tool` resuelve y el motor revela el schema completo de una tool perezosa. Permite al frontend mostrar "Schema de `<toolName>` listo" sin esperar al `tool-output-available`. Detalles en [29_lazy_tool_loading.md](./developer_guide/29_lazy_tool_loading.md). |

Secuencia completa:

```json
{ "type": "tool-input-start",     "toolCallId": "call_abc", "toolName": "search" }
{ "type": "tool-input-delta",     "toolCallId": "call_abc", "inputTextDelta": "{\"q\"" }
{ "type": "tool-input-delta",     "toolCallId": "call_abc", "inputTextDelta": ":\"Rust\"}" }
{ "type": "tool-input-available", "toolCallId": "call_abc", "toolName": "search", "input": { "q": "Rust" } }
{ "type": "tool-output-available","toolCallId": "call_abc", "output": { "results": [...] } }
```

Secuencia con lazy loading (`tool-described` antes del schema):

```json
{ "type": "tool-input-available", "toolCallId": "call_xyz", "toolName": "describe_tool", "input": { "name": "search_orders" } }
{ "type": "tool-described",       "nodeId": "agent", "toolCallId": "call_xyz", "toolName": "search_orders" }
{ "type": "tool-output-available","toolCallId": "call_xyz", "output": { "name": "search_orders", "schema": { ... } } }
```

---

### Skill — herramienta `load_skill`

Emitido cuando el LLM invoca la herramienta sintética `load_skill`.

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `skill-loaded` | `nodeId`, `toolCallId`, `skillName`, `reference?`, `source`, `sizeBytes` | Skill cargada exitosamente |

---

### Status — heartbeat de progreso

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `status` | `stage` (siempre `"running"`), `node_id`, `idleSecs` | Emitido periódicamente mientras un nodo lleva tiempo sin producir otros eventos (evento `Progress` interno) |

```json
{ "type": "status", "stage": "running", "node_id": "llm_1", "idleSecs": 12 }
```

> Se emite tanto a nivel top como dentro de subgrafos (en ese caso también lleva `"type": "status"`, sin prefijo `subgraph-`; se distingue por `level`/`path`). Ver `sse_mapper.rs:406-411` y `sse_mapper.rs:565-570`.

---

### Agent-turn — límites de turno del LLM

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `agent-turn` | `phase` (`"start"` o `"finish"`), `node_id` | Emitido al iniciar/terminar un turno de mensaje del LLM (`LlmMessageStart`/`LlmMessageFinish`), tanto top-level como dentro de subgrafos |

```json
{ "type": "agent-turn", "phase": "start",  "node_id": "llm_1" }
{ "type": "agent-turn", "phase": "finish", "node_id": "llm_1" }
```

> A diferencia de `finish`/`error`, `agent-turn` **no termina el stream** — solo marca límites de turno para mantener vivo el watchdog de "sin eventos" del cliente. Ver `sse_mapper.rs:334-343` (top-level) y `sse_mapper.rs:611-620` (subgrafo).

---

### Thinking — LLMs internos del nodo `orchestrator`

Los sub-componentes internos del `orchestrator` (`planner`, `phase_reactor`, `critic`) emiten sus tokens como `thinking-delta`. **No** son eventos de subgrafo.

> El `final_reactor` es la excepción: como produce la respuesta final dirigida al usuario, sus tokens se emiten como `text-delta` (top-level) o `subgraph-text-delta` (cuando el orchestrator está anidado dentro de un `subgraph`), no como `thinking-delta`. Ver sección [Final reactor — respuesta al usuario](#final-reactor--respuesta-al-usuario) más abajo.

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `thinking-delta` | `node_id`, `node_type`, `delta` | Token de un LLM interno del orchestrator |

El `node_id` identifica cuál sub-componente está pensando y **coincide exactamente** con el `node_id` del `subgraph-node-start`/`subgraph-node-end` que lo enmarca. Desde 2026-08-21 también coinciden `level` y `path`: antes el `thinking-delta` salía un nivel más arriba y bajo otro linaje que su propio `node-start`, así que un árbol armado agrupando por `path` no podía colgarlo bajo su nodo. Ver [nota de migración](adp_migration/2026-08-21-thinking-delta-level-fix.md).

> Con el `orchestrator` **anidado dentro de un `subgraph`** el thinking sigue siendo `thinking-delta`. Hasta 2026-08-21 en ese caso se emitía como `subgraph-text-delta`, o sea que el razonamiento interno del planner se renderizaba como la respuesta del agente. No existe un tipo `subgraph-thinking-delta`: los frames de thinking no son eventos de subgrafo en ningún nivel, y `level`/`path` bastan para ubicarlos.


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
| `"critic_<agente>"` | `"critic"` | Crítico para un agente específico |

> `thinking-delta` **solo** se emite si el sub-componente tiene `"streaming": true` en su config. Sin streaming, el token llega completo en el `output` del `subgraph-node-end`.

---

### Final reactor — respuesta al usuario

A diferencia de los demás sub-componentes del `orchestrator`, el `final_reactor` **no** emite `subgraph-node-start` / `subgraph-node-end` ni `thinking-delta`. Sus tokens fluyen como el stream de texto del propio orchestrator:

| Contexto del orchestrator | Eventos emitidos por el final_reactor |
|--------------------------|---------------------------------------|
| Top-level (orchestrator en el grafo principal) | `text-start` → `text-delta*` → `text-end` |
| Dentro de un nodo `subgraph` | `subgraph-text-start` → `subgraph-text-delta*` → `subgraph-text-end` |

El `id` del bloque de texto es el del **orchestrator** (no `"final_reactor"`), y el `text-end` / `subgraph-text-end` se cierra con el `node-end` / `subgraph-node-end` del propio orchestrator.

Top-level:
```json
{ "type": "node-start", "node_id": "orch_1", "node_type": "orchestrator" }
…
{ "type": "text-start", "id": "txt_xxx" }
{ "type": "text-delta", "id": "txt_xxx", "delta": "Plan de viaje" }
{ "type": "text-delta", "id": "txt_xxx", "delta": " listo." }
{ "type": "text-end",   "id": "txt_xxx" }
{ "type": "node-end",   "node_id": "orch_1", "output": { "final_response": "Plan de viaje listo.", "all_tasks": [...] } }
```

Anidado dentro de un `subgraph`:
```json
{ "type": "subgraph-node-start", "node_id": "orch_1", "node_type": "orchestrator" }
…
{ "type": "subgraph-text-start", "id": "txt_xxx" }
{ "type": "subgraph-text-delta", "id": "txt_xxx", "delta": "Plan de viaje" }
{ "type": "subgraph-text-end",   "id": "txt_xxx" }
{ "type": "subgraph-node-end",   "node_id": "orch_1", "output": { "final_response": "Plan de viaje listo.", ... } }
```

El texto completo también queda disponible en `output.final_response` del `node-end`/`subgraph-node-end` del orchestrator.

---

### Cierre del grafo

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `usage-summary` | `nodes` (array) | Justo antes de `finish` |
| `finish` | `finishReason`, `usage`, `output` | Fin de la ejecución |
| `error` | `errorText` | Error irrecuperable |
| `cancelled` | `reason`, `output` | Emitido justo antes del `finish` con `finishReason: "cancelled"`, cuando la ejecución se cancela (p.ej. el usuario detiene el run). Cierra cualquier bloque de texto abierto antes de emitirse. |

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
  "total_tokens": 2890
}
```

> `model` y `provider` identifican **el nodo de esa fila**, no el agente que lo
> despachó. Un `llm_call` anidado como tool puede declarar en su `fixed_config`
> un provider y un modelo distintos a los del padre, y esta fila reporta los
> suyos. Hasta el 2026-08-23 llegaban en `null` para todo nodo anidado, lo que
> permitía atribuir sus tokens pero no tarifarlos.
>
> `prompt_tokens` es el input **fresco** — los tokens servidos desde cache nunca
> están adentro, en ningún provider (los adapters normalizan la discrepancia
> entre las tres APIs). `total_tokens` suma las cinco columnas, cache incluido.
>
> `cache_read_tokens` y `cache_write_tokens` están **siempre presentes**, incluso
> en `0`. `thinking_tokens` solo aparece si es > 0.
>
> Ver [§14 — Provider prompt caching](developer_guide/14_llm_deep_dive.md) para
> la fórmula de costo y la tabla de semántica por provider.

#### `finish`

```json
{
  "type": "finish",
  "finishReason": "stop",
  "usage": {
    "promptTokens": 2400,
    "completionTokens": 680,
    "cacheReadTokens": 1600,
    "cacheWriteTokens": 800,
    "totalTokens": 5730,
    "thinkingTokens": 250
  },
  "output": { ... }
}
```

`finishReason`:

| Valor | Cuándo |
|-------|--------|
| `"stop"` | Ejecución completada normalmente |
| `"suspended"` | El grafo se pausó esperando respuesta del usuario |
| `"cancelled"` | La ejecución fue cancelada (ver evento `cancelled` arriba); precedido por un frame `cancelled` con `reason`/`output` |

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

Además de los nodos internos, el propio `subgraph` emite un **par de frontera** con `node_type: "subgraph"` que delimita todo su sub-árbol. El `node_id` de esa frontera sale de, en orden: el nombre del agente (`orchestrator`), el id del nodo del grafo (ruta por aristas), o el nombre del tool que el modelo llamó (ruta tool).

> Desde 2026-08-21 la ruta tool **también** emite frontera. Antes no emitía ninguna: el fallback estaba escrito pero nada poblaba la clave de la que dependía, así que un `subgraph` usado como tool streameaba sin delimitador. Ver [nota de migración](adp_migration/2026-08-21-subgraph-tool-boundary-frames.md).
>
> Y en las dos rutas de frontera sintética (agente de orchestrator, o tool) el contenido del hijo ahora **anida bajo** el nombre de la frontera en `path`, en vez de salir como su hermano. Eso sube un nivel el `level` de ese contenido. La ruta por aristas queda igual. Ver [nota de migración](adp_migration/2026-08-21-nested-level-and-path-changes.md).

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
| `subgraph-tool-described` | `nodeId`, `toolCallId`, `toolName` | Contraparte de subgrafo de `tool-described` — emitido cuando `describe_tool` resuelve dentro de un `subgraph` o agente-tarea del orchestrator. |

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

### `for_each` — progreso de fan-out

Un `for_each` emite dos eventos propios además de los de sus filas. Con el
prefijo `subgraph-` cuando el `for_each` está anidado.

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `batch-progress` | `nodeId`, `total`, `completed`, `ok`, `err`, `inFlight` | Al empezar, por cada fila terminada, y al final |
| `batch-item-finished` | `nodeId`, `index`, `key`, `status` | En cuanto una fila termina (`status`: `"ok"` \| `"err"`) |

Las filas corren el mismo grafo target, así que emiten los mismos `node_id`.
Desde 2026-08-21 cada fila corre bajo `<nodeId>#<índice>` en el `path`, y ese
índice **coincide con el campo `index`** de su `batch-item-finished`:

```
{ "type": "batch-item-finished", "nodeId": "for_each", "index": 0, "status": "ok" }
{ "type": "subgraph-text-delta",  "path": "coordinador>abanico>for_each#0>eco", ... }
{ "type": "subgraph-text-delta",  "path": "coordinador>abanico>for_each#1>eco", ... }
```

Antes las filas compartían un `path` idéntico: no se podían atribuir, y sus
bloques de texto colisionaban con `concurrency > 1`. Ver la
[nota de migración](adp_migration/2026-08-21-for-each-row-lineage.md).

---

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

**2. Sus LLMs internos `planner`, `critic`, `phase_reactor` como subgrafos con thinking-delta:**

```
subgraph-node-start  { node_id: "planner", node_type: "planner", ... }
thinking-delta       { node_id: "planner", node_type: "planner", delta: "[{\"task\"..." }
thinking-delta       { node_id: "planner", node_type: "planner", delta: "\"assigned_to\"..." }
subgraph-node-end    { node_id: "planner", node_type: "planner", output: { result: { items: [...] } } }
```

> El `node_id` en `thinking-delta` **siempre coincide** con el del `subgraph-node-start` que lo enmarca. Esto permite al frontend correlacionarlos.

> **El `final_reactor` no aparece como subgrafo ni emite `thinking-delta`** — sus tokens son la respuesta final al usuario y se emiten como `text-delta` (top-level) o `subgraph-text-delta` (orchestrator anidado). Ver sección [Final reactor — respuesta al usuario](#final-reactor--respuesta-al-usuario).

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
  text-start            { id: "txt_final" }
  text-delta            { id: "txt_final", delta: "Plan de viaje" }
  text-delta            { id: "txt_final", delta: " listo." }
  text-end              { id: "txt_final" }

node-end        { node_id: "orch_1", node_type: "orchestrator", output: { final_response: "Plan de viaje listo.", ... } }

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
| `batch-progress` | top | `nodeId`, `total`, `completed`, `ok`, `err`, `inFlight` | — |
| `batch-item-finished` | top | `nodeId`, `index`, `key`, `status` | — |
| `tool-output-available` | top | `toolCallId`, `output` | — |
| `skill-loaded` | top | `nodeId`, `toolCallId`, `skillName`, `source`, `sizeBytes` | `reference` |
| `tool-described` | top | `nodeId`, `toolCallId`, `toolName` | — |
| `status` | top/sub | `stage`, `node_id`, `idleSecs` | — |
| `agent-turn` | top/sub | `phase`, `node_id` | — |
| `thinking-delta` | top | `node_id`, `node_type`, `delta` | — |
| `usage-summary` | top | `nodes` | — |
| `finish` | top | `finishReason`, `usage`, `output` | — |
| `cancelled` | top | `reason`, `output` | — |
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
| `subgraph-tool-described` | sub | `nodeId`, `toolCallId`, `toolName` | — |
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
