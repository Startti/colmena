# La frontera de un `subgraph`-as-tool ahora cierra cuando su child falla

**Acción de ADP: recomendada** — la mitigación de path-prefix en
`fix/dangling-subgraph-nodes` se vuelve un no-op para la frontera.

## Qué pasaba / qué cambia

El child de un `subgraph`-as-tool que fallaba propagaba el error sin emitir
cierre: la frontera quedaba abierta para siempre (reproducido: 2
`subgraph-node-start`, 0 `subgraph-node-end`). Ahora el executor se resuelve
antes del `node-start`, y en `Err` de `run_subgraph(...)` la frontera cierra
antes de re-lanzar:

```jsonc
{ "type": "subgraph-node-end", "node_id": "Helper", "output": null,
  "status": "error", "errorText": "Error de ejecución en el nodo: ...",
  "level": 1, "path": "agent>Helper" }
```

`errorText` solo acompaña al cierre cuando vino del despacho como tool
(`__colmena_tool_name`) — el único camino masked por `MaskingObserver` (#310);
agente del `orchestrator` o por aristas cierran igual, sin `errorText`.
SUSPENDED y resume no cambian.

## Qué queda pendiente / qué tiene que hacer ADP

El nodo **interno** que falló todavía no cierra su propio `subgraph-node-end`
(PR siguiente). La mitigación de path-prefix en `fix/dangling-subgraph-nodes`
se vuelve un no-op para la frontera — el motor ya la cierra; dejarla activa no
hace daño.

## Qué se rompe si se ignora

Nada nuevo. Sin [el cambio recomendado](2026-09-15-node-end-error-status.md),
la frontera igual se cierra (por `node_id`), solo se pinta `'done'`.
