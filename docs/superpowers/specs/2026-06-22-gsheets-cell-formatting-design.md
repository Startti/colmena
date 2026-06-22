# Google Sheets cell formatting (`gsheets_format_range`) — Design

- **Fecha:** 2026-06-22
- **Estado:** Aprobado, listo para plan de implementación
- **Backlog:** [Cell formatting](../../BACKLOG.md) (Subsystem E v1.1)
- **Subsistema:** E (Google Sheets) — ver [`docs/developer_guide/39_gsheets.md`](../../developer_guide/39_gsheets.md)

## Problema

Hoy el agente puede **escribir valores y fórmulas** en Google Sheets
(`gsheets_set_cell` / `gsheets_set_range` / `gsheets_run_python`, todos con
`valueInputOption=USER_ENTERED`) pero **no puede aplicar formato**: negrita,
colores, bordes, alineación, formato de número, anchos de columna. "Output
pulido" (resaltar headers, totales, bloques con bordes) es un pedido
frecuente y hoy requiere que el usuario lo haga a mano post-generación.

Todas las operaciones de formato viven en la API `spreadsheets.batchUpdate`
(distinta de `spreadsheets.values.*` que usa la escritura de valores).
Ninguna está cableada aún.

## Decisiones de diseño

| Punto | Decisión | Razón |
|---|---|---|
| Atributos v1 | **Todos**: texto (bold/italic/underline/strikethrough/size/font/color), `background_color`, `borders`, alineación H/V, `number_format`, `wrap`, `column_width_px`, `row_height_px` | El owner pidió cobertura completa; todos mapean a 3 tipos de request sobre el mismo plumbing |
| Forma de tool | **1 tool genérico** `gsheets_format_range` con `format` estructurado | Una sola superficie + schema rico; el dispatcher hace fan-out |
| Granularidad | **Lista de ops** `[{sheet, range, format}]` → **un solo `spreadsheets.batchUpdate` atómico** | "Output pulido" combina header+totales+bordes; 1 round-trip, atómico |
| Partial updates | `repeatCell` con **`fields` mask preciso** (solo subcampos presentes) | Setear `background` NO debe borrar el `bold` existente |
| Co-edit guard | **Ninguno** | El formato es no-destructivo (no toca valores) e idempotente; no aplica el collision policy de los writes |
| Fórmulas | **Fuera de scope** — ya funcionan (USER_ENTERED) y están documentadas | Las fórmulas son valores, no estilo; ortogonal |

## Arquitectura

### 1. Tool LLM-facing — `gsheets_format_range`

```jsonc
{
  "spreadsheet_id": "<id>",
  "ops": [
    {
      "sheet": "Hoja1",          // nombre de tab (o sheetId numérico)
      "range": "A1:D1",          // A1 notation, sin prefijo de sheet
      "format": {
        "text": {
          "bold": true, "italic": false, "underline": false,
          "strikethrough": false,
          "font_size": 11, "font_family": "Arial",
          "color": "#FFFFFF"     // hex; color del texto (foreground)
        },
        "background_color": "#4472C4",
        "horizontal_alignment": "CENTER",   // LEFT|CENTER|RIGHT
        "vertical_alignment": "MIDDLE",     // TOP|MIDDLE|BOTTOM
        "number_format": { "type": "CURRENCY", "pattern": "\"$\"#,##0.00" },
        "wrap": "WRAP",                     // OVERFLOW|CLIP|WRAP
        "borders": {
          "top":    { "style": "SOLID", "color": "#000000" },
          "bottom": { "style": "SOLID_THICK" },
          "left":   { "style": "SOLID" },
          "right":  { "style": "SOLID" },
          "inner_horizontal": { "style": "SOLID", "color": "#CCCCCC" },
          "inner_vertical":   { "style": "SOLID", "color": "#CCCCCC" }
        },
        "column_width_px": 120,   // aplica a las columnas que abarca `range`
        "row_height_px": 24       // aplica a las filas que abarca `range`
      }
    }
  ]
}
```

- Todos los sub-campos de `format` son **opcionales**; solo se emiten requests
  para los presentes.
- Colores: string hex `#RRGGBB` (LLM-friendly), convertido internamente a
  `RgbColor { red, green, blue }` floats 0.0–1.0.
- `number_format.type`: `NUMBER|CURRENCY|PERCENT|DATE|TIME|DATE_TIME|TEXT|SCIENTIFIC`;
  `pattern` opcional (si se omite, Google usa el patrón default del tipo).
- `borders.<side>.style`: `SOLID|SOLID_MEDIUM|SOLID_THICK|DASHED|DOTTED|DOUBLE`;
  `color` opcional (default negro).

Respuesta: `{ "ok": true, "ops_applied": <n>, "requests_sent": <m> }`.

### 2. Fan-out: `format` → requests de `spreadsheets.batchUpdate`

Por cada op se generan 1..N requests; todos los requests de todas las ops se
concatenan en **un solo** `batchUpdate`:

| Sub-campo(s) de `format` | Request | Nota |
|---|---|---|
| `text`, `background_color`, `horizontal/vertical_alignment`, `number_format`, `wrap` | **`repeatCell`** con `cell.userEnteredFormat` + `fields` mask preciso | Un solo repeatCell por op cubre todos estos |
| `borders` | **`updateBorders`** sobre el GridRange | Request separado |
| `column_width_px` | **`updateDimensionProperties`** (`dimension: COLUMNS`) | Rango de columnas derivado del A1 range |
| `row_height_px` | **`updateDimensionProperties`** (`dimension: ROWS`) | Rango de filas derivado del A1 range |

El `fields` mask del `repeatCell` se construye dinámicamente, e.g. si solo
viene `background_color` → `fields: "userEnteredFormat.backgroundColor"`; si
viene `text.bold` + `background_color` →
`fields: "userEnteredFormat(textFormat.bold,backgroundColor)"`. Esto garantiza
partial updates (no se pisan atributos no especificados).

### 3. Piezas nuevas

**Dominio** (`gsheets/domain/traits.rs`):
```rust
/// Apply N raw `spreadsheets.batchUpdate` requests in one round-trip.
/// Used for formatting (repeatCell / updateBorders / updateDimensionProperties).
/// Distinct from `batch_update_cells`, which is the values-only
/// `values.batchUpdate`.
async fn batch_update(
    &self,
    id: &SpreadsheetId,
    requests: Vec<serde_json::Value>,
) -> Result<(), SheetsError>;
```
Aditivo. Solo lo implementan el HTTP client de producción y el `MockSheetsClient`
de tests; **ADP no implementa `SheetsClient`** → sin breaking change.

**Aplicación** — nuevo módulo `gsheets/application/format.rs` (introduce la
capa `application/` en gsheets, espejo de `gdocs/application/`):
- `pub struct FormatSpec { ... }` + sub-structs (`TextFormat`, `Borders`,
  `BorderSide`, `NumberFormat`), todos `Deserialize + JsonSchema`, todos los
  campos `Option`.
- `build_format_requests(sheet_id: i64, grid: GridRange, spec: &FormatSpec) -> Vec<Value>`
  — **función pura, sin I/O**, unit-testeable. Mapea el spec a los requests
  de la tabla §2 con el `fields` mask correcto.
- Helpers puros: `a1_to_grid_range(range: &str) -> Result<GridRange, _>`
  (0-based, end-exclusive; "A1" → r[0,1) c[0,1); "A1:D1" → r[0,1) c[0,4);
  "B:B" columna entera → sin row bounds), y `hex_to_rgb(hex: &str) -> RgbColor`
  (acepta `#RRGGBB` y `RRGGBB`).
- `GridRange` interno = `{ sheet_id, start_row, end_row, start_col, end_col }`.

**Dispatcher** (synthetic tool):
1. Parsea args; valida `ops` no vacío.
2. Resuelve cada `sheet` (nombre o sheetId) → `sheet_id` numérico vía
   `list_sheets` (patrón `name_or_sheet_id` ya existente). Cachea el listado
   una vez por llamada.
3. Por op: `a1_to_grid_range(range)` + `build_format_requests(...)`.
4. Concatena todos los requests → `client.batch_update(id, requests)`.
5. Devuelve el envelope `{ ok, ops_applied, requests_sent }`.

### 4. Tool + wiring

`gsheets_format_range` se agrega al toolkit `gsheets` (**10 → 11 tools**).
Sync points (mismo patrón que los demás gsheets tools):
- `gsheets_tools.rs`: `TOOL_FORMAT_RANGE` const, `FormatRangeArgs`,
  `tool_format_range()`, `dispatch_format_range()`, agregar al builder list.
- `mod.rs`: re-export dispatcher + const.
- `toolkit_packages.rs`: agregar a `gsheets` (descripción 10→11) + actualizar
  el test de conteo (`gsheets_package_has_all_ten_tools` → eleven).
- `dag_tool_executor.rs`: router (import + match arm).
- `text/tools/gsheets.yaml`: descripción LLM-facing (estructura del `format`,
  ejemplos de header/totales/bordes, colores hex, que es no-destructivo).
- `docs/developer_guide/41_builtin_tools_index.md`: fila + pointer.

### 5. Casos borde / validación

- `ops` vacío → `InvalidArgs`.
- `sheet` inexistente → error claro con los nombres disponibles.
- A1 range inválido → `InvalidArgs` con el string ofensor.
- Hex inválido (no 6 hex digits) → `InvalidArgs`.
- `format` vacío (sin ningún sub-campo) para una op → se omite esa op (0
  requests) o `InvalidArgs` ("format has no attributes"); v1: **`InvalidArgs`**
  para fallar ruidoso en vez de no-op silencioso.
- `column_width_px`/`row_height_px` sobre un range de columna/fila entera
  ("B:B") → válido; sobre "A1:D1" aplica a cols A–D / fila 1.
- Enums inválidos (`horizontal_alignment: "MIDDLE"`) → `InvalidArgs` con los
  valores aceptados.

### 6. Testing

- **Unit (builder)**: cada sub-campo → request correcto + `fields` mask exacto;
  combinación (text+background → un repeatCell con mask compuesto); borders →
  `updateBorders`; column_width/row_height → `updateDimensionProperties`;
  multi-op → requests concatenados en orden.
- **Unit (helpers)**: `a1_to_grid_range` (single cell, range, columna entera,
  fila entera, multi-col, inválidos), `hex_to_rgb` (con/sin `#`, inválidos),
  fields-mask builder.
- **Unit (dispatcher)** con `MockSheetsClient`: resuelve sheet name→id, arma el
  batchUpdate, maneja sheet inexistente.
- **Integration `#[ignore]` live**: crear spreadsheet → escribir una tabla
  (header + filas) → `gsheets_format_range` con ops: header (bold + background
  + center), bloque de datos (bordes), columna (width) → assert `batchUpdate`
  200 (y opcionalmente re-leer formato vía `get` con `fields`).
- **E2E LLM-in-the-loop** (`tests/graphs/agents/gsheets_format_range_e2e.json`):
  agente crea un sheet, escribe una tablita, y formatea header + fila de
  totales en un turno. IDs placeholder; ejecutado live por el controlador.

### 7. No-objetivos (fuera de scope)

- **Fórmulas** — ya funcionan (USER_ENTERED) y están documentadas. (Ortogonal:
  son valores, no estilo.)
- **Conditional formatting** (`addConditionalFormatRule`), **charts**
  (`addChart`), **data validation/dropdowns** (`setDataValidation`),
  **merge cells** (`mergeCells`) → items parked/niche separados del backlog.
- **"Clear formatting" mode** → v1 setea formato; resetear queda fuera (el
  agente puede setear defaults explícitos si lo necesita).
- **Read formula+value combinado** → item de backlog aparte (Subsystem E v1.1).

## Impacto cross-repo

**Ninguno.** Aditivo: un método nuevo en el trait `SheetsClient` (solo lo
implementan el HTTP client + el mock; ADP no implementa el trait), un módulo
de aplicación nuevo, un synthetic tool nuevo. Sin cambios en la firma pública
de `EngineConfig` / `ColmenaEngine` / traits exportados → el worker de ADP no
se ve afectado; el tool se activa solo si el operador opta vía `enabled_tools`.
