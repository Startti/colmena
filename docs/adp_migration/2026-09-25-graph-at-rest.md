# Una fila de `dag_runs` guarda solo el esqueleto del grafo

**No setear `COLMENA_SUBGRAPH_RESUME_GRAPH` en un worker v0.18 después de desplegar
v0.19.** En v0.19 esa válvula no existe. En v0.18 correría el grafo guardado, y las
filas que escribe v0.19 ya no tienen config con qué correr.

**Acción de ADP:**
- **Al subir a v0.19.0 no hay cambios de código.** ADP no construye `ResumeGraph` ni
  lee `dag_runs.graph_json`:
  - `git grep -n ResumeGraph apps/service` da cero;
  - `graph_json` solo aparece en el esquema de Prisma.
- **Después del deploy, correr el backfill** que deja en reposo las filas escritas
  antes. En ADP es el script `scrub-dag-runs-at-rest`, por el workflow «Seed platform
  data»: primero en seco y después de verdad.

## Qué cambió (entrada 82)

- Los seis escritores de `DagRunState` guardan
  `GraphSkeleton::at_rest_json(&graph)`, no el grafo entero:
  - el arranque de un hijo;
  - las dos cancelaciones;
  - el watchdog;
  - el suspend;
  - el fin.

  La forma es exactamente esta:

  ```json
  { "nodes": { "<id>": { "type": "<tipo>" } },
    "edges": [ { "from": "a", "to": "b" }, { "from": "b", "to": "a", "cyclic": true } ] }
  ```

  `cyclic` va solo cuando es `true`. No van `config`, `timezone`, `location`, `locale`,
  `trigger_on` ni los topes de llamadas. El par de fixtures
  `src/libs/colmena/tests/fixtures/at_rest/` fija la forma. Un backfill que la escriba
  desde afuera tiene que dar el mismo JSON: ADP copia esos fixtures a su test.
- Vale para la raíz y para el hijo. La raíz nunca leyó su `graph_json`. El hijo solo lee
  el esqueleto al reanudar, desde la entrada 78.

## Superficie de Rust

- `ResumeGraph` queda con dos variantes, `Fresh(Value)` y `Unavailable(String)`. **Sale
  `Stored`**: quien la construya fuera del crate deja de compilar.
- Salen la válvula `COLMENA_SUBGRAPH_RESUME_GRAPH` y sus funciones privadas.
- Nueva: `GraphSkeleton::at_rest_json(&Graph) -> serde_json::Value`.

## Compatibilidad y rollback

- **Filas viejas, escritas por v0.18 o antes**, con el grafo entero: v0.19 las reanuda
  igual, porque el esqueleto se calcula del grafo entero.
- **Rollback de v0.19 a v0.18: seguro**, con la válvula apagada. Medido en el E2E:
  turno 1 con v0.19 (filas sin config) y turno 2 con v0.18, que termina bien.
- **Por debajo de v0.18: no.** v0.17 y anteriores reanudan con la copia guardada y
  correrían un grafo sin config.

## Qué cambió (entrada 83): `__graph_nodes`

- `global_shared_state.__graph_nodes` pasa de la `config` entera de cada nodo (claves
  incluidas, y el `child_graph_inline` de un `subgraph`) a
  `{ "<id>": { "description": "<texto>" } }`, solo para los nodos con una `description`
  de texto. Queda así en memoria y en reposo.
- Su único lector, el planner, lee `description` y no cambia. Un nodo sin descripción
  ya daba «No description provided.».
- Con las dos entradas, ninguna columna de una fila escrita por v0.19 guarda una clave
  de la config del grafo. Medido en el E2E con un centinela: `graph_json`,
  `global_shared_state` y `all_outputs` limpios, en la raíz y en el hijo.
- ADP: el backfill también reduce `__graph_nodes` en las filas viejas.

## Qué no cambia

- `all_outputs` no cambia: las salidas de las tools son datos de la corrida.
- La corrida en memoria no cambia: el motor sigue corriendo el grafo entero que le manda
  el embebedor.

## Qué se rompe si se ignora

Nada al compilar, salvo para quien construya `ResumeGraph::Stored`. Sin el backfill, las
filas escritas antes de v0.19 siguen con las claves en claro.
