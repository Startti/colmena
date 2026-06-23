# Formato de celdas de tabla en Google Docs — `gdocs_format_table` — Design

- **Fecha:** 2026-06-22
- **Estado:** Aprobado, listo para plan de implementación
- **Backlog:** "Formato de celdas de tabla en gdocs" (Subsystem G v1.1)
- **Subsistema:** G (Google Docs) — complementa `gdocs_set_table_cell` (§46, solo texto plano)
- **Hermano:** `gsheets_format_range` (§47) — mismo idioma multi-op, distinto target (tablas dentro de un Doc, no una hoja de cálculo)

## Problema

Tras §46 el agente puede leer tablas (`gdocs_read_tables`), escribir el **texto** de
una celda (`gdocs_set_table_cell`) e insertar/borrar filas y columnas — pero **no
puede dar formato** a las celdas. No hay forma de poner un header con fondo y
negrita, bordes en la tabla, o una fila de totales destacada dentro de un Doc.
`gsheets_format_range` (§47) resolvió esto para Google Sheets; falta el equivalente
para tablas en Docs.

Además, en el E2E del 2026-06-22 (§48) confirmamos que `gemini-2.5-flash`
**subutiliza** el formato con prompts abiertos a menos que la `description` del tool
lo empuje. Aplicamos esa lección acá con un nudge always-on (sin skill en v1).

## Decisiones de diseño

| Punto | Decisión | Razón |
|---|---|---|
| Direccionamiento | **Multi-op por rango** (`ops: [{table_index, cell_range, format}]`), espeja `gsheets_format_range` | El modelo ya aprendió el idioma multi-op con `gsheets-presentable-output`; una tabla presentable necesita ≥3 specs (header/data/totales) en **una llamada atómica** (una corrida de guard, un `batchUpdate`, un bump de revisión) |
| Índices del rango | 0-based, **end-EXCLUSIVE** (`row_start, row_end, col_start, col_end`), como `a1_to_grid_range` de gsheets; modelo de `gdocs_read_tables` | Consistencia con gsheets y con el read de tablas |
| Atributos v1 | **Esenciales presentables**: fondo de celda, estilo de texto (bold/italic/underline/strikethrough/font_size/color), alineación horizontal y vertical, bordes | Cubre "tabla presentable" completa. Ancho de columna y padding diferidos a v1.1 (request type extra + gotcha de auto-fit) |
| Bordes | `borders.{top,bottom,left,right}` aplicados a **cada celda** del rango | La Docs API **no** tiene inner_horizontal/inner_vertical (eso es de Sheets); cada celda tiene sus 4 bordes propios en `updateTableCellStyle` |
| Presentable-por-default | **Nudge always-on en la `description`** (prong 1 de gsheets), **sin skill** | El barato y de alto leverage; difiere la skill a v1.1 si el E2E muestra que el nudge no alcanza (YAGNI) |
| Archivo | **Nuevo `gdocs/application/table_format.rs`** | `table.rs` ya tiene 550 líneas; responsabilidad separada y testeable de forma aislada |
| Cambios de trait | **Ninguno** | `batch_update` genérico ya existe; reusa `CellSnapshot` (ya trae `content_start_index`/`content_end_index`) → sin tocar `DocsClient` ni el parser de #121 |

## Arquitectura

### Tool sintético `gdocs_format_table` (gdocs 35→36)

```jsonc
gdocs_format_table {
  "document_id": "string",
  "tab_id": "string?",                       // opcional; multi-tab
  "ops": [{
    "table_index": 0,                          // 0-based, como gdocs_read_tables
    "cell_range": { "row_start": 0, "row_end": 1, "col_start": 0, "col_end": 4 },  // 0-based, end-EXCLUSIVE
    "format": {
      "background_color": "#1F4E78",           // cell-level
      "vertical_alignment": "MIDDLE",          // TOP|MIDDLE|BOTTOM (cell-level)
      "borders": {                             // cell-level (cada lado opcional)
        "top":    { "style": "SOLID", "color": "#000000", "width_pt": 1 },
        "bottom": { "style": "SOLID" }
      },
      "text": {                                // text-level (por celda)
        "bold": true, "italic": false, "underline": false, "strikethrough": false,
        "font_size": 11, "color": "#FFFFFF"
      },
      "horizontal_alignment": "CENTER"         // LEFT|CENTER|RIGHT|JUSTIFIED (paragraph-level)
    }
  }]
}
```

Todos los campos de `format` son opcionales. Un op que solo trae `background_color`
solo emite un `updateTableCellStyle`; uno con `text.bold` emite `updateTextStyle`;
etc. Colores hex `#RRGGBB`.

### Use case `run_format_table` (en `table_format.rs`)

```
run_format_table(ctx, doc_id, ops, tab_id) -> Result<EditResult, DocsError>
```

1. `run_guard_non_blocking(ctx, doc_id)` **una vez** (no por op).
2. Por cada op: `find_table(snap, table_index, tab_id)` → `validate_range(table, cell_range)`
   (bounds con **aritmética checked**, end-exclusive, `row_start < row_end ≤ rows`, idem cols;
   error `InvalidArgs` claro si se sale o si el rango está vacío).
3. Acumula los requests de todos los ops (fan-out abajo).
4. **Un solo** `apply_and_finalize(... ChangeKind::Style ...)` → atómico.

### Builder `build_format_table_requests(table, cell_range, format, tab_id) -> Vec<Value>`

Fan-out por op (cada celda del rango se resuelve vía `find_cell`):

- **1× `updateTableCellStyle`** con `tableRange` (`tableCellLocation` de la celda
  superior-izquierda + `rowSpan`/`columnSpan` derivados del rango) → `backgroundColor`,
  `contentAlignment` (vertical), `borderTop/Bottom/Left/Right`, con `fields` mask
  PRECISO (updates parciales — no pisa atributos hermanos). Solo se emite si el op
  trae al menos un atributo cell-level.
- **N× `updateTextStyle`** (una por celda del rango, sobre
  `content_start_index..content_end_index` del `CellSnapshot`) → bold/italic/
  underline/strikethrough/fontSize/foregroundColor, con `fields` mask. Reusa el
  patrón de payload de `style.rs` (`rgb_to_color`, construcción text-map + fields).
- **N× `updateParagraphStyle`** (alineación horizontal, por celda, sobre el mismo
  rango de contenido) → `alignment` (LEFT/CENTER/RIGHT/JUSTIFIED), `fields:
  "alignment"`. Solo si el op trae `horizontal_alignment`.

Celdas merged: `CellSnapshot.row_span`/`col_span` ya disponibles; el builder respeta
el master de merge al resolver `find_cell` (mismo comportamiento que §46).

### Reusos (sin reescribir)

- `find_table` / `find_cell` / `cell_location` (de `table.rs` — hoy privados; se
  promueven a `pub(crate)`).
- `rgb_to_color` (de `style.rs`) para la shape `OptionalColor`/`rgbColor`.
- `run_guard_non_blocking`, `apply_and_finalize`, `ChangeKind::Style`,
  `GuardContext`, `EditResult` (de #121 / co_edit_guard).
- `CellSnapshot.content_start_index` / `content_end_index` (ya parseados en #121).

### Prong presentable (nudge)

Bloque "presentable por default" always-on en la `description` de `gdocs_format_table`
en `text/tools/gdocs.yaml`: cuando armes una tabla para mostrar, aplicá formato
presentable en una sola llamada multi-op (header con fondo + negrita + texto
contrastante + centrado, bordes en la tabla, fila de totales destacada), con un
ejemplo compacto de `ops`. **Sin skill** en v1.

### Co-edit guard

No-bloqueante (espeja table edits de #121): scope = celdas formateadas; cambios
humanos fuera de scope → `soft_warnings` (no bloquean); el formato es no-destructivo
del texto (no cambia el contenido, solo estilo).

## Testing

- **Unit** (`table_format.rs` tests): el builder emite los 3 tipos de request con
  `fields` masks correctos; `updateTableCellStyle` solo cuando hay atributo cell-level;
  `validate_range` rechaza out-of-bounds y rango vacío (checked arithmetic, sin panic);
  merged-cell respeta el master; un op con solo `text` no emite `updateTableCellStyle`.
- **Integration** (fake `DocsClient`): `gdocs_format_table` sobre un doc con tabla →
  verifica el body del `batchUpdate` (presencia y shape de los requests).
- **E2E live (criterio de éxito):** formatear una tabla real (header azul + bold +
  blanco + centrado, bordes, fila de totales gris) vía un prompt; read-back de
  `updateTableCellStyle`/`textStyle` vía Docs API `documents.get` confirma fondo,
  bordes, bold, alineación. Si con el nudge el modelo subutiliza, iterar el wording
  (parte del E2E, no un task nuevo). Limpiar el doc demo después.

## No-objetivos

- Ancho de columna y padding de celda (v1.1 — `updateTableColumnProperties` con
  gotcha de auto-fit; `updateTableCellStyle.paddingX`).
- Formato de celdas en gsheets (eso es §47, otro subsistema).
- `mode: "suggest"` (item separado de Subsystem G v1.1).
- Forzar formato hard-coded — el enfoque es guía (nudge); el modelo decide.

## Impacto cross-repo

Ninguno. Aditivo: nuevo `table_format.rs` + un tool + entrada en `gdocs.yaml` +
nudge + wiring. `batch_update` genérico ya existe → **sin cambio en el trait
`DocsClient`**; sin cambio en `EngineConfig`/`ColmenaEngine` ni firmas exportadas →
el worker de ADP no se ve afectado.

## Sync points (de #121/#122 — checklist para el plan)

- Exposición en `llm.rs` (`all_gdocs` / `gdocs_entries`, 35→36) — **el trap conocido**
  (tool ruteado pero no expuesto = invisible).
- Router + `mod.rs` del módulo de tools.
- Toolkit package count test (`gdocs` alias).
- `docs/developer_guide/41_builtin_tools_index.md` (+ coverage test
  `index_doc_covers_all_registered_tools`).
- `text/tools/gdocs.yaml` (description + nudge).
- Dev guide §45/§46.
- CHANGELOG §49.
- BACKLOG: marcar el item shipped.
