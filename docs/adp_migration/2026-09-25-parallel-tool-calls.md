# Una llamada a una tool `parallel` trae su propia identidad (`childScope`)

**Acción de ADP:** soportar `childScope` **antes** de subir el pin del motor. Ya está
hecho en Startti/adp#855 (mergeada en `develop` el 2026-09-25): el árbol de eventos, la
traza, la facturación y las ejecuciones cuelgan la frontera de una llamada en
`${path}>${childScope}`. Para las tools sin `parallel` no cambia nada.

## Qué cambió

Una entrada de `tool_configurations` puede declarar `"parallel": true` (booleano; otro
valor falla la validación del grafo al cargar). Cada llamada de esa tool:

- abre su frontera como `<tool>#<k>`, donde k es el índice de la llamada en el mensaje
  `tool_calls` del modelo. Es la posición en el mensaje, no un contador por tool, y
  pasa aunque sea la única llamada del mensaje;
- lleva `childScope: "<tool>#<k>"` en `tool-input-available` y `tool-output-available`,
  y en sus variantes `subgraph-tool-input-available` y `subgraph-tool-output-available`.

La memoria no cambia: el `node_id` de la conversación del hijo sigue saliendo de
`memory_mode` (`tool/<tool>/<thread>` en `dynamic`). En este paso las llamadas todavía
corrían una después de la otra; desde el paso 2, abajo, un grupo corre a la vez.

### Antes / después

Mismo mensaje del modelo: `Run`, `Nota`, `Run`, con `Run` declarada `parallel`.

```json
// antes: las dos llamadas a Run abrían la misma frontera
{ "type": "tool-input-available", "toolCallId": "call_clima", "toolName": "Run", "input": { "task": "clima" }, "path": "agent" }
{ "type": "subgraph-node-start",  "node_id": "Run", "node_type": "subgraph", "path": "agent>Run" }

// después
{ "type": "tool-input-available", "toolCallId": "call_clima", "toolName": "Run", "input": { "task": "clima" }, "childScope": "Run#0", "path": "agent" }
{ "type": "subgraph-node-start",  "node_id": "Run#0", "node_type": "subgraph", "path": "agent>Run#0" }
{ "type": "tool-input-available", "toolCallId": "call_nota", "toolName": "Nota", "input": { "texto": "empecé" }, "path": "agent" }
{ "type": "subgraph-node-start",  "node_id": "Nota", "node_type": "subgraph", "path": "agent>Nota" }
{ "type": "tool-input-available", "toolCallId": "call_precios", "toolName": "Run", "input": { "task": "precios" }, "childScope": "Run#2", "path": "agent" }
{ "type": "subgraph-node-start",  "node_id": "Run#2", "node_type": "subgraph", "path": "agent>Run#2" }
```

Los frames de «después» son del E2E `src/libs/colmena/tests/parallel_tool_identity.rs`
(grafo `tests/graphs/agents/parallel_tool_identity.json`), recortados.

## Qué no cambia

- **Tools sin `parallel`:** sus frames no traen `childScope` (ausente, nunca `null`) y
  su frontera conserva el nombre pelado. Son idénticos byte a byte a los de antes.
- **`tool-input-start` nunca trae `childScope`**, tampoco en una tool `parallel`. Sale
  del chunk del stream, antes de que la llamada tenga su k, y un turno sin streaming no
  lo emite. El valor se toma de `tool-input-available`, por el mismo `toolCallId`. El
  reducer de ADP ya lo lee ahí; que también lo busque en `tool-input-start` no molesta,
  porque allí no llega.

## Qué se rompe si se ignora

Nada al compilar. Si se sube el pin sin soportar `childScope`, las fronteras de una tool
`parallel` llegan como `agent>Run#0` y `agent>Run#2`, y el árbol de ADP no las cuelga de
su llamada. Solo afecta a los grafos que declaran `parallel: true` en alguna tool: un
grafo sin ese campo produce exactamente los frames de antes.

## Paso 2: un grupo de llamadas `parallel` corre a la vez

**Acción de ADP:** ninguna de código, porque el soporte de `childScope` ya está
(Startti/adp#855, mergeada). Pero **no declarar `parallel` en ninguna tool todavía**:
esperar el paso que maneja varias preguntas en un mismo grupo (ver abajo).

Desde este paso, las llamadas seguidas a tools `parallel` de un mismo mensaje del
modelo forman un grupo que corre concurrente: hasta `COLMENA_MAX_PARALLEL_TOOL_CALLS`
cadenas a la vez (default 4), con las llamadas que comparten hilo de memoria en serie.
Una tool sin `parallel` es una barrera y corre sola, como siempre. Un grafo sin
`parallel` produce los mismos frames que antes, en el mismo orden.

Lo que ve ADP en un grupo:

- **Los frames se intercalan.** Cada `tool-input-available` sale cuando su llamada
  empieza y cada `tool-output-available` cuando termina, así que el de una llamada
  pedida después puede llegar primero, y los frames de los hijos (`subgraph-*`) de dos
  llamadas se mezclan en el stream. Se asocian por `toolCallId` y por `childScope`, que
  es justo lo que hace ADP desde #855. Nada puede suponer que el
  `tool-output-available` de una llamada llega antes del `tool-input-available` de la
  siguiente.
- **La historia de la conversación queda en el orden del modelo**, no en el orden en
  que terminaron.
- **Una repetición que el guard contesta** emite sus frames después de los del grupo.

Frames reales del E2E `src/libs/colmena/tests/parallel_tool_groups.rs` (grafo
`tests/graphs/agents/parallel_tool_groups.json`), recortados; `precios` cierra antes que
`clima` aunque el modelo la pidió segunda:

```json
{ "type": "tool-input-available",  "toolCallId": "call_clima",   "childScope": "Run#0", "path": "agent" }
{ "type": "subgraph-node-start",   "node_id": "Run#0", "node_type": "subgraph", "path": "agent>Run#0" }
{ "type": "tool-input-available",  "toolCallId": "call_precios", "childScope": "Run#1", "path": "agent" }
{ "type": "subgraph-node-start",   "node_id": "Run#1", "node_type": "subgraph", "path": "agent>Run#1" }
{ "type": "subgraph-node-end",     "node_id": "Run#1", "node_type": "subgraph", "path": "agent>Run#1" }
{ "type": "tool-output-available", "toolCallId": "call_precios", "childScope": "Run#1", "path": "agent" }
{ "type": "subgraph-node-end",     "node_id": "Run#0", "node_type": "subgraph", "path": "agent>Run#0" }
{ "type": "tool-output-available", "toolCallId": "call_clima",   "childScope": "Run#0", "path": "agent" }
```

Con cada hijo durmiendo 2 s y 2,3 s, el grupo tarda 2,31 s del primer
`tool-input-available` al último `tool-output-available`; con `parallel: false`, 4,34 s.

### Por qué no declarar `parallel` en este paso

En este paso, si en un grupo suspendía más de una llamada (dos hijos que preguntan), el
motor conservaba solo la primera pregunta en el orden del modelo. Las otras llamadas
recibían el marcador «NO se ejecutó» aunque corrieron, y sus hijos quedaban suspendidos.
El paso 3, abajo, lo cierra: desde ahí se puede declarar `parallel`.

## Paso 3: una pregunta por turno dentro de un grupo

**Acción de ADP:** ninguna de código. Desde este paso, `parallel` se puede declarar en
una tool cuyo hijo pregunta, como Run My Agent.

Cuando uno o más hijos de un grupo preguntan, el motor espera a que termine el grupo y
suspende el turno en **una** pregunta: la de la primera llamada en el orden del modelo,
aunque otra haya preguntado antes. Lo que ve ADP:

- **Un solo `finish` con `finishReason: "suspended"`**, con la pregunta de esa llamada
  en `output.questions` y su id en `output._pending_tool_call_id`. Se muestra y se
  reanuda como cualquier otra pregunta.
- **Los hermanos que terminaron llegan antes.** Su `tool-output-available`, con su
  `childScope`, sale antes del `finish`, y su resultado queda en la historia.
- **Cada otra pregunta se cierra.** Su llamada recibe un `tool-output-available` (con
  su `childScope`) cuyo `output` es el texto que lee el modelo:
  «Este agente hizo una pregunta mientras otro también preguntaba. No terminó: volvé a
  correrlo solo cuando termine el otro.» Es un string, no un objeto. La fila del hijo en
  `dag_runs` pasa a `FAILED`, con sus descendientes suspendidos, así que el padre queda
  con un solo hijo `SUSPENDED`. La frontera de ese hijo (`subgraph-node-start` en
  `agent>Run#1`) no recibe `subgraph-node-end`, como la de cualquier hijo suspendido.
- **Las llamadas que no corrieron** (las que venían después de una pregunta en su misma
  cadena, y las pedidas después del grupo) reciben el marcador «NO se ejecutó», como en
  el paso 2.
- **El resume** reanuda el hijo de la pregunta que quedó, con el mismo `childScope`: sus
  frames llegan con `path` `agent>Run#0>…`. Como en todo resume de un `subgraph` usado
  como tool, ese turno no emite otro `subgraph-node-start` de la frontera ni un
  `tool-output-available` de la llamada reanudada. Tampoco emite el `subgraph-node-end`
  de la frontera: la que se abrió en el turno de la pregunta no se cierra nunca.
- **Re-correr el agente cerrado funciona.** El modelo puede volver a llamarlo en una
  corrida siguiente, o en el mismo turno reanudado (después de leer el texto de cierre),
  y lo hace en el mismo hilo de memoria. Ese hilo termina con la pregunta abierta, y
  la corrida fresca la contesta primero con otro texto
  («Esta llamada quedó sin resultado: la conversación siguió sin ella (se cortó, o era
  una pregunta que no se contestó). No la retomes; si todavía hace falta, volvé a
  hacerla.»). Sin eso, Anthropic y OpenAI devolverían 400 en ese hilo para siempre.
  Esta curación vale para **todos** los agentes en una corrida fresca, no solo para Run
  My Agent. También cubre un Stop o el watchdog a mitad de turno: el mensaje del
  asistente ya estaba guardado con sus ids abiertos, y los resultados que ya habían
  terminado en un grupo se pierden, porque se escriben al cerrar el grupo.

Frames reales del E2E `src/libs/colmena/tests/parallel_tool_suspend.rs` (grafo
`tests/graphs/agents/parallel_tool_suspend.json`), recortados. Los dos hijos preguntan;
`beta` antes, pero el modelo pidió `alfa` primero:

```json
{ "type": "tool-input-available",  "toolCallId": "call_alfa", "childScope": "Run#0", "path": "agent" }
{ "type": "subgraph-node-start",   "node_id": "Run#0", "node_type": "subgraph", "path": "agent>Run#0" }
{ "type": "tool-input-available",  "toolCallId": "call_beta", "childScope": "Run#1", "path": "agent" }
{ "type": "subgraph-node-start",   "node_id": "Run#1", "node_type": "subgraph", "path": "agent>Run#1" }
{ "type": "subgraph-tool-input-available", "toolCallId": "ask_beta", "toolName": "Preguntar", "path": "agent>Run#1>hijo" }
{ "type": "subgraph-tool-input-available", "toolCallId": "ask_alfa", "toolName": "Preguntar", "path": "agent>Run#0>hijo" }
{ "type": "tool-output-available", "toolCallId": "call_beta", "output": "Este agente hizo una pregunta mientras otro también preguntaba. No terminó: volvé a correrlo solo cuando termine el otro.", "childScope": "Run#1", "path": "agent" }
{ "type": "finish", "finishReason": "suspended", "output": { "questions": [{ "id": "pregunta_hijo", "question": "¿alfa: seguimos?" }], "_pending_tool_call_id": "call_alfa" } }
```

Filas después de ese turno: en `dag_runs`, el hijo de `alfa` queda `SUSPENDED` y el de
`beta` `FAILED`. En `llm_node_history` del padre, el `tool` de `call_beta` tiene el texto
de arriba. El turno con la respuesta termina, y un tercer turno que vuelve a correr
`beta` le manda a su modelo un hilo sin ids abiertos.

## Documentación

- [sse_events_reference.md](../sse_events_reference.md#childscope--una-llamada-a-una-tool-parallel)
- [Guía 19, «Varias llamadas a la misma tool en un turno»](../developer_guide/19_nested_agents_and_subgraphs.md#varias-llamadas-a-la-misma-tool-en-un-turno-parallel)
- [Guía 19, «Un grupo de llamadas `parallel` corre a la vez»](../developer_guide/19_nested_agents_and_subgraphs.md#un-grupo-de-llamadas-parallel-corre-a-la-vez)
- [Guía 19, «Suspensión dentro de un batch paralelo de tools»](../developer_guide/19_nested_agents_and_subgraphs.md#suspensión-dentro-de-un-batch-paralelo-de-tools)
