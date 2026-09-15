# `node-end`/`subgraph-node-end` ganan `status`/`errorText`; la frontera de una tool `llm_call`/`for_each` es el primer emisor

**Acción de ADP: recomendada, no obligatoria.** Aditivo — un cierre exitoso
sigue sin la clave `status` en absoluto (no `null`, ausente).

## Qué cambia

`NodeFinish`/`SubgraphNodeFinish` (`events.rs`) ganaron `error:
Option<NodeEndError>` (aditivo, `skip_serializing_if`, compatible con frames
viejos). `SseMapper` lo traduce a `"status":"error"` + `"errorText"` (si
`error.message` es `Some`) en `node-end`/`subgraph-node-end`, top-level y
wrapped.

**El primer emisor real**: el cierre de la frontera de una tool
`llm_call`/`for_each` (`DagToolExecutor::execute_inner`) ya se emitía antes en
éxito y en error — eso no es nuevo. Lo que cambia: el cierre por falla ahora
marca `status:"error"` y, como se emite **después** de `mask_outbound`, trae
`errorText` con cualquier secure value decodificado ya enmascarado:

```jsonc
{ "type": "subgraph-node-end", "node_id": "Helper", "node_type": "llm_call",
  "output": null, "status": "error",
  "errorText": "Request failed: ... models/gemini-does-not-exist-9000 ...",
  "level": 1, "path": "agent>Helper" }
```

Antes, lo único que decía que había fallado era el `ToolResult` (`success:
false`) que recibe el modelo — `event-tree-builder.ts`/`colmena-events.reducer.ts`
cerraban ese `node_id` como `'done'` siempre, así que el árbol de UI lo
mostraba como terminado bien.

**Todavía abierto** (próximos PRs de esta serie): la frontera propia de un
`subgraph`-as-tool y cualquier nodo interno de un run anidado no reportan su
falla — siguen sin cerrar en absoluto. `status:"error"` sin `errorText` es
válido: solo dice que ese sitio de cierre no tenía texto enmascarado para el
wire.

## Qué tiene que hacer ADP

Ambos `closeNode` ya aceptan `status`/`errorText` — hoy siempre se llaman con
`'done'`. Cambio de una línea en cada sitio:

```ts
// apps/api/src/chat/application/event-tree-builder.ts
// packages/shared/src/hooks/colmena-events.reducer.ts
closeNode(ev.node_id, ev.output, ev.status === "error" ? "error" : "done", ev.errorText);
```

## Qué gana ADP, y qué se rompe si lo ignora

Gana: una tool `llm_call`/`for_each` que falla deja de pintarse como `'done'`
en el árbol de UI. Se rompe si se ignora: nada — el comportamiento actual
(siempre `'done'`) continúa sin cambios.
