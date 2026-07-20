# `for_each` — ejecución determinista de tools sobre una lista

## Qué es

`for_each` es un nodo (`node_type: "for_each"`) que ejecuta un tool
embebido (`target`) **una vez por cada fila de una lista**, de forma
**determinista**: la iteración ocurre en Rust (`run_list` /
`ListToolExecutor`), no porque el LLM vuelva a llamar el mismo tool N veces
dentro de su propio loop.

Es **un solo nodo, usable de dos formas**:

1. **Como nodo de grafo** — `config` estático declara `target`/`items`/
   políticas; se ejecuta al llegar a ese nodo en el DAG, sin LLM.
2. **Como tool de un `llm_call`** — se declara en `tool_configurations` con
   `node_schema`; el LLM decide cuándo invocarlo y aporta la lista de
   filas (`items`), mientras `target` y las políticas quedan `fixed`
   (el LLM nunca las elige).

En ambos casos el resultado es el mismo: una tabla `{ total, ok, err,
results[] }` con una entrada por fila, en el orden original,
independientemente de la concurrencia usada para ejecutarlas.

Motivación: sin `for_each`, "actualiza el plan de estos 20 usuarios" hace
que el LLM re-llame el mismo tool 20 veces dentro de su loop — lento, caro
en tokens, y no determinista (el modelo puede saltarse filas o repetirlas).
`for_each` mueve esa iteración al motor: el LLM arma la lista una vez y el
fan-out ocurre en Rust.

## El contrato `target`

```json
{
  "target": {
    "node_type": "http_request",
    "node_schema": { "...": "mismo formato que un tool_configurations normal" }
  }
}
```

- `node_type` debe ser un `ExecutableNode` **registrado** (ver
  `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`). Puede ser
  cualquier nodo: `http_request`, `sql_query`, `python_script`, `add`, o
  incluso `subgraph` (agents-as-tools por fila — ver ejemplo abajo).
- `node_type: "for_each"` está **prohibido** (guard anti-recursión) — un
  `for_each` no puede targetearse a sí mismo.
- `node_schema` sigue exactamente las mismas convenciones que un
  `tool_configurations.<tool>.node_schema` normal: campos `fixed` (plumbing
  estático, invisible para la fila), campos con `type`/`required`
  (rellenados desde la fila), y contenedores anidados (`properties`) — el
  merge usa la misma función `merge_args_into_schema` que usa el executor
  de tools LLM normal.
- Cada fila se fusiona dentro de ese `node_schema` y se valida contra sus
  campos `required` **antes** de despachar — una fila con un campo
  requerido faltante se reporta como `status: "err"` para esa fila
  únicamente (no aborta las demás, salvo `on_error: "abort"`).

## `items` vs `items_from`

La lista de filas se resuelve en este orden (config-first, luego inputs —
ver "Config vs tool" más abajo):

1. **`items`** — array inline de objetos, uno por fila. Es el campo
   típico que un LLM completa cuando se expone como tool.
   ```json
   "items": [{ "user_id": 101, "plan": "pro" }, { "user_id": 102, "plan": "pro" }]
   ```
2. **`items_from`** — handle de fuente de datos, para leer filas sin que
   el modelo las re-escriba:
   ```json
   "items_from": {
     "source": "sheet",
     "ref": "<spreadsheet_id>|<sheet>|<range?>",
     "column": "user_id",
     "as": "uid"
   }
   ```
   - **v1 solo soporta `source: "sheet"`** — lee un Google Sheet vía
     `dispatch_gsheets_read` (mismo backend que `gsheets_read`),
     `ref = "<spreadsheet_id>|<sheet>|<range opcional>"`.
   - `column`/`as` seleccionan y renombran una sola columna de cada fila
     (p.ej. quedarte solo con `user_id` y renombrarlo a `uid`). Sin
     `column`, las filas pasan completas.
   - **`source: "attachment"` está diferido a v1.1** — un `ExecutableNode`
     plano no puede resolver `document_id → bytes` hoy (esa resolución
     vive en `DagToolExecutor`, no se inyecta a los nodos). **No lo
     configures** — falla con `unknown source 'attachment'`. Si necesitas
     leer un CSV/XLSX adjunto, usa un nodo upstream
     (`data_run_python`/lectura) que alimente el edge de entrada de
     `for_each` (ver punto 3).
3. **Edge de entrada** (`input` o `default`) — si ni `items` ni
   `items_from` están configurados, `for_each` lee un array desde su
   input port por defecto. Solo aplica al uso como **nodo de grafo**
   (un tool no tiene edges).

Si el número de filas resuelto excede `max_items`, se trunca (con log de
advertencia) antes de despachar.

## Políticas: `on_error`, `concurrency`, `max_items`

| Campo | Default | Descripción |
|---|---|---|
| `on_error` | `"continue"` | `"continue"` corre todas las filas pese a fallos anteriores, reportando ok/err por fila. `"abort"` deja de despachar filas nuevas apenas una falla (best-effort bajo concurrencia — las filas ya en vuelo terminan). |
| `concurrency` | `1` | Filas despachadas al target simultáneamente. `1` = secuencial. Valores `< 1` se ajustan a `1`. |
| `max_items` | `1000` | Tope duro de filas procesadas; protege contra un `items_from` desbocado o un array gigante enviado por el LLM. |

Cuando `for_each` se expone como **tool**, `on_error`/`concurrency`/
`max_items` casi siempre van `fixed` en `node_schema` — son decisiones de
diseño del operador del grafo, no algo que el LLM deba elegir por llamada.

## La tabla de resultados

```json
{
  "output": {
    "total": 3,
    "ok": 2,
    "err": 1,
    "results": [
      { "index": 0, "input": { "user_id": 101, "plan": "pro" }, "status": "ok", "output": { "...": "salida del target" } },
      { "index": 1, "input": { "user_id": 102, "plan": "pro" }, "status": "ok", "output": { "...": "..." } },
      { "index": 2, "input": { "user_id": 103 }, "status": "err", "error": "row 2: missing required param 'plan'" }
    ]
  }
}
```

- Una entrada por fila, **en orden original** — sin importar la
  concurrencia usada para ejecutarlas.
- `output` está presente solo si `status == "ok"`; `error` (string) solo
  si `status == "err"`.
- `output` es exactamente lo que devolvió el nodo target para esa fila
  (p.ej. `{ status, body }` para `http_request`, `{ result, extra_info }`
  para un `subgraph`/`llm_call`).

## Eventos de progreso (SSE)

`for_each` emite dos tipos de evento sobre el `ExecutionObserver`:

- **`batch-progress`** — snapshot agregado (`total`, `completed`, `ok`,
  `err`, `in_flight`). Se emite al arrancar (`completed: 0`) y al terminar
  (`completed == total`).
- **`batch-item-finished`** — uno por fila, con `index`, `key` (primer
  campo escalar de la fila, o `index=N` si no hay ninguno) y `status`
  (`"ok"` / `"err"`). Útil para UIs que quieren pintar una checklist de
  progreso fila por fila.

Cuando el `target` es un `subgraph`, los eventos internos del sub-agente
(`subgraph-node-start`, `agent-turn`, etc.) se propagan también, con
`level`/`path` incrementados (`agent>sub_agent`) — mismo mecanismo que
subgraph-as-tool normal.

## HITL fail-closed

Si el target se suspende dentro de una fila (un nodo `suspend`, o un
`subgraph` cuyo sub-agente pide input humano), esa fila se reporta como
**error** (`"row N: target suspended (HITL not supported inside
for_each)"`) — `for_each` **no soporta pausar el fan-out a mitad de
camino**. Diseña los targets de `for_each` para que no requieran
intervención humana por fila.

## Guard anti-recursión y `MAX_SUBGRAPH_TOOL_DEPTH`

- `target.node_type == "for_each"` se rechaza inmediatamente
  (`for_each: a for_each cannot target itself`) — no hay fan-out anidado
  de `for_each`.
- Cuando `target.node_type == "subgraph"`, cada fila corre un
  sub-agente aislado (Mode B — sin memoria compartida entre filas) y
  hereda el guard de profundidad normal de subgraph-as-tool
  (`MAX_SUBGRAPH_TOOL_DEPTH = 5`).

## Config vs tool — cómo `for_each` lee sus propios campos

`for_each` lee `target`/`items`/`items_from`/`on_error`/`concurrency`/
`max_items` con el patrón `cfg_or_input` (config-first, inputs-fallback,
el mismo que usa el nodo `suspend`):

- **Como nodo de grafo**, el motor pasa el `config` estático del nodo
  directamente — `for_each` lo lee de ahí.
- **Como tool de un `llm_call`**, `DagToolExecutor` pliega todo lo
  configurado/aportado por el LLM dentro de `inputs` y pasa
  `config = {}` — `for_each` cae al fallback de `inputs`, que es
  exactamente el comportamiento anterior a este mecanismo.

Esto permite usar el mismo nodo, sin cambios de código, tanto en un grafo
estático como colgado de un `tool_configurations`.

## Ejemplo — uso como nodo de grafo

`tests/graphs/basic/for_each_node.json`:

```json
{
  "nodes": {
    "start": { "type": "input", "config": { "prompt": "run the batch" } },
    "fe1": {
      "type": "for_each",
      "config": {
        "target": {
          "node_type": "add",
          "node_schema": {
            "a": { "type": "number", "required": true },
            "b": { "type": "number", "required": true }
          }
        },
        "items": [{ "a": 1, "b": 2 }, { "a": 10, "b": 20 }],
        "concurrency": 2,
        "on_error": "continue"
      }
    },
    "log_result": { "type": "log" }
  },
  "edges": [
    { "from": "start", "to": "fe1" },
    { "from": "fe1", "to": "log_result" }
  ]
}
```

```bash
cargo run --bin dag_engine -- run tests/graphs/basic/for_each_node.json
```

Resultado: `results.length == 2`, ambas `ok`, sumas `3` y `30`.

## Ejemplo — uso como tool LLM

`tests/graphs/agents/for_each_http_tool.json` expone `batch_update_users`
(`target: http_request`, endpoint eco) — el LLM arma `items` a partir de un
pedido natural ("Update the plan to pro for users 101, 102, 103") y
`for_each` hace 3 llamadas HTTP deterministas, una por usuario.

`tests/graphs/agents/for_each_subgraph_tool.json` expone
`batch_draft_messages` (`target: subgraph`, sub-agente inline) — cada fila
corre un `llm_call` aislado, probando que `for_each` compone con
subgraph-as-tool además de con nodos planos.

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/for_each_http_tool.json \
  --agent-session-id agent_foreach_001
```

Ambos grafos fueron verificados en vivo (Gemini 2.5 Flash) — 3/3 filas
`ok` en cada uno, con frames `batch-progress`/`batch-item-finished`
presentes en el SSE.

## Ver también

- [`docs/node_configurations.json`](../node_configurations.json) →
  `node_configurations.for_each` — schema canónico de config fields.
- [`docs/node_as_tools_reference.json`](../node_as_tools_reference.json) →
  `node_types_as_tools.for_each` — exposición como tool, ambos ejemplos.
- [`docs/agent_context/node_ports_reference.md`](../agent_context/node_ports_reference.md)
  — puertos default (`input` fallback / `output`).
- [`docs/superpowers/specs/2026-07-20-deterministic-list-tool-execution-design.md`](../superpowers/specs/2026-07-20-deterministic-list-tool-execution-design.md)
  — spec de diseño completo, incluyendo deferrals de v1.1
  (`items_from: attachment`, `items_from: tool_result`, checkpoint store
  durable, `results_to` sink).
