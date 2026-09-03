# `validate_graph` ahora valida de verdad

**Fecha:** 2026-09-02 · **Afecta:** bindings PyO3 y napi · **Acción de ADP:** ninguna

## Qué cambió

`validate_graph` (Python) / `validateGraph` (Node) **solo deserializaba**. No
llamaba a `Graph::validate()`, pese a que su documentación afirmaba replicar la
estrictez de `cargo run -- run <file>`. Ahora sí la llama.

## Antes y después

| Grafo | Antes | Ahora |
|---|---|---|
| node id con `/` (ej. `"a/b"`) | pasaba | **`DagException` / `DagError`** |
| `node_schema` malformado (campo `array` sin `items`) | pasaba | **rechazado** |
| `memory_mode` inválido, o con memoria y sin `connection_url` | pasaba | **rechazado** |
| bloque `mcp` sin `url`, con URL no-HTTPS, o en un tool que no es MCP | pasaba | **rechazado** |
| campo de config inventado (`"modle"`) | pasaba | sigue pasando — para eso está `dag_engine lint` |

Los cuatro casos que ahora se rechazan **ya fallaban al ejecutar**. El cambio
adelanta el error al momento de validar; no invalida ningún grafo que antes
corriera bien.

## Acción de ADP: ninguna

Verificado antes de hacer el cambio: **`apps/service/ia/platform/` no usa
`validate_graph` ni `validateGraph` en ningún lado**. El worker recibe el grafo
por Redis y entra por `ColmenaEngine::execute_stream_cancellable`
(`worker/src/main.rs`), que recibe un `Graph` ya deserializado y no pasa por esta
función.

Si en algún momento ADP empezara a llamarla, el único riesgo es que un grafo
persistido con alguno de los cuatro defectos de arriba deje de pasar la
validación — pero ese grafo tampoco se puede ejecutar hoy.

> **Nota aparte, no incluida en este cambio:** el camino de producción de ADP
> tampoco valida. `execute_stream_cancellable` recibe un `Graph` sin llamar a
> `Graph::validate()`, así que un grafo con node id inválido o `node_schema`
> malformado llega hasta la ejecución. Cablearlo ahí es un cambio con impacto en
> producción y necesita su propia decisión; queda anotado en `docs/BACKLOG.md`.
