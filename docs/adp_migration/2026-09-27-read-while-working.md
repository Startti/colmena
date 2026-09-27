# Leer un mensaje a mitad de corrida

**Acción de ADP:** subir el pin a `colmena_dag_engine-v0.21.5`. El worker arma un
`SteeringInbox` en Redis para los jobs que piden `steering: true`, se lo da a
`RunControl::with_steering` y lo cierra al terminar el job; el árbol lee el frame
`user-message-consumed`. Si no lo hace no se rompe nada: sin buzón todo corre como hoy.

## Qué cambia

- `RunControl::with_steering(Arc<dyn SteeringInbox>)`: el agente de la raíz lee, entre pasos,
  los mensajes que esperan en el buzón. `SteeringInbox` (`colmena::llm::domain::steering`)
  tiene tres operaciones: `take()` (todo lo que espera, en orden; sigue abierto),
  `take_or_close()` (en una sola operación: lo que espera o, sin nada, cierra) y `close()`
  (cierra y descarta lo que quedaba; dos veces no hace nada). Hacia el motor no falla: una
  implementación que no llega a su almacén devuelve nada. Un mensaje es
  `SteeringMessage { id, text }`, con el id del cliente (`[A-Za-z0-9_.-]{1,256}`, no solo
  puntos; otro id se salta, igual que un texto en blanco).
- Cuándo lee: en el tope de cada paso (después de todos los resultados del paso anterior:
  un grupo en paralelo se espera entero) y en la respuesta final, que toma o cierra. Cada
  mensaje leído se guarda en la historia del agente como `user`, después de los resultados.
  El buzón se cierra al volver el bucle, como sea que vuelva; el worker igual lo cierra al
  terminar el job (un Stop suelta el bucle a mitad de un `await`).
- Solo el `llm_call` de la raíz lee. Un hijo, el trabajo de una llamada, las filas de un
  `for_each` y los nodos internos (`critic`, `planner`, `reactor`, `orchestrator`) nunca.
  Cerrar es del job: está pensado para un grafo de un solo agente, como el de Auto.
- Rust: `LlmStreamPart`, `NodeEvent` y `DagExecutionEvent` ganan `UserMessageConsumed`;
  `AgentService::run_steered(params, inbox)` es nuevo. ADP no hace `match` exhaustivo sobre
  esos tipos (`git grep 'DagExecutionEvent::\|NodeEvent::\|LlmStreamPart::' apps/service`
  da solo archivos `.md`).

## Qué hace el worker

Hoy arma `RunControl::new(token)` para cada job (`worker/src/main.rs`). Con un job que pidió
`steering: true`, le suma `.with_steering(inbox)`, con un buzón en Redis que cumpla el
contrato del trait:

- `take`, `take_or_close` y `close` son cada una **una** operación atómica (un script): entre
  «no hay nada» y «cerrado» no puede entrar un mensaje, y un mensaje no se lee dos veces.
- Dejar un mensaje (la ruta de la API del worker) lo acepta solo con el buzón abierto; con el
  buzón cerrado o sin buzón contesta que no, y el cliente lo manda como turno. Un id repetido
  entra una vez.
- Nunca falla hacia el motor ni lo frena: con Redis caído o lento devuelve nada, con un tope
  corto por operación.
- Al volver el job, como sea que vuelva (terminó, Stop, error), `close()`. Las claves vencen
  solas por si el worker muere antes.

Detalle: [12 → Leer un mensaje a mitad de corrida](../developer_guide/12_dag_engine_guide.md#leer-un-mensaje-a-mitad-de-corrida-steeringinbox)
y [19 → Un hijo nunca lee lo que escribe la persona](../developer_guide/19_nested_agents_and_subgraphs.md#un-hijo-nunca-lee-lo-que-escribe-la-persona).

## El frame

`{"type":"user-message-consumed","id":"<id>","node_id":"<nodo>"}` (más `level` y `path`),
solo de nivel superior, después de los `tool-output-available` del paso. Lleva el id, no el
texto: quien arma el árbol pone ahí el mensaje que ya tiene. No contiene `"type":"finish"` ni
`"type":"error"`, así que la API del worker no corta el stream por él. Con un Stop o el vigía
de inactividad, un mensaje ya guardado sale antes de `cancelled` o del error. Lo que el
agente escribe después sigue con el mismo `id` de texto: el árbol lo pone después del
mensaje. Frames: [referencia de SSE](../sse_events_reference.md#mensaje-leído--user-message-consumed).
E2E: `src/libs/colmena/tests/read_while_working.rs`.
