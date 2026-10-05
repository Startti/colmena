# Progreso de una herramienta larga (`tool-progress`)

**Acción de ADP:** ninguna al compilar; subir el pin cuando se quiera mostrar la fila.
Ningún tool emite el evento todavía (lo conectará la ruta de archivos grandes, detrás del
interruptor `COLMENA_LARGE_TABULAR`), así que con el pin nuevo el flujo SSE no cambia.
El reductor de ADP (`colmena-events.reducer`) ya lee estos frames; un cliente que no los
conoce los ignora.

## Qué cambia

- Frames nuevos `tool-progress` y `subgraph-tool-progress` (este último con `level` y
  `path`, como todo frame `subgraph-`). Campos: `nodeId`, `toolCallId`, `stage`
  (`queued`, `preparing`, `staging`, `running` o `collecting`) y `elapsedMs`; `done`,
  `total` y `unit` solo cuando el paso los conoce (un valor desconocido se omite, nunca
  llega como `null` ni como `0`).
- `toolCallId` es el id de la llamada a la herramienta, el mismo que lleva
  `tool-input-available`; el reductor lo usa para encontrar la fila de la herramienta.
- Rust: `NodeEvent::ToolProgress` y `DagExecutionEvent::ToolProgress` son variantes
  nuevas, y `ToolProgressStage` es un tipo nuevo. Comprobado en el repo de ADP
  (no en este): `git grep 'DagExecutionEvent::\|NodeEvent::' -- apps/service` da solo
  archivos `.md`, así que no hay `match` exhaustivo sobre esos tipos.
