# `Graph::validate()` corre en toda entrada al motor

**Fecha:** 2026-09-03 · **PR:** cierre del hueco abierto por #248

## Qué cambió

Hasta ahora sólo el CLI validaba el grafo antes de correrlo. Las entradas de
librería —`execute_stream`, `execute_stream_cancellable`, `run_dag`,
`stream_sse_parts`— recibían un `Graph` ya deserializado y lo ejecutaban sin
verificar. **Ese es el camino que usa el worker de ADP.**

La validación pasó al único punto donde convergen todas las entradas
(`DagRunUseCase::execute_stream`), junto al pre-flight de provider keys.

## Qué ve ADP

Un grafo con cualquiera de estas cuatro cosas ahora falla **antes de ejecutar**,
con un error nombrando el nodo, en vez de fallar más tarde o en silencio:

| Caso | Antes | Ahora |
|---|---|---|
| node id con `/` | paths de subgrafo ambiguos | error al inicio |
| `node_schema` malformado | fallaba igual, al construir tools | error al inicio, mejor mensaje |
| `memory_mode` inválido | fallaba igual, al construir tools | error al inicio, mejor mensaje |
| bloque `mcp` mal configurado | **silencio** — el servidor se ignoraba | error al inicio |

**Dos de los cuatro ya fallaban de todos modos**; para esos esto sólo adelanta
el error. Los otros dos son fallas nuevas, y son justamente las que antes no se
veían.

## Acción de ADP

**Ninguna obligatoria.** El error viaja por el stream como cualquier otro
`DagError`, así que el manejo actual lo cubre. No cambia el formato SSE.

## Si esto rompe algo

Válvula de seguridad, sin rollback ni redeploy del motor:

```
COLMENA_GRAPH_VALIDATION=off
```

Misma forma que `COLMENA_PREFLIGHT_HEALTH=off`. Si hay que usarla, conviene
avisar: significa que hay grafos en producción que el motor considera
estructuralmente inválidos, y lo que corresponde es arreglarlos.

## Riesgo residual, dicho con honestidad

No se pudo inspeccionar la base de grafos de producción de ADP desde el entorno
de desarrollo, así que **no se puede afirmar que ningún grafo vivo falle**. Lo
que sí se verificó: dos de las cuatro clases ya fallaban más adelante, y las
otras dos (`mcp`, node id con `/`) son improbables — MCP es una feature de este
mismo mes, y el canvas de ADP no genera ids con `/`.
