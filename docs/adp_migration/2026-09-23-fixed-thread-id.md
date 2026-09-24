# `thread_id` fijo: memoria por agente sin exponer el hilo al modelo

**Acción de ADP: compilar "Run My Agent" con `memory_mode: "dynamic"` y `thread_id: {
"fixed": "${agentId}" }`** en su `node_schema`, junto con `child_graph_ref: { "fixed":
{ "agent_id": "${agentId}", … } }`. Así, cada agente del usuario gana su propio hilo de
memoria (`tool/<tool>/<agentId>/…`) sin que el modelo del padre vea `thread_id`.
Aditivo — nada se rompe en grafos existentes; sin esta acción, `dynamic` sigue como
hoy: el modelo nombra el hilo él mismo (entrada 66 del changelog, «Sub-agent tool
memory»).

## Qué cambia

Task 4 de la cadena `child_graph_ref` (PR 4/5). Antes, `node_schema.thread_id` siempre
era auto-expuesto al modelo como parámetro **requerido** — el motor ignoraba un `fixed`
declarado ahí. Ahora `thread_id` acepta `{ "fixed": "<template>" }` como cualquier otro
campo `node_schema`, templado contra los parámetros top-level del mismo `node_schema`
(p. ej. `agentId`). Fijo:

- El motor **no** auto-expone `thread_id` — no hay parámetro que elegir.
- El resultado **no** lleva el prefijo `[hilo: <id>]`.
- El tool queda **afuera** de la tool sintética `list_threads`.
- La memoria sigue keyando por el valor resuelto (`tool/<tool_name>/<valor-resuelto>`)
  — un hilo distinto por cada `agentId`.
- Si la plantilla no resuelve (parámetro ausente o vacío), la llamada devuelve
  `ToolResult { success: false, error: Some("unresolved_thread_id") }`, nunca un hilo
  compartido por todos.

### El shape que ADP compila

```json
"tool_configurations": {
  "<uuid-o-nombre>": {
    "name": "Run_My_Agent",
    "node_type": "subgraph",
    "memory_mode": "dynamic",
    "node_schema": {
      "agentId": { "type": "string", "required": true, "description": "id del agente a correr" },
      "prompt":  { "type": "string", "required": true, "description": "instrucción para el agente" },
      "child_graph_ref": { "fixed": { "agent_id": "${agentId}" } },
      "thread_id": { "fixed": "${agentId}" }
    }
  }
}
```

`agentId` **tiene que** estar declarado LLM-visible en el mismo `node_schema` — el
templado de `fixed` solo resuelve contra parámetros que la llamada trajo
(`node_schema_merge.rs`). Si falta, falla con `unresolved_thread_id`, nunca comparte
hilo entre agentes.
