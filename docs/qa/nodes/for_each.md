# QA — Nodo `for_each`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/for_each.rs`
Fuentes de doc revisadas:
- `docs/node_configurations.json` (líneas 511–584)
- `docs/node_as_tools_reference.json` (líneas 2209–2308+)
- `docs/agent_context/node_ports_reference.md` (tabla for_each)
- `docs/developer_guide/49_for_each.md` (guía completa)

---

## 1) Config documentada NO soportada por el código

**Sin discrepancias detectadas.** Todos los campos, valores, defaults, políticas y comportamientos fail-closed descritos en las 4 fuentes están correctamente implementados en el código:

- `target` (objeto embebido con `node_type` y `node_schema` opcional): validación en línea 277–286, rechazo self-target en línea 285–287.
- `items` (array inline): resolución y validación de tipo en línea 99–106.
- `items_from` (data-source handle): soporte `source: "sheet"` en línea 114–140, rechazo de fuentes desconocidas.
- `on_error` (continue/abort): parseo en línea 164–167, integración en `ExecPolicy`.
- `concurrency` (1 a 64): clamp en línea 168–172, techo defensivo `MAX_CONCURRENCY = 64` línea 24.
- `max_items` (default 1000): truncación con warning en línea 297–303.
- `results_to` (crear nueva sheet): creación línea 322–366, modos final/incremental línea 350–601.
- Validación de requeridos por fila (antes de despachar): línea 427–451.
- HITL fail-closed (suspend en fila = error): línea 476–481.
- Linaje por fila (`<node_id>#<index>`): línea 468–470.
- Contexto forwarded (`__colmena_subgraph_depth`, etc.): línea 33–37, 394–421.

---

## 2) Código NO documentado

**2 hallazgos menores** — ambos detalles de implementación interna, sin impacto en el contrato observable:

1. **Mecanismo `ChildScopeObserver::wrap()` no explícitamente nombrado** (línea 468–470): El código anida el observer por fila bajo `{node_id}#{index}` para aislar el stream. La documentación (developer_guide línea 275–302) describe el **efecto** (linaje separado por fila) pero no menciona el tipo de observer wrapper usado internamente. Esto es correcto — es un detalle interno; QA no necesita saberlo.

2. **Per-row `item_state` initialization** (línea 457): Cada fila se ejecuta con su propio `let mut item_state = json!({})` — esto no está documentado en ningún nivel. Es un detalle de implementación (cada fila es un ejecutable separado) y no afecta el contrato observable, pero hace que cada fila sea completamente aislada a nivel de estado, lo cual refuerza la promesa HITL fail-closed.

Estos dos hallazgos no requieren corrección de código ni documentación — son implementación interna coherente con la especificación.

---

## 3) Plan de pruebas QA

### Objetivo general
Verificar que el nodo `for_each` ejecuta un target embebido de forma determinista, una vez por fila, reportando ok/err por fila en la tabla de resultados, sin reexecutar o saltarse filas bajo concurrencia.

### Casos de prueba

#### **Caso 1: Inline `items` + target simple (happy path)**
- **Objetivo**: Verificar ejecución secuencial con array inline
- **Grafo mínimo** (`tests/graphs/basic/for_each_node.json` ya existe):
  ```json
  {
    "nodes": {
      "fe": {
        "type": "for_each",
        "config": {
          "target": {"node_type": "add", "node_schema": {"a": {"type":"number","required":true}, "b": {"type":"number","required":true}}},
          "items": [{"a": 1, "b": 2}, {"a": 10, "b": 20}]
        }
      }
    },
    "edges": []
  }
  ```
- **Entrada**: Sin interacción (node list)
- **Resultado esperado**: `output.total=2, output.ok=2, results[0].output={output: 3}, results[1].output={output: 30}`
- **Verificación**: Contra el JSON de output

#### **Caso 2: Como tool del LLM con `items` visible**
- **Objetivo**: Verificar configuración tool_configurations donde LLM aporta `items`
- **Grafo**: `tests/graphs/agents/for_each_http_tool.json` (ya existe)
- **Entrada**: Prompt como "Update the plan to pro for users 101, 102, 103"
- **Resultado esperado**: LLM genera `items = [{"user_id": 101, "plan": "pro"}, ...]`, `for_each` dispone 3 filas, todas `ok`
- **Verificación**: En la tabla de resultados, `results.length=3`, todas `status="ok"`

#### **Caso 3: `items_from: sheet` — lectura de Google Sheet**
- **Objetivo**: Verificar resolución de filas desde una sheet en lugar de array inline
- **Grafo**:
  ```json
  {
    "nodes": {
      "fe": {
        "type": "for_each",
        "config": {
          "target": {"node_type": "add", "node_schema": {"a": {"type":"number"}, "b": {"type":"number"}}},
          "items_from": {"source": "sheet", "ref": "<REAL_SHEET_ID>|Sheet1|A2:B5"}
        }
      }
    },
    "edges": []
  }
  ```
- **Entrada**: Una sheet con dos columnas (a, b) y 3 filas de datos
- **Resultado esperado**: `output.total=3` (si hay 3 filas en A2:B5)
- **Verificación**: Contador `total` coincide con filas leídas

#### **Caso 4: `items_from: sheet` con `column` y `as`**
- **Objetivo**: Verificar selección y renombramiento de columna única
- **Grafo**:
  ```json
  {
    "target": {"node_type": "http_request", ...},
    "items_from": {"source": "sheet", "ref": "<SHEET>|Sheet1", "column": "user_id", "as": "uid"}
  }
  ```
- **Entrada**: Sheet con columnas `user_id`, `name`, etc.
- **Resultado esperado**: Cada fila es `{"uid": <valor_original>}` (solo columna seleccionada, renombrada)
- **Verificación**: Inspeccionar `results[i].input` — debe ser `{"uid": ...}`, no el objeto completo

#### **Caso 5: `on_error: continue` (default)**
- **Objetivo**: Verificar que las filas fallan independientemente sin abortar el batch
- **Grafo**:
  ```json
  {
    "target": {"node_type": "add", "node_schema": {"a": {"type":"number","required":true}, "b": {"type":"number","required":true}}},
    "items": [{"a": 1, "b": 2}, {"a": 10}, {"a": 5, "b": 3}],
    "on_error": "continue"
  }
  ```
- **Entrada**: 3 filas, la segunda sin `b`
- **Resultado esperado**: `total=3, ok=2, err=1`, índice 1 tiene `status="err"`, indices 0 y 2 tienen `status="ok"`
- **Verificación**: 3 resultados en tabla, una con `status="err"`

#### **Caso 6: `on_error: abort`**
- **Objetivo**: Verificar que el batch se detiene cuando se encuentra el primer error
- **Grafo**: Igual a caso 5, pero `on_error: "abort"`
- **Entrada**: Ídem
- **Resultado esperado**: `total=3, ok=1, err=1, results.length <= 2` (no garantizado cuáles terminen bajo concurrencia, pero la propagación se queda).
- **Verificación**: Debe haber menos de 3 intentos de ejecución (inspeccionar logs o eventos SSE)

#### **Caso 7: `concurrency: 1` (secuencial)**
- **Objetivo**: Verificar ejecución ordenada, una fila por vez
- **Grafo**: `items: [...]` con 5 filas, `concurrency: 1`
- **Entrada**: 5 filas simple (p.ej. `add` con números)
- **Resultado esperado**: Orden original preservado en `results[]`; ninguna concurrencia visible en logs/SSE
- **Verificación**: `results[i].index == i` para todo i; eventos SSE muestran `in_flight=0` o `in_flight=1`

#### **Caso 8: `concurrency: 4`**
- **Objetivo**: Verificar fan-out paralelo correcto
- **Grafo**: `items: [...]` con 10 filas, `concurrency: 4`
- **Entrada**: 10 filas
- **Resultado esperado**: Orden original preservado; `batch-progress` reporta `in_flight` hasta 4 en paralelo
- **Verificación**: `results[]` mantiene orden original a pesar de concurrencia; eventos SSE muestran `in_flight >= 2` en algún momento

#### **Caso 9: `concurrency: 1000` (clamp a 64)**
- **Objetivo**: Verificar techo defensivo `MAX_CONCURRENCY = 64`
- **Grafo**: `concurrency: 1000`
- **Entrada**: 100 filas
- **Resultado esperado**: No crash; `batch-progress` reporta `in_flight <= 64`
- **Verificación**: Log muestra ningún warning sobre concurrencia (el clamp es silencioso); `in_flight` máximo observado es 64

#### **Caso 10: `max_items` truncation**
- **Objetivo**: Verificar truncación de lista oversized
- **Grafo**: `items: [...]` con 1500 elementos (excede default 1000)
- **Entrada**: 1500 filas
- **Resultado esperado**: Warning en log: `⚠️ [for_each] 1500 rows exceeds max_items=1000, truncating`; `output.total=1000`
- **Verificación**: Log contiene la advertencia; `total` final es 1000, no 1500

#### **Caso 11: Lista vacía**
- **Objetivo**: Verificar que empty list no es error
- **Grafo**: `items: []`
- **Entrada**: Array vacío
- **Resultado esperado**: `output.total=0, output.ok=0, output.err=0, results=[]`; sin error
- **Verificación**: Nodo retorna `Ok` (no `Err`); output es `{total:0, ok:0, err:0, results:[]}`

#### **Caso 12: `items` no es array (error tipado)**
- **Objetivo**: Verificar error si `items` es string u otro tipo
- **Grafo**: `items: "not-an-array"`
- **Entrada**: String en lugar de array
- **Resultado esperado**: Nodo falla con error: `for_each: 'items' must be an array of row objects, got string`
- **Verificación**: Error message contiene "must be an array" y el tipo

#### **Caso 13: Fila sin campo `required`**
- **Objetivo**: Verificar validación de requeridos antes de despacho
- **Grafo**: Target `add` con `node_schema` que requiere `a` y `b`; `items: [{"a": 1}]` (falta `b`)
- **Entrada**: 1 fila incompleta
- **Resultado esperado**: `total=1, ok=0, err=1, results[0].status="err"`, `error` contiene "missing required param 'b'"
- **Verificación**: Fila no se ejecuta (no genera output falso), error es claro

#### **Caso 14: Contenedor anidado con requerido**
- **Objetivo**: Verificar validación de requeridos dentro de contenedores (p.ej. `body.user_id`)
- **Grafo**: Target `http_request` con `node_schema.body.properties.user_id` required; `items: [{"path": "/users"}]` (falta `user_id` dentro de body)
- **Entrada**: 1 fila; solo `path` (no la estructura `body.user_id`)
- **Resultado esperado**: `results[0].status="err"`, error menciona `missing required param` (el validator ve dentro del contenedor)
- **Verificación**: Guard no da falso positivo (fila sin error spurio)

#### **Caso 15: Guard `target.node_type == "for_each"`**
- **Objetivo**: Verificar rechazo de self-targeting
- **Grafo**: `target: {node_type: "for_each", ...}`
- **Entrada**: Any
- **Resultado esperado**: Nodo falla con: `for_each: a for_each cannot target itself`
- **Verificación**: Error message exacto

#### **Caso 16: `results_to` con modo `final`**
- **Objetivo**: Verificar creación de sheet y escritura bulk al final
- **Grafo**: `items: [...]`, `results_to: {sink: "sheet", title: "test_final", mode: "final"}`
- **Entrada**: 3 filas
- **Resultado esperado**: Output contiene `results_sheet: {spreadsheet_id, url}`; nueva sheet creada; encabezado + 3 data rows (escritura una sola vez al final)
- **Verificación**: URL es accesible; sheet tiene el nombre "test_final"; filas aparecen todas juntas (no incrementalmente)

#### **Caso 17: `results_to` con modo `incremental`**
- **Objetivo**: Verificar escritura fila por fila en vivo
- **Grafo**: `items: [...]`, `results_to: {sink: "sheet", mode: "incremental"}`, `concurrency: 2`
- **Entrada**: 4 filas (slow targets para ver progreso)
- **Resultado esperado**: Sheet creada; primeras filas escritas en A2, A3, etc. conforme termina cada una (no al final)
- **Verificación**: Monitorear la sheet durante ejecución; filas aparecen incrementalmente, no todas al final

#### **Caso 18: HITL fail-closed (suspend en fila)**
- **Objetivo**: Verificar que un `suspend` dentro de una fila se reporta como error, no pausa el batch
- **Grafo**: Target es `suspend` o un subgraph que contiene un suspend; `items: [1, 2, 3]`
- **Entrada**: 3 filas
- **Resultado esperado**: Fila que dispara suspend → `status="err"`, `error` contiene "HITL not supported inside for_each"; otras filas continúan
- **Verificación**: Una fila es error, las demás `ok`; el batch **no** se pausa (si el target es subgraph con suspend, ese suspend no interactúa con el usuario)

#### **Caso 19: Contexto forwarded (`__colmena_subgraph_depth`)**
- **Objetivo**: Verificar propagación de contexto recursion/session a cada fila
- **Grafo**: Target es `subgraph` embebido; `for_each` con input que contiene `__colmena_subgraph_depth = 2`
- **Entrada**: 2 filas
- **Resultado esperado**: Cada fila del subgraph hereda `__colmena_subgraph_depth = 2` (no se resetea a 0)
- **Verificación**: Subgraph no falla por MAX_SUBGRAPH_TOOL_DEPTH erróneamente; el contador se propaga

#### **Caso 20: Eventos SSE `batch-progress` y `batch-item-finished`**
- **Objetivo**: Verificar emisión de eventos de progreso
- **Grafo**: `items: [3 filas]`, `concurrency: 2`
- **Entrada**: SSE subscriber activo
- **Resultado esperado**: 
  - `batch-progress` al inicio: `{total: 3, completed: 0, ok: 0, err: 0, in_flight: 0}`
  - `batch-item-finished` x3 (uno por fila): `{index, key, status}`
  - `batch-progress` al final: `{total: 3, completed: 3, ok: 3, err: 0, in_flight: 0}`
- **Verificación**: Eventos SSE presentes en stream; orden correcto; contadores coinciden con tabla final

---

## Resumen para QA

- **Código bien documentado**: Todas las características descritas en las 4 fuentes están correctamente implementadas.
- **Ambos modos funcionales**: graph node (config-driven) y LLM tool (node_schema-driven) comparten la misma lógica.
- **Validación fail-closed**: Campos requeridos por fila, self-targeting, suspend-in-row.
- **Eventos observable**: SSE batch-progress/batch-item-finished para UI.
- **Resultados a sheets**: Opcional, via `results_to` (final o incremental).
- **Pruebas clave**: concurrency clamping, max_items truncation, empty list, per-row lineage, contexto forwarded.
