# `node-end`/`subgraph-node-end` gain an additive `status`/`errorText` wire contract

**Acción de ADP: ninguna todavía.** Este PR solo agrega el campo y su mapeo —
**nada en el motor lo llena todavía**, así que ningún frame real trae
`status`/`errorText` hoy. Es el primero de una serie de PRs; el siguiente hace
que el cierre de una tool `llm_call`/`for_each` sea el primer emisor real.

## Qué cambia

`NodeFinish`/`SubgraphNodeFinish` (`events.rs`) ganan `error:
Option<NodeEndError>` (aditivo, `skip_serializing_if`, compatible con frames
viejos). `SseMapper` lo traduce a `"status":"error"` + `"errorText"` (si
`error.message` es `Some`) en `node-end`/`subgraph-node-end`, top-level y
wrapped. Un cierre exitoso sigue sin `status` en absoluto — byte-idéntico a
antes de este campo:

```jsonc
// Cuando (en un PR futuro) algo construya error: Some(..):
{ "type": "subgraph-node-end", "node_id": "Helper", "node_type": "llm_call",
  "output": null, "status": "error",
  "errorText": "Request failed: ... modelo no encontrado ...",
  "level": 1, "path": "agent>Helper" }
```

## Qué tiene que hacer ADP

Nada todavía. Cuando el primer emisor real aterrice, el cambio recomendado
será de una línea en cada sitio (`event-tree-builder.ts`,
`colmena-events.reducer.ts`), ya que ambos `closeNode` ya aceptan
`status`/`errorText`:

```ts
closeNode(ev.node_id, ev.output, ev.status === "error" ? "error" : "done", ev.errorText);
```

## Qué se rompe si se ignora

Nada — el campo no existe hoy en ningún frame real.
