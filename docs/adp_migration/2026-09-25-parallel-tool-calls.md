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
`memory_mode` (`tool/<tool>/<thread>` en `dynamic`). Las llamadas todavía corren una
después de la otra.

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

## Documentación

- [sse_events_reference.md](../sse_events_reference.md#childscope--una-llamada-a-una-tool-parallel)
- [Guía 19, «Varias llamadas a la misma tool en un turno»](../developer_guide/19_nested_agents_and_subgraphs.md#varias-llamadas-a-la-misma-tool-en-un-turno-parallel)
