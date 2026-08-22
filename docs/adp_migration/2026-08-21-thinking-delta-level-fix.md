# `thinking-delta` ahora llega al nivel y al linaje correctos

**Acción de ADP: NINGUNA obligatoria.** El tipo de evento no cambia. Cambian
`level` y `path`, que hasta ahora eran incorrectos.

## Qué pasaba

Un nodo `orchestrator` tiene LLMs internos: `planner`, `critic`,
`phase_reactor`. Emitía sus eventos por **dos caminos que no coincidían**:

- Los frames de estructura (`subgraph-node-start` / `subgraph-node-end`) salían
  envueltos → llegaban a nivel `N+1`, path `…>orchestrator>planner`.
- Los tokens de pensamiento salían sin envolver → llegaban a nivel `N`, path
  `…>planner`.

El mismo nodo lógico, en dos niveles y dos linajes distintos. Un árbol que
agrupe por `path` no podía colgar el pensamiento del planner debajo de su propio
`node-start`: quedaba huérfano un nivel más arriba.

Encima había un segundo problema. Cuando el orchestrator estaba **anidado dentro
de un `subgraph`** (el caso de los roles Testing e Implementation del creador de
agentes), ese token envuelto se mapeaba a `subgraph-text-delta` — el mismo tipo
que usa la respuesta del agente. O sea que el razonamiento interno de un planner
se **renderizaba como si fuera la respuesta al usuario**.

## Qué cambia

Los tokens de pensamiento ahora salen envueltos, al mismo nivel y bajo el mismo
path que su `node-start`. Y un `ThinkingToken` envuelto ya **no** cae en
`subgraph-text-delta`: sigue siendo `thinking-delta`.

Se evaluó introducir un tipo `subgraph-thinking-delta` y se descartó a
propósito: el razonamiento es razonamiento en cualquier nivel, `level` y `path`
ya alcanzan para ubicarlo, y la
[referencia de eventos](../sse_events_reference.md) ya establece que los frames
de thinking **no** son eventos de subgrafo. Mantener el nombre evita que ADP
tenga que aprender un tipo nuevo.

### Antes

```json
{ "type": "thinking-delta", "node_id": "planner", "node_type": "planner",
  "delta": "…", "level": 0, "path": "planner" }
```

…mientras su propio `subgraph-node-start` venía con `level: 1` y
`path: "orch>planner"`.

Y con el orchestrator anidado, peor todavía:

```json
{ "type": "subgraph-text-delta", "id": "txt_…", "node_id": "planner", "delta": "…" }
```

### Después

```json
{ "type": "thinking-delta", "node_id": "planner", "node_type": "planner",
  "delta": "…", "level": 1, "path": "orch>planner" }
```

Ahora `level` y `path` coinciden exactamente con los del `subgraph-node-start`
del mismo `node_id`.

## Qué tiene que hacer ADP

Nada obligatorio. Si el frontend emparejaba thinking con su nodo **por
`node_id`** (que es lo que la referencia de eventos recomendaba, justamente
porque el path no servía), eso sigue funcionando igual.

Si además usa `level` para indentar, ahora el thinking se indenta un nivel más
adentro, que es donde corresponde.

## Riesgo

**Bajo.** No aparecen ni desaparecen tipos de evento. Lo único que se mueve son
`level` y `path`, y se mueven **hacia** el valor correcto.

El único caso que cambia de tipo es el orchestrator anidado, donde el thinking
dejaba de ser thinking y salía como `subgraph-text-delta`. Si el frontend estaba
mostrando ese texto como respuesta del agente, ahora deja de aparecer ahí y pasa
a aparecer como thinking — que es lo que siempre debió ser.

No afecta el conteo de tokens ni la facturación: el `LlmUsage` de los LLMs
internos sigue saliendo sin envolver y atribuido al nodo `orchestrator`,
exactamente como antes. Fue una decisión deliberada — esos eventos no tienen
identidad de nodo propia, así que envolverlos habría movido la contabilidad de
tokens fuera del orquestador.

## Cómo verificar

Correr un agente con un `orchestrator`. Para cada `node_id` interno (`planner`,
`critic`, `phase_reactor`), el `level` y el `path` de sus `thinking-delta` deben
ser idénticos a los del `subgraph-node-start` que lo enmarca.
