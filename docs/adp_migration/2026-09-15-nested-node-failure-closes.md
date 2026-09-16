# Un nodo interno que falla dentro de un run anidado ahora cierra

**Acción de ADP: recomendada** — la mitigación de path-prefix en
`fix/dangling-subgraph-nodes` se vuelve totalmente un no-op; recomendamos
migrar `closeNode` a matchear por `path` (no solo `node_id`), porque filas
concurrentes de `for_each` comparten el mismo `node_id` interno bajo `path`s
distintos.

## Qué pasaba / qué cambia

Un nodo que fallaba **dentro** de un run anidado nunca cerraba su propio
`node-end`/`subgraph-node-end` a ninguna profundidad — solo la frontera del
subgraph que lo contenía cerraba (#313). Ahora cierra con `status:"error"`
(+ `errorText` solo si ya estaba enmascarado, #312) antes de que el error
suba, sin excepción por tipo: un `subgraph` anidado recibe este cierre IGUAL
que cualquier nodo, además del self-close que `SubGraphNode` ya emite un
nivel más adentro (#313) — dos pares start/end distintos, no un duplicado.
Contrato completo en
[sse_events_reference.md](../sse_events_reference.md#nodo-que-falla) y
[guide 19](../developer_guide/19_nested_agents_and_subgraphs.md#cuando-el-sub-agente-falla).
El run **raíz** no cambia.

## Qué queda pendiente / qué se rompe si se ignora

Nada bloqueante ni nuevo — un nodo interno que fallaba ya se veía
indefinidamente "corriendo"; ahora se pinta `error`. Sí recomendamos migrar
`closeNode` a matchear por `path`: en `for_each` con filas concurrentes,
cada fila comparte el mismo `node_id` interno bajo un `path` distinto
(`<node>#0>...`, `<node>#1>...`), y un `closeNode` por `node_id` puede
cerrar la fila equivocada — riesgo preexistente que ignorar esto mantiene.
