# Diseño — `update_by_position` agrega columnas nuevas

Fecha: 2026-06-26
Estado: aprobado, pendiente de implementación
Alcance: `gsheets_run_python` modo `update_by_position` (Google Sheets)

## Problema

El modo `update_by_position` de `gsheets_run_python` **no puede agregar una
columna que no existe ya en el header de la hoja**. Antes de hacer el diff,
el código proyecta el DataFrame devuelto **solo a las columnas presentes en el
header** (`comparable`), con el comentario explícito
*"projecting `new` to the comparable columns so a model-added column never trips
the diff's column-mismatch check"*
([`gsheets_run_python.rs:1077-1115`](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs)).

Resultado: si el modelo hace `df['Margen'] = '={{price}}-{{cost}}'` y devuelve
el df bajo `update_by_position`, la columna nueva se descarta **en silencio** —
la tool responde `{"changes":{"rows":0,"cells":0},"skipped_columns":[]}` sin
error ni señal.

### Contradicción con la documentación

El **ejemplo canónico documentado** muestra exactamente este caso como si
funcionara:

- Descripción de la tool: `text/tools/gsheets.yaml:161` →
  `df.loc[df['Categoria'] == 'Bebidas', 'Margen'] = '={{Venta}}-{{Costo}}'`
- Skill `edit-rows` (`skills/gsheets-editing/references/edit-rows.md:72`) → idem.

La doc promete que se puede rellenar una columna nueva con una fórmula vía
`update_by_position`, pero el código la descarta. Este diseño hace real lo que
la documentación ya promete.

### Verificación empírica (hoja real)

Reproducido contra una hoja `products` real (columnas
`product_id, sku, name, category, cost, price, stock`):

- Bug: `df['Margen']='={{price}}-{{cost}}'` + `update_by_position` →
  `{"mode":"update_by_position","changes":{"rows":0,"cells":0},"skipped_columns":[],"tab":"products"}`;
  el read final NO muestra `Margen`.
- Workaround actual (dos pasos): `gsheets_set_cell` `H1="Margen"` (crea el
  header) y *luego* `update_by_position` → escribe 1000 celdas
  (`H2:=F2-E2`, …). Confirma que con el header presente, el modo sí escribe.

## Comportamiento propuesto

Cuando el df devuelto contiene columnas que **no existen** en el header de la
hoja, en vez de descartarlas se **agregan**: se escribe el header en la primera
columna libre y se rellenan sus celdas. Automático (sin flag), additive, todo
en el **único `batch_update_cells` atómico** que ya hace el modo (junto con los
diffs de celdas existentes).

### Reglas de colocación y llenado

- **Detección.** Columna nueva = nombre de columna del df que **no está en
  `header_cols`** (los nombres crudos leídos de `1:1`). Si el nombre ya existe
  pero es no-direccionable (header duplicado), **no** se agrega: se mantiene el
  comportamiento actual (se reporta en `skipped_columns`). Esto evita crear una
  3.ª columna ante headers duplicados.
- **Índice.** Las columnas nuevas se anexan después de la última columna del
  header: primera nueva → índice `header_cols.len()`, siguiente →
  `+1`, … en el **orden de columnas del df**.
- **Filas.** Para cada registro `i` del df cuyo valor en esa columna **no es
  `null`**, se escribe en la fila `df_index[i] + 2` (fila 1 = header). Valores
  null/NaN se saltan.
- **Fórmulas.** Los `{{Nombre}}` se resuelven con el mapa de columnas
  direccionables existentes **+ las columnas nuevas en su índice asignado** (así
  una fórmula puede referenciar otra columna nueva). Un `{{Nombre}}` que no
  resuelve sigue devolviendo `FormulaUnknownColumn` y **aborta** la escritura
  (sin escrituras parciales), igual que hoy.
- **Expansión de grilla.** `batch_update_cells` usa
  `spreadsheets.values:batchUpdate` (`USER_ENTERED`), que auto-expande la grilla
  para acomodar la nueva columna. El header de la columna nueva va en el mismo
  batch, así que la columna se crea sola. (Se verifica en E2E.)

### Reporte (additive — no rompe el wire-format)

- Nuevo campo `added_columns`: lista de `{name, column}` (p.ej.
  `{"name":"Margen","column":"H"}`).
- Las celdas de columnas nuevas cuentan en `changes.cells` y aparecen en
  `changes.columns`.
- `formula_cells` incluye las fórmulas escritas en la columna nueva.
- `skipped_columns` mantiene su semántica actual (columnas del **snapshot** no
  direccionables). Las columnas nuevas NO van a `skipped_columns` — van a
  `added_columns`.

## Estructura del código

- Helper **puro** y unit-testeable:

  ```rust
  /// Para cada columna del df ausente del header, asigna el índice de columna
  /// siguiente y emite (header cell + celdas de cuerpo crudas). No resuelve
  /// fórmulas ni toca el cliente.
  fn plan_new_columns(
      header_cols: &[String],
      new_records: &[Map<String, Value>],
      df_index: &[Value],
  ) -> NewColumnPlan
  ```

  `NewColumnPlan { added: Vec<(String /*name*/, usize /*col_idx*/)>, cells: Vec<PlannedCell> }`
  donde `PlannedCell { col_idx, row_1based, raw: Value }`.

- `do_update_by_position`:
  1. (igual que hoy) calcula `comparable`/`skipped`, proyecta, hace el diff de
     celdas existentes → `cell_updates`.
  2. llama `plan_new_columns`, construye el mapa `resolvable` extendido
     (existentes + nuevas), resuelve fórmulas de las celdas planeadas con
     `resolve_formula_placeholders`, las convierte a `CellValue` y las mergea en
     `cell_updates` (incluye los header cells de fila 1).
  3. un único `batch_update_cells`.
  4. enriquece la respuesta con `added_columns` y suma al conteo.

La resolución de fórmulas y la conversión a `CellValue` quedan en el caller
(igual que el path existente), para que el helper sea puro.

## Tests

- **Unit (helper puro `plan_new_columns`):**
  - detecta una columna ausente del header y le asigna `header_cols.len()`
    (header cell en fila 1 + celdas de cuerpo).
  - múltiples columnas nuevas → índices consecutivos en orden de df.
  - nombre ya presente en header → no se trata como nueva (cero celdas).
  - valores `null` en el cuerpo se saltan.
  - `df_index` desordenado mapea la fila correcta (`idx + 2`).
- **Unit existentes** de `update_by_position` siguen verdes (cambio additive).
- **E2E real** contra hoja `products`: `df['Margen']='={{price}}-{{cost}}'` +
  `update_by_position` (sin `set_cell` previo) → la columna aparece con fórmulas
  en una sola llamada; `added_columns` reporta `{"name":"Margen","column":"H"}`.

## Docs

- `text/tools/gsheets.yaml` (descripción de `gsheets_run_python`): una línea
  aclarando que columnas del df ausentes del header se **anexan**, y documentar
  `added_columns` en el resultado.
- `skills/gsheets-editing/references/edit-rows.md`: ajustar la nota
  "Do NOT add rows with this mode" para aclarar que **filas** no se agregan pero
  **columnas nuevas sí** (anexadas), y que las fórmulas/valores se rellenan.

## No-objetivos (YAGNI)

- No se toca `update_in_place` (ya devuelve `Column mismatch` explícito; modo
  poco usado).
- No se agrega flag de opt-in: agregar columnas es el comportamiento esperado y
  documentado.
- No se agregan **filas** nuevas (sigue siendo edición de filas existentes).
- No hay cambio de API pública de colmena → ADP no afectado (solo cambia el
  wire-format del resultado de la tool, que es additive).
