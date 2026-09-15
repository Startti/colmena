# La frontera de un `subgraph`-as-tool ahora cierra cuando su child falla

**Acción de ADP: recomendada** — la mitigación de path-prefix en
`fix/dangling-subgraph-nodes` se vuelve un no-op para la frontera.

## Qué pasaba / qué cambia

El child de un `subgraph`-as-tool que fallaba propagaba el error sin emitir
cierre: la frontera quedaba abierta para siempre (reproducido: 2
`subgraph-node-start`, 0 `subgraph-node-end`). Ahora cierra con
`status:"error"` (+ `errorText` solo en el camino masked por
`MaskingObserver`) antes de re-lanzar — contrato completo y ejemplo en
[guide 19](../developer_guide/19_nested_agents_and_subgraphs.md#cuando-el-sub-agente-falla)
y [sse_events_reference.md](../sse_events_reference.md#nodo-que-falla).
SUSPENDED y resume no cambian.

## Qué queda pendiente / qué tiene que hacer ADP

El nodo **interno** que falló todavía no cierra su propio `subgraph-node-end`
(PR siguiente). La mitigación de path-prefix se vuelve un no-op para la
frontera — el motor ya la cierra; dejarla activa no hace daño.

## Qué se rompe si se ignora

Nada nuevo. Sin [el cambio recomendado](2026-09-15-node-end-error-status.md),
la frontera igual se cierra (por `node_id`), solo se pinta `'done'`.
