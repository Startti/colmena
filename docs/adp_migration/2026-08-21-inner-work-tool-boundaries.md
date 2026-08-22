# Fronteras de stream para `llm_call` y `for_each` despachados como tool

**Acción de ADP: RECOMENDADA.** Aparecen frames nuevos, ninguno desaparece.
El frontend puede simplificar: un solo camino de código donde antes hacían falta
dos.

## Qué pasaba

Tres tipos de nodo pueden despacharse como tool y correr trabajo interno propio.
Solo uno delimitaba su sub-árbol en el stream:

| Tool | Corre trabajo interno | Emitía frontera |
|------|----------------------|-----------------|
| `subgraph` | sí (un grafo hijo) | **sí** |
| `llm_call` | sí (su propio loop de agente) | **no** |
| `for_each` | sí (una corrida del target por fila) | **no** |

`SubGraphNode` emite su par de frontera desde dentro del nodo. Los otros dos no
tenían nada equivalente: sus frames anidados llegaban sin ningún marcador que
dijera dónde abre y dónde cierra ese sub-árbol.

No se perdía información — un consumidor podía acotarlos con el
`tool-input-available` / `tool-output-available` de nivel 0 más el campo `path`.
Pero obligaba al frontend a tener **dos caminos de código**: uno para tools con
frontera y otro para tools sin ella, para renderizar exactamente la misma idea.

Este gap es **anterior** a esta tanda de cambios. Lo que hizo el fix de
[`llm_call` como tool](2026-08-21-llm-call-as-tool-nesting.md) fue volverlo
visible: antes esos eventos salían en nivel 0 atribuidos al padre, así que la
pregunta de la frontera ni siquiera se planteaba.

## Qué cambia

Todo tool que corre trabajo interno emite ahora el mismo par que `subgraph`:

```
subgraph-node-start   { node_id: "<nombre del tool>", node_type: "llm_call" | "for_each" }
  … frames anidados del sub-árbol …
subgraph-node-end     { node_id: "<nombre del tool>", output }
```

La frontera va **un nivel por encima** del contenido que delimita, igual que ya
ocurría con un subgraph-as-tool. El `node_type` dice qué corre adentro.

Ejemplo real (`tests/graphs/agents/nested_llm_agents_showcase.json`), un
`llm_call` expuesto como tool `consultar_experto`:

```
L0  tool-input-available    consultar_experto
L1  subgraph-node-start     consultar_experto   node_type: llm_call   ← nuevo
L1  subgraph-text-delta     path: analista>consultar_experto
L1  subgraph-node-end       consultar_experto                          ← nuevo
L0  tool-output-available   consultar_experto
```

Los nodos hoja (`http_request`, `sql_query`, `multiply`, …) **no** emiten
frontera: no corren nada adentro, no hay sub-árbol que delimitar.

## Riesgo

**Bajo.** Es puramente aditivo: aparecen dos frames nuevos por cada tool de esos
dos tipos, y ninguno de los que ya llegaban cambia de forma, de tipo ni de nivel.

El riesgo concreto es que el frontend **cuente nodos** o renderice una fila por
cada `subgraph-node-start` que ve. Un agente con muchas tools de tipo `llm_call`
o `for_each` va a mostrar más nodos que antes — que es el comportamiento
correcto, pero conviene revisar cualquier lugar que asuma un conteo fijo.

Si el frontend ya maneja `subgraph-node-start` / `subgraph-node-end` para
subgrafos, no hay nada que hacer: los frames nuevos son de la misma forma y el
código existente los toma tal cual.

## Cómo verificarlo

```bash
cargo run --bin dag_engine -- run tests/graphs/advanced/nested_sse_remediation_e2e.json \
  --agent-session-id verificacion_001 > /tmp/salida.sse
python3 scripts/verify_nested_sse_e2e.py /tmp/salida.sse
```

El check `frontera también para llm_call/for_each como tool` cubre este cambio.
Para verlo en forma de árbol:

```bash
python3 scripts/render_nested_sse.py /tmp/salida.sse --tree
```
