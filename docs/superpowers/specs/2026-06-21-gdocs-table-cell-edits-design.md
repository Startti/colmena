# Surgical table-cell edits (Subsystem G v1.1) — Design

- **Fecha:** 2026-06-21
- **Estado:** Aprobado, listo para plan de implementación
- **Backlog:** [Surgical table-cell edits](../../BACKLOG.md) (Subsystem G v1.1)
- **Subsistema:** G (Google Docs) — ver [`docs/developer_guide/45_gdocs.md`](../../developer_guide/45_gdocs.md)

## Problema

Hoy el agente **no puede ver ni editar tablas** dentro de un Google Doc.
[`parse_paragraphs`](../../../src/libs/colmena/src/gdocs/infrastructure/http_client.rs)
descarta explícitamente los elementos `table` del cuerpo del documento
(comentario "Tables / section breaks / TOC are NOT counted as paragraphs").
En consecuencia `ParagraphKind::TableRow` existe en el enum pero nunca se
emite, y `gdocs_read_outline` jamás muestra una tabla. Las tablas existen
físicamente en el doc (Drive las convierte nativamente desde markdown en
`gdocs_create_from_markdown`) pero son una caja negra para el agente: no las
puede inspeccionar ni modificar sin un round-trip manual fuera de banda.

Este diseño agrega **lectura + edición quirúrgica** de tablas: descubrir
tablas y sus celdas, escribir el texto de una celda, e insertar/borrar filas
y columnas.

## Decisiones de diseño

| Punto | Decisión | Razón |
|---|---|---|
| Operaciones v1 | `read_tables`, `set_table_cell`, `insert_table_row`, `delete_table_row`, `insert_table_column`, `delete_table_column` (6 tools) | CRUD completo de estructura; el read es obligatorio porque hoy las tablas son invisibles |
| Contenido de celda | **Solo texto plano** | Evita el problema de cursor-math de markdown→ops (índices post-tabla mal computados, ver backlog "Markdown tables en insert/replace"). Predecible y testeable |
| Direccionamiento | `table_index` 0-based (orden de aparición dentro del tab) + `row`/`col` 0-based; `read_tables` expone esos índices | Determinista y simple. El agente descubre con `read_tables` y luego direcciona |
| Co-edit guard | **Doc-level grueso** — reusa `apply_and_finalize`; cualquier cambio humano desde el cursor del agente se reporta como `pending_human_changes_outside_scope` (soft-warning), sin partición fina dentro/fuera de la tabla | Consistente con los otros 20 tools de edición. La partición fina exigiría modelar tablas en el diff de párrafos (fuera de scope v1) |
| Granularidad de escritura | **1 celda por llamada**, fetch fresco del snapshot cada vez | Editar una celda desplaza los índices UTF-16 siguientes; un batch multi-celda exigiría recalcular offsets dentro del mismo `batchUpdate` (frágil). El agente itera si necesita varias |

## Arquitectura

### 1. Dominio — nuevos tipos (additive, ADP no afectado)

En `src/libs/colmena/src/gdocs/domain/types.rs`:

```rust
/// Una tabla parseada del cuerpo del documento, con sus celdas y los
/// índices UTF-16 que la Docs API usa para direccionar contenido.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableSnapshot {
    /// 0-based, orden de aparición de la tabla dentro de su tab.
    pub table_index: u32,
    /// Tab donde vive la tabla. `None` en docs single-tab legacy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<TabId>,
    /// `startIndex` del elemento `table` (lo que la Docs API pide como
    /// `tableStartLocation.index` para insertTableRow/Column).
    pub start_index: u32,
    pub rows: u32,
    pub columns: u32,
    /// Filas × columnas. Una celda merged "esclava" aparece igual en su
    /// posición (row_span/col_span del master indican la fusión).
    pub cells: Vec<Vec<CellSnapshot>>,
}

/// Una celda individual con su texto y rango de contenido.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellSnapshot {
    pub row: u32,
    pub col: u32,
    /// Texto plano concatenado de los párrafos de la celda (trim del
    /// `\n` final).
    pub text: String,
    /// startIndex del primer párrafo de la celda (donde se inserta texto).
    pub content_start_index: u32,
    /// endIndex del último párrafo de la celda.
    pub content_end_index: u32,
    /// >1 si la celda es el master de un merge. 1 en celdas normales.
    pub row_span: u32,
    pub col_span: u32,
}
```

`TabSnapshot` gana un campo aditivo:

```rust
pub struct TabSnapshot {
    pub tab_id: Option<TabId>,
    pub paragraphs: Vec<ParagraphSnapshot>,
    #[serde(default)]
    pub tables: Vec<TableSnapshot>,   // ← nuevo
}
```

El campo es aditivo con `#[serde(default)]`: el co-edit guard, el outline y
todo consumidor existente de `DocumentSnapshot` lo ignoran sin cambios.

### 2. Infraestructura — parsing

Nueva función en `http_client.rs`:

```rust
fn parse_tables(
    content: &[serde_json::Value],
    tab_id: &Option<TabId>,
    table_counter: &mut u32,
) -> Vec<TableSnapshot>
```

Recorre **el mismo** array `content[]` que ya recorre `parse_paragraphs` (en
el mismo loop o en una segunda pasada), pero captura los elementos con clave
`table`. Para cada uno:

- `start_index` = `elem.startIndex`.
- `rows` = `table.rows`, `columns` = `table.columns`.
- Para cada `tableRows[r].tableCells[c]`:
  - `content_start_index` = `cell.startIndex`,
    `content_end_index` = `cell.endIndex`.
  - `text` = concatenación de `content[].paragraph.elements[].textRun.content`
    (mismo extractor que `paragraph_text_and_range`, trim del `\n` final).
  - `row_span`/`col_span` = `cell.tableCellStyle.rowSpan`/`columnSpan`
    (default 1).

`parse_snapshot` invoca `parse_tables` por cada body de tab (y por el body
legacy single-tab), poblando `TabSnapshot::tables`. El `table_counter` se
resetea **por tab** (los índices son relativos al tab, igual que el
direccionamiento expuesto).

**Sin cambios en el trait `DocsClient`.** Igual que `insert_image`, la capa de
aplicación construye los `Request` JSON y llama al `batch_update` genérico ya
existente. La lectura usa `get` (ya existe).

### 3. Aplicación — nuevo `application/table.rs`

Funciones (todas reciben el `GuardContext`/contexto de guard que usan los
demás edits, y finalizan vía el `apply_and_finalize` existente en
`insert.rs`, que aplica el guard doc-level + revision tracking + outline):

```rust
pub async fn run_read_tables(
    client: &dyn DocsClient,
    doc_id: &DocumentId,
    tab_id: Option<&TabId>,
) -> Result<TableListing, DocsError>;

pub async fn run_set_table_cell(
    ctx: &GuardContext<'_>, doc_id: &DocumentId,
    table_index: u32, row: u32, col: u32, text: &str,
    tab_id: Option<TabId>,
) -> Result<EditResult, DocsError>;

pub async fn run_insert_table_row(
    ctx, doc_id, table_index, at_row, insert_below, tab_id,
) -> Result<EditResult, DocsError>;
pub async fn run_delete_table_row(ctx, doc_id, table_index, row, tab_id) -> ...;
pub async fn run_insert_table_column(ctx, doc_id, table_index, at_col, insert_right, tab_id) -> ...;
pub async fn run_delete_table_column(ctx, doc_id, table_index, col, tab_id) -> ...;
```

`TableListing` es el shape de retorno de `read_tables`: lista de
`{ table_index, tab_id?, rows, columns, cells: [{ row, col, text_preview,
row_span, col_span }] }`.

#### Shapes de los `Request` (Docs API)

**set_table_cell** (reemplazo de texto plano):
1. Localiza la celda `(row, col)` en `TableSnapshot::cells`.
2. Si la celda tiene texto (`content_end_index - 1 > content_start_index`):
   emite `deleteContentRange` sobre `[content_start_index, content_end_index - 1)`.
   El `-1` **preserva el `\n` final obligatorio** de la celda (la Docs API
   rechaza borrar el último párrafo-mark de una celda). Si la celda está
   vacía, se omite el delete.
3. Emite `insertText` en `index = content_start_index` con el texto nuevo.
4. Orden en el batch: delete primero, insert después (índice de inserción
   sigue válido porque apunta al boundary inicial de la celda).

```json
[
  { "deleteContentRange": { "range": { "startIndex": S, "endIndex": E_menos_1, "tabId": "..." } } },
  { "insertText": { "location": { "index": S, "tabId": "..." }, "text": "nuevo texto" } }
]
```

**insert_table_row**:
```json
{ "insertTableRow": {
    "tableCellLocation": {
      "tableStartLocation": { "index": <start_index>, "tabId": "..." },
      "rowIndex": <at_row>, "columnIndex": 0 },
    "insertBelow": <bool> } }
```

**delete_table_row**:
```json
{ "deleteTableRow": {
    "tableCellLocation": {
      "tableStartLocation": { "index": <start_index>, "tabId": "..." },
      "rowIndex": <row>, "columnIndex": 0 } } }
```

**insert_table_column** / **delete_table_column**: análogos con
`insertTableColumn` (`insertRight: bool`) / `deleteTableColumn`, usando
`columnIndex` como ancla.

El `tabId` se incluye en cada `Location`/`range` solo cuando el snapshot
tiene `tab_id = Some(_)` (docs multi-tab); se omite en single-tab legacy.

### 4. Synthetic tools + toolkits

6 tools nuevos en
`src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`:

| Tool | Args | Read/Write |
|---|---|---|
| `gdocs_read_tables` | `doc_id`, `tab_id?` | read |
| `gdocs_set_table_cell` | `doc_id`, `table_index`, `row`, `col`, `text`, `tab_id?` | write |
| `gdocs_insert_table_row` | `doc_id`, `table_index`, `at_row`, `insert_below?` (default true), `tab_id?` | write |
| `gdocs_delete_table_row` | `doc_id`, `table_index`, `row`, `tab_id?` | write |
| `gdocs_insert_table_column` | `doc_id`, `table_index`, `at_col`, `insert_right?` (default true), `tab_id?` | write |
| `gdocs_delete_table_column` | `doc_id`, `table_index`, `col`, `tab_id?` | write |

- `gdocs_read_tables` se agrega también al toolkit read-only `gdocsread`.
- Los 6 se agregan a los arrays `all_gdocs` / `gdocs_entries` en `llm.rs`, y
  se actualiza el builder-count test.
- Descripciones LLM-facing en `src/libs/colmena/text/tools/gdocs.yaml`.

### 5. Casos borde

- **Índices fuera de rango** (`table_index` / `row` / `col`): `InvalidArgs`
  con las dimensiones reales en el mensaje (e.g. "table 2 has 3 rows × 4
  columns; row 5 out of range").
- **Celdas merged** (`row_span`/`col_span` > 1): `read_tables` las reporta
  con sus spans. `set_table_cell` sobre una posición cubierta por un merge
  (celda "esclava", no el master) → `InvalidArgs` con explicación. Insert/
  delete de fila/columna en una tabla con merges → la operación procede pero
  el `EditResult` incluye un soft-warning de que la Docs API puede reacomodar
  las celdas fusionadas.
- **Borrar la última fila o columna**: la Docs API rechaza dejar una tabla 0×N
  / N×0; el error se propaga con el mensaje de Google.
- **Celda vacía en set**: se omite el `deleteContentRange`, solo se inserta.
- **Multi-tab**: `tabId` en cada `Location`. Tablas direccionadas por
  `table_index` relativo al tab indicado (default: primer tab si `tab_id`
  ausente).

### 6. Scope `drive.file`

Todo el feature usa **Docs API** (`documents.get` + `documents.batchUpdate`,
scope `documents`) → funciona en **docs compartidos por el usuario**, igual
que `gdocs_insert_*` y `gdocs_replace_*`. No hay dependencia de Drive
export/list (que fallaría con `403 appNotAuthorizedToFile` en docs no
creados por la app). Ver la caja de scope en `45_gdocs.md`.

## Testing

- **Unit (parsing)**: `parse_tables` contra fixtures JSON — 1 tabla simple,
  multi-tabla en un tab, tabla en multi-tab, celdas merged (spans), celda
  vacía. Verifica `table_index`, dims, `content_start/end_index`, `text`.
- **Unit (request builders)**: cada `run_*` con `MockDocsClient` — verifica
  el shape exacto de los `Request` JSON emitidos (set_cell delete+insert con
  el `-1`; insert/delete row/col con `tableStartLocation` correcto;
  inclusión condicional de `tabId`).
- **Unit (validación)**: índices fuera de rango, celda merged esclava,
  celda vacía (skip delete).
- **Integration `#[ignore]` live** (`tests/gdocs_integration_test.rs`):
  crear doc desde markdown con una tabla (Drive convierte) → `read_tables`
  (verifica dims y celdas) → `set_table_cell` → `insert_table_row` →
  `delete_table_column` → re-leer y verificar el estado final.
- **E2E LLM-in-the-loop** (`tests/graphs/agents/gdocs_table_edits_e2e.json`):
  grafo donde el agente lee una tabla, llena una celda y agrega una fila
  sobre un doc operator-provided. IDs placeholder; ejecutado live por el
  controlador con secrets inyectados en memoria.

## No-objetivos (v1.1, fuera de scope)

- Contenido rico/markdown por celda (listas, multi-párrafo, estilo inline).
  Diferido — reabre el problema de cursor-math.
- Batch multi-celda en una sola llamada (`set_table_cells`).
- Partición fina del co-edit guard a nivel celda.
- Formateo de celda (background, borders, ancho de columna) — es el item
  "Cell formatting" separado del backlog (Sheets), no aplica a Docs en v1.
- Crear tablas nuevas desde cero vía tool dedicado (hoy se crean vía
  `gdocs_create_from_markdown`).

## Impacto cross-repo

**Ninguno.** Cambios puramente aditivos: nuevos tipos de dominio (campo
`tables` con `#[serde(default)]`), nuevo módulo de aplicación, 6 synthetic
tools nuevos. Sin cambios en la firma pública de `EngineConfig` /
`ColmenaEngine` / traits exportados. El worker de ADP no se ve afectado;
los tools se activan solo si el operador opta vía `enabled_tools`.
