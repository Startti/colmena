# SSE Events Reference

## Eventos de ciclo de vida — todos los nodos

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `node-start` | `node_id`, `node_type`, `config`, `inputs` | Antes de ejecutar cualquier nodo |
| `node-end` | `node_id`, `node_type`, `output` | Después de ejecutar cualquier nodo |

---

## Eventos de texto — nodos con LLM (`llm`, `planner`, `critic`, `reactor`, `extraction`)

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `text-start` | `id` (uuid) | Al llegar el primer token de un nodo |
| `text-delta` | `node_id`, `delta` | Por cada token de streaming |
| `text-end` | `id` (mismo uuid) | Al terminar el nodo (NodeFinish) |

---

## Eventos de herramientas — nodo `llm` con tool calling

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `tool-input-start` | `toolCallId`, `toolName` | Al detectar la primera vez un tool call |
| `tool-input-delta` | `toolCallId`, `inputTextDelta` | Por cada chunk de argumentos del tool |
| `tool-input-available` | `toolCallId`, `toolName`, `input` | Cuando los args del tool están completos |
| `tool-output-available` | `toolCallId`, `output` | Cuando el tool termina de ejecutarse |

---

## Eventos de subgrafo — nodos internos de `subgraph` / `orchestrator`

Todos los eventos que ocurren dentro de un subgrafo se emiten con el prefijo `subgraph-`.

| Evento | Campos | Cuándo |
|--------|--------|--------|
| `subgraph-node-start` | `node_id`, `node_type`, `config`, `inputs` | Antes de ejecutar un nodo interno |
| `subgraph-node-end` | `node_id`, `node_type`, `output` | Después de ejecutar un nodo interno |
| `subgraph-text-start` | `id` (uuid) | Primer token de un nodo LLM interno |
| `subgraph-text-delta` | `node_id`, `delta` | Por cada token de streaming interno |
| `subgraph-text-end` | `id` (mismo uuid) | Al terminar el nodo LLM interno |
| `subgraph-tool-input-start` | `toolCallId`, `toolName` | Primera vez que un tool call interno aparece |
| `subgraph-tool-input-delta` | `toolCallId`, `inputTextDelta` | Chunk de argumentos de tool interno |
| `subgraph-tool-input-available` | `toolCallId`, `toolName`, `input` | Args del tool interno completos |
| `subgraph-tool-output-available` | `toolCallId`, `output` | Tool interno terminó de ejecutarse |
| `subgraph-thinking-delta` | `node_id`, `delta` | Token de razonamiento interno |

---

## Eventos especiales

| Evento | Campos | Quién lo emite | Cuándo |
|--------|--------|----------------|--------|
| `thinking-delta` | `node_id`, `delta` | `orchestrator` | Tokens de LLM internas del orchestrator (planner/critic/reactor se convierten a ThinkingToken) |
| `usage-summary` | `nodes` | Engine | Justo antes del `finish` |
| `finish` | `finishReason`, `usage.promptTokens`, `usage.completionTokens` | Engine | Al terminar todo el grafo |
| `error` | `errorText` | Engine | Si hay un error en ejecución |

---

## Flujo completo de ejemplo

```
node-start              (trigger)
node-end                (trigger)

node-start              (llm con tools)
  text-start
  text-delta × N
  tool-input-start
  tool-input-delta × N
  tool-input-available
  tool-output-available
  text-end
node-end                (llm)

node-start              (subgraph / orchestrator)
  subgraph-node-start   (nodo interno)
    subgraph-text-start
    subgraph-text-delta × N
    subgraph-tool-input-start
    subgraph-tool-input-delta × N
    subgraph-tool-input-available
    subgraph-tool-output-available
    subgraph-text-end
  subgraph-node-end     (nodo interno)
node-end                (subgraph / orchestrator)

usage-summary
finish
```
