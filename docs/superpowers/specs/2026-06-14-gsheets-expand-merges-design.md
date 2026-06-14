# gsheets: expand merges (forward-fill de celdas combinadas) — Design

- **Fecha:** 2026-06-14
- **Estado:** Aprobado, pendiente de plan de implementación
- **Autor:** brainstorm con daniel@startti.co
- **Alcance:** subsistema gsheets (`src/libs/colmena/src/gsheets/`) + las dos
  superficies LLM que leen sheets.

## 1. Problema

Cuando un Google Sheet tiene celdas combinadas (merged cells), la API
`spreadsheets.values.get` —que es lo que hoy usa
[`read_range`](../../../src/libs/colmena/src/gsheets/infrastructure/http_client.rs)—
devuelve el valor **solo en la celda top-left (ancla) del merge**; el resto del
rectángulo viene vacío/`null`.

Esto rompe cosas **en silencio**:

- Una columna "Categoría" con celdas combinadas (un valor que visualmente abarca
  5 filas) llega a pandas como 1 valor + 4 `NaN`. Un `groupby("Categoría")`, un
  `join`, o una comparación fila-a-fila dan resultados mal **sin que el agente se
  entere** de que había merges.
- Encabezados combinados se ven como huecos, confundiendo al modelo sobre la
  estructura de la tabla.

El agente no tiene forma de saber que hay merges, así que tampoco puede
"acordarse" de rellenarlos. La corrección tiene que ser automática.

## 2. Objetivo

Al leer un sheet, propagar (forward-fill) el valor del ancla de cada merge a
todo su rectángulo, de modo que cada celda combinada devuelva el valor que
*realmente* representa visualmente.

### Decisiones de diseño (cerradas en brainstorm)

| Decisión | Elección | Por qué |
|---|---|---|
| **Superficies cubiertas** | Ambas (`gsheets_read_range` + `gsheets_run_python`) vía helper compartido | El valor está en el path de pandas (merges rompen groupby/join), pero cubrir `read_range` con el mismo helper cuesta casi nada y evita inconsistencia "huecos acá, relleno allá". |
| **Activación** | **Always-on**, sin flag | Una celda combinada *realmente* contiene ese valor en todo su span; rellenarla **es** mostrar la verdad del sheet. No hay caso legítimo de "querer los huecos". |
| **Frescura** | Fresco en cada lectura, **sin cache** | Otros agentes/personas pueden cambiar la estructura de merges durante el run; un mapa cacheado quedaría stale y rellenaría mal (mismo principio de co-edit safety que en gdocs). |
| **Fetch** | **Approach B** — una sola llamada `spreadsheets.get` con `includeGridData` que trae valores **y** merges juntos | Fresco y sin round-trip extra. Encaja con "sin cache". Costo: reescritura acotada del parser de valores. |
| **Sub-rangos que cortan un merge** | **B1** — rellenar solo con anclas presentes en la grilla | El 95% de las lecturas son sheet completo o top-anchored (ancla siempre dentro). Para el borde raro (sub-rango que arranca a mitad de un merge vertical), la celda queda vacía = comportamiento actual, no empeora nada. B2 (traer el ancla faltante) agrega round-trips condicionales por un borde improbable. |

## 3. Arquitectura

El forward-fill vive **dentro de `read_range`** (capa de infraestructura de
gsheets). Como las dos superficies LLM llaman a `read_range`, **ambas heredan el
relleno sin lógica propia**. Cero cambios de merges en los archivos de
synthetic-tools.

```
gsheets_read_range  ─┐
                     ├──> SheetsClient::read_range ──> [fetch grid+merges] ──> [parse] ──> forward_fill_merges ──> [as_records / markdown ya existentes]
gsheets_run_python  ─┘                                  (Approach B, 1 call)                  (merge_fill.rs)
```

### Piezas

1. **`http_client.rs::read_range`** (modificado) — cambia el endpoint a Approach
   B y orquesta: fetch grid+merges → parse a `Vec<Vec<CellValue>>` + lista de
   `MergeRect` → `forward_fill_merges` → la lógica existente
   (`rectangle_to_records` cuando `as_records`, o raw) corre sobre la grilla
   **ya rellenada**, sin tocarse.

2. **`merge_fill.rs`** (módulo nuevo en `gsheets/infrastructure/`) — función pura
   `forward_fill_merges`. Sin I/O, unit-testeable en aislamiento.

3. **Texto LLM-facing** (`text/tools/` de gsheets) — las descripciones de
   `gsheets_read_range` y `gsheets_run_python` mencionan que las celdas
   combinadas se expanden automáticamente, para que el modelo entienda por qué
   ve valores repetidos y no asuma datos duplicados por error.

## 4. Cambio en `read_range` (Approach B)

Reemplaza `spreadsheets.values.get` por:

```
GET {sheets_base}/{id}                # mismo endpoint que ya usa list_sheets
    ?ranges=<sheet!range>             # repetible; si range=None, solo el nombre del sheet
    &includeGridData=true
    &fields=sheets(data(startRow,startColumn,rowData(values(formattedValue,effectiveValue,userEnteredValue))),merges)
```

Una sola llamada, fresca, trae **valores + merges juntos**. El `fields` está
restringido a lo mínimo necesario para acotar el payload (no traer formato,
notas, validaciones, etc.).

### Mapeo de `ValueRenderOption` → campo de `CellData`

| `ValueRenderOption` | Campo del grid | Notas |
|---|---|---|
| `FormattedValue` | `formattedValue` (string) | Lo que ve el usuario, locale-formatted. |
| `UnformattedValue` | `effectiveValue` → typed (`numberValue`/`stringValue`/`boolValue`/`null`) | Default para pandas. |
| `Formula` | `userEnteredValue.formulaValue` si existe; si no, el literal de `userEnteredValue` | Las celdas no-fórmula devuelven su valor literal. |

El parser produce exactamente los mismos tipos de `CellValue` que hoy genera el
path de `values.get`, para no regresionar el shape de salida (ver §6, riesgo
principal).

### Coordenadas

- El `GridData` trae `startRow`/`startColumn` (offset absoluto del bloque
  devuelto dentro del sheet; ausentes ⇒ `0`).
- `merges` viene en **coordenadas absolutas del sheet** (half-open:
  `startRowIndex`..`endRowIndex`, `startColumnIndex`..`endColumnIndex`).
- `read_range` resta el offset del `GridData` a las coords de cada merge antes de
  pasarlas a `forward_fill_merges`, para alinearlas con los índices de la grilla
  relativa.

## 5. `forward_fill_merges` — lógica pura

```rust
/// Rectángulo de merge en coords absolutas del sheet (half-open, convención API).
struct MergeRect {
    start_row: usize,
    end_row: usize,   // exclusivo
    start_col: usize,
    end_col: usize,   // exclusivo
}

/// Forward-fill de celdas combinadas.
///
/// Para cada merge cuya ancla (start_row, start_col), tras restar el offset
/// absoluto de la grilla, cae DENTRO de los límites de `grid`, copia el valor
/// del ancla a todas las celdas del rectángulo (clampeado a `grid`).
///
/// B1: los merges cuya ancla cae FUERA de la grilla (p.ej. un sub-rango que
/// arranca a mitad de un merge vertical) se saltean — esas celdas quedan como
/// estaban (vacías), igual que el comportamiento actual.
fn forward_fill_merges(
    grid: &mut Vec<Vec<CellValue>>,
    merges: &[MergeRect],
    row_offset: usize,
    col_offset: usize,
)
```

### Casos cubiertos

- **Merge horizontal** (1 fila, N columnas) → fill a la derecha.
- **Merge vertical** (N filas, 1 columna) → fill hacia abajo.
- **Merge bloque** (N×M) → fill en ambas direcciones.
- **Ancla fuera de la grilla** (B1) → skip, celdas quedan vacías.
- **Sin merges** → no-op; salida idéntica al comportamiento previo.
- **Ancla vacía/null** → fill con vacío (no-op efectivo).
- **Clamp a bordes** → un merge que se extiende más allá de la grilla devuelta se
  rellena solo hasta el límite de la grilla (sin panic / out-of-bounds).

## 6. Riesgos y mitigaciones

| Riesgo | Mitigación |
|---|---|
| **Regresión del path bien testeado.** Cambiar `values.get` → `spreadsheets.get` podría alterar tipos/shape de `CellValue` para sheets sin merges. | Tests que comparan el parse de grid-data celda por celda contra el comportamiento previo. Para sheets sin merges el fill es no-op y la salida debe ser idéntica. |
| **Payload más pesado.** `includeGridData` trae más metadata por celda que `values.get`. | `fields` restringido a los 3 campos de valor + `merges`. Aceptable dado el requisito de frescura (sin cache). |
| **Round-trip por lectura.** Sin cache, cada lectura pega a la API. | Es Approach B: un solo round-trip (igual cantidad que hoy, no más). El cache se descartó por correctness; el costo es 1 GET, igual que antes. |

## 7. Impacto en ADP / compatibilidad

- **No hay break de API Rust.** La firma de `SheetsClient::read_range` y de
  `ReadOptions`/`ReadResponse` queda intacta. Cambia solo la implementación
  interna del adapter HTTP.
- **El output observable cambia:** celdas antes vacías ahora traen el valor del
  merge. Esto solo afecta lo que el LLM ve en el resultado de la tool —
  intencional, es el objetivo de la feature. No cruza ningún borde de wire-format
  hacia ADP.
- Nota para ADP: sin cambio de código requerido.

## 8. Testing

- **Unit `merge_fill`**: horizontal, vertical, bloque, ancla-fuera-de-grilla
  (B1), no-op sin merges, clamp a bordes, ancla vacía.
- **Unit parse grid-data**: las 3 `ValueRenderOption` sobre un JSON mock de
  `spreadsheets.get`; aserción de que sin merges el output iguala al de
  `values.get`.
- **E2E real** (regla del repo — verificar contra Google real antes de cerrar):
  un sheet con encabezados y categorías combinadas →
  - `gsheets_read_range` muestra el relleno en markdown/json.
  - `gsheets_run_python` hace un `groupby` sobre la columna antes-combinada y da
    el resultado correcto (no `NaN`).
  - SSE guardado en `/tmp/colmena_e2e/<name>.sse` + reporte amigable.

## 9. Fuera de alcance (YAGNI)

- **B2** (traer anclas fuera del sub-rango) — borde improbable, se documenta la
  limitación de B1.
- Flag de opt-out — descartado: no hay caso legítimo de querer los huecos.
- Cache de merges — descartado por correctness bajo co-edición.
- Expandir merges en **escritura** (`set_range`/`update_in_place`) — esta feature
  es solo de lectura.
