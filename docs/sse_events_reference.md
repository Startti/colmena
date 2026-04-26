# SSE Events Reference

Esta referencia documenta todos los eventos que el motor DAG emite sobre el stream SSE (o stdout en modo `colmena run`). Cada línea del stream es un objeto JSON con al menos el campo `"type"`.

---

## Eventos de ciclo de vida — todos los nodos

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `node-start` | `node_id`, `node_type`, `config`, `inputs` | Antes de ejecutar cualquier nodo |
| `node-end` | `node_id`, `node_type`, `output` | Después de que un nodo completa su ejecución |

> `inputs` tiene filtradas las claves que empiezan por `__` y `session_id`.

---

## Eventos de texto — nodos LLM con streaming

Aplica a: `llm`, `extraction` y cualquier nodo que emita `LlmToken`.

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `text-start` | `id` (uuid) | Primer token de un nodo LLM |
| `text-delta` | `id` (mismo uuid), `delta` | Por cada token de texto generado |
| `text-end` | `id` (mismo uuid) | Al finalizar el nodo (NodeFinish) |

---

## Eventos de razonamiento — modelos con extended thinking

Aplica cuando el modelo emite bloques de razonamiento (`ReasoningStart/Delta/End`).

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `reasoning-start` | `id` (uuid del bloque) | Apertura de un bloque de razonamiento |
| `reasoning-delta` | `id`, `delta` | Token de razonamiento |
| `reasoning-end` | `id` | Cierre del bloque de razonamiento |

---

## Eventos de herramientas — nodo `llm` con tool calling

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `tool-input-start` | `toolCallId`, `toolName` | Primer chunk de argumentos del tool (una vez por tool_id) |
| `tool-input-delta` | `toolCallId`, `inputTextDelta` | Chunk de argumentos del tool (streaming) |
| `tool-input-available` | `toolCallId`, `toolName`, `input` | Argumentos del tool completos |
| `tool-output-available` | `toolCallId`, `output` | El tool terminó de ejecutarse |

Secuencia completa para un tool call:

```
tool-input-start      { toolCallId: "call_abc", toolName: "getWeather" }
tool-input-delta      { toolCallId: "call_abc", inputTextDelta: "{\"city\"" }
tool-input-delta      { toolCallId: "call_abc", inputTextDelta: ":\"SF\"}" }
tool-input-available  { toolCallId: "call_abc", toolName: "getWeather", input: {"city":"SF"} }
tool-output-available { toolCallId: "call_abc", output: {"weather":"sunny"} }
```

---

## Evento de skill — `load_skill` tool

Emitido junto con `tool-input-available` / `tool-output-available` cuando el LLM llama al tool sintético `load_skill`.

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `skill-loaded` | `nodeId`, `toolCallId`, `skillName`, `reference?`, `source`, `sizeBytes` | Cuando una skill se carga exitosamente |

---

## Eventos de thinking — LLMs internos del `orchestrator`

Estos eventos corresponden a los LLMs **internos** del nodo `orchestrator`: `planner`, `phase_reactor`, `critic` y `final_reactor`. No pertenecen a los agentes-tarea sino al proceso de "pensamiento" del orchestrator.

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `thinking-delta` | `node_id`, `delta` | Token de un LLM interno del orchestrator |

> El `node_id` aquí es el del nodo `orchestrator`, no el del LLM interno.

---

## Eventos de subgrafo — nodos dentro de `subgraph` y agentes-tarea del `orchestrator`

Todos los eventos que ocurren dentro de un subgrafo se emiten con el prefijo `subgraph-`. Aplica a:
- Nodos dentro de un nodo `subgraph`
- Agentes-tarea que el `orchestrator` ejecuta para completar sus fases

### Ciclo de vida

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-node-start` | `node_id`, `node_type`, `config`, `inputs` | Antes de ejecutar un nodo interno |
| `subgraph-node-end` | `node_id`, `node_type`, `output` | Después de ejecutar un nodo interno |

### Texto

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-text-start` | `id` (uuid) | Primer token de un LLM interno |
| `subgraph-text-delta` | `id`, `delta` | Por cada token de texto interno |
| `subgraph-text-end` | `id` | Al finalizar el nodo LLM interno |

### Razonamiento

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-reasoning-start` | `id` | Apertura de bloque de razonamiento interno |
| `subgraph-reasoning-delta` | `id`, `delta` | Token de razonamiento interno |
| `subgraph-reasoning-end` | `id` | Cierre del bloque de razonamiento interno |

### Herramientas

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-tool-input-start` | `toolCallId`, `toolName` | Primer chunk de argumentos de tool interno (una vez por tool_id) |
| `subgraph-tool-input-delta` | `toolCallId`, `inputTextDelta` | Chunk de argumentos de tool interno |
| `subgraph-tool-input-available` | `toolCallId`, `toolName`, `input` | Argumentos del tool interno completos |
| `subgraph-tool-output-available` | `toolCallId`, `output` | Tool interno terminó |

### Skill

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-skill-loaded` | `nodeId`, `toolCallId`, `skillName`, `reference?`, `source`, `sizeBytes` | Skill cargada dentro del subgrafo |

### Resumen de uso

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-usage-summary` | `nodes` (array) | Resumen de tokens del subgrafo |

### Error

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-error` | `errorText` | Error dentro del subgrafo |

---

## Eventos de cierre del grafo

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `usage-summary` | `nodes` (array) | Justo antes del `finish`, resumen de tokens por nodo |
| `finish` | `finishReason`, `usage`, `output` | Fin de la ejecución del grafo |
| `error` | `errorText` | Error irrecuperable en el grafo |

### Estructura de `usage-summary.nodes`

Cada entrada del array `nodes`:

```json
{
  "node_id": "llm_agent_1",
  "node_type": "llm",
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

> `thinking_tokens`, `cache_read_tokens` y `cache_write_tokens` son opcionales (solo presentes si > 0).

### Estructura de `finish.usage`

```json
{
  "promptTokens": 2400,
  "completionTokens": 680,
  "totalTokens": 3330,
  "thinkingTokens": 250,
  "cacheReadTokens": 1600,
  "cacheWriteTokens": 800
}
```

> `thinkingTokens`, `cacheReadTokens` y `cacheWriteTokens` son opcionales.

### `finish.finishReason`

| Valor | Cuándo |
|-------|--------|
| `"stop"` | Ejecución completada normalmente |
| `"suspended"` | Ejecución pausada esperando respuesta del usuario (`__colmena_status: SUSPENDED`) |

---

## Notas sobre el orchestrator

El nodo `orchestrator` tiene dos tipos de actividad interna con eventos distintos:

| Actividad | Evento emitido |
|-----------|---------------|
| LLMs de planeación/revisión (planner, phase_reactor, critic, final_reactor) | `thinking-delta` |
| Agentes-tarea que ejecuta como subgrafos | `subgraph-node-start/end`, `subgraph-text-*`, etc. |

Esto permite al frontend distinguir el "pensamiento" del orchestrator de las respuestas reales de los agentes-tarea.

---

## Flujo completo de ejemplo

### Grafo simple con un nodo LLM con tools

```
node-start              { node_id: "trigger" }
node-end                { node_id: "trigger" }

node-start              { node_id: "llm_agent", node_type: "llm" }
  text-start            { id: "txt_abc123" }
  text-delta            { id: "txt_abc123", delta: "Voy a " }
  text-delta            { id: "txt_abc123", delta: "buscar..." }
  tool-input-start      { toolCallId: "tc_1", toolName: "search" }
  tool-input-delta      { toolCallId: "tc_1", inputTextDelta: '{"q"' }
  tool-input-available  { toolCallId: "tc_1", toolName: "search", input: { q: "..." } }
  tool-output-available { toolCallId: "tc_1", output: { results: [...] } }
  text-end              { id: "txt_abc123" }
node-end                { node_id: "llm_agent" }

usage-summary           { nodes: [...] }
finish                  { finishReason: "stop", usage: {...}, output: {...} }
```

### Orchestrator con agente-tarea

```
node-start              { node_id: "orchestrator" }

  thinking-delta        { node_id: "orchestrator", delta: "Analizando..." }   ← planner interno

  subgraph-node-start   { node_id: "researcher_agent" }
    subgraph-text-start { id: "txt_xyz" }
    subgraph-text-delta { id: "txt_xyz", delta: "Investigando..." }
    subgraph-tool-input-available { toolCallId: "tc_2", toolName: "search", ... }
    subgraph-tool-output-available { toolCallId: "tc_2", output: {...} }
    subgraph-text-end   { id: "txt_xyz" }
  subgraph-node-end     { node_id: "researcher_agent" }

  thinking-delta        { node_id: "orchestrator", delta: "Revisando..." }    ← critic/reactor interno

node-end                { node_id: "orchestrator" }

usage-summary           { nodes: [...] }
finish                  { finishReason: "stop", usage: {...}, output: {...} }
```

### Ejecución con suspend

```
node-start              { node_id: "orchestrator" }
  thinking-delta        { ... }  ← planner pidió aclaración
node-end                { node_id: "orchestrator" }

finish                  { finishReason: "suspended", usage: {...}, output: { question: "..." } }
```
