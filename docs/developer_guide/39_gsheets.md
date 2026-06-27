# 39. Google Sheets integration (Subsystem E)

> v1.1 ships 11 synthetic LLM tools mirroring `crdt_doc_*` shape — agents
> read, write, create, and analyse Google Sheets via the Sheets API v4 +
> Drive API. Auth via Service Account JSON or Application Default
> Credentials. No OAuth user-scoped flow in v1.

## Recommended activation

Enable the whole gsheets surface with one alias:

```json
"enabled_tools": ["gsheets"]
```

This expands to all 11 gsheets tools via the toolkit-packages registry.
For a read-only-style agent, exclude write tools:

```json
"enabled_tools": [
  "gsheets",
  "!gsheets_delete_sheet",
  "!gsheets_add_sheet",
  "!gsheets_create_spreadsheet",
  "!gsheets_create_from_xlsx"
]
```

See [40_toolkit_packages.md](40_toolkit_packages.md) for the full syntax and exclusion semantics.

## Tool surface

| Tool | What it does |
|---|---|
| `gsheets_create_spreadsheet` | Create a new empty spreadsheet. Returns `{spreadsheet_id, url}`. |
| `gsheets_create_from_xlsx` | Upload an `.xlsx` attachment as a native Google Sheet (auto-conversion). **Deferred to E-T7b** (attachment plumbing pending); tool definition is published but calls error at runtime. |
| `gsheets_export_xlsx` | Download a Google Sheet as `.xlsx`, register as attachment. **Deferred to E-T7b** for the same reason. |
| `gsheets_list_sheets` | List tabs in a spreadsheet. |
| `gsheets_add_sheet` | Add a new tab. |
| `gsheets_delete_sheet` | Delete a tab by title or numeric id. |
| `gsheets_read` | Read cells; `value_render` controls formula vs evaluated; `as_records` controls 2D-array vs records shape. |
| `gsheets_run_python` | **Preferred for analysis.** Run sandboxed pandas/numpy/scipy code against one or more sheet ranges loaded server-side — rows NEVER pass through the LLM context. See section below. |
| `gsheets_set_cell` | Write one cell. Strings starting with `=` are evaluated by Google server-side. |
| `gsheets_set_range` | Bulk-write a rectangular block. Same formula semantics. |
| `gsheets_format_range` | Apply cell formatting (text style, background, borders, alignment, number format, column width, row height) to one or more ranges in a single atomic `batchUpdate`. **Non-destructive** — never touches values/formulas. See "Cell formatting" below. |

UX aliases (per D-T16 lessons): `address` ↔ `addr`, `start` ↔ `start_addr`,
`values` ↔ `values_2d`, `name` ↔ `sheet`. Single-A1 ranges
auto-expanded (`"C1"` → `"C1:C1"`).

## Auth

Two paths, no app config required in colmena itself:

1. **Service Account JSON** — set `GOOGLE_APPLICATION_CREDENTIALS=/path/to/sa.json`.
   The SA email must be shared (Edit access) on each spreadsheet the
   agent touches. Best for unattended/automation use.
2. **Application Default Credentials** — when the env var is unset,
   `yup-oauth2` falls back to ADC: GCP metadata server in cloud
   environments, or `gcloud auth application-default login` for local
   dev.

Scopes: defaults to `spreadsheets` + `drive.file`. Override via
`COLMENA_GSHEETS_SCOPES=<comma-sep>` (short names or full URLs).

## Formulas — Google evaluates them

Unlike subsystem D (where colmena's `formula_engine` evaluates
spreadsheet formulas), Google Sheets evaluates `=...` formulas
server-side. Write a string starting with `=` to `gsheets_set_cell` /
`gsheets_set_range` (or via `gsheets_run_python`'s `output_sheets`) and
Google parses, evaluates, and cascades it. Read back via
`gsheets_read(..., value_render="UNFORMATTED_VALUE")` to get the computed
number, or `value_render="FORMULA"` to get the text.

### Column-name placeholders `{{Column}}` (run_python, shipped 2026-06-26)

Inside a `gsheets_run_python` formula string you reference a column by
**name** in double braces instead of a hand-computed A1 letter:
`df.loc[mask, 'Importe'] = '={{Cantidad}}*{{Tarifa}}'`. At write time the
dispatcher substitutes the real A1 ref for the **same row** from the
sheet's positional header (`=S5*U5`), so the model never computes column
letters — which it gets wrong whenever an empty/duplicate header column
shifts the positions (it lands `=R5*T5` → silent `#VALUE!`). Each
`{{Name}}` resolves per row, so assigning to a whole column / a condition
(`df.loc[df['X']=='Y', …]`, preferred) / a range fills the formula down
with each row's own refs. Works in **every** write mode (`replace`/create,
`overwrite`, `update_by_position`, `update_in_place`).

- Unknown / misspelled column → structured `FormulaUnknownColumn` error
  (lists the valid names), aborts before any write (no partial writes).
- The result echoes `formula_cells` (real cell → resolved formula), sampled
  at 50 with `formula_cells_total` + `formula_cells_truncated` past that, so
  the model reports what it actually wrote instead of recomputing A1.
- MVP is **current-row** refs only — an aggregate like `=SUM(...)` still
  needs a literal A1 range.

## Cell formatting: `gsheets_format_range` (E v1.1)

`gsheets_format_range` applies **presentation** to cells — text style,
colors, borders, alignment, number format, column width, row height — to
one or more ranges in a single **atomic** `spreadsheets.batchUpdate`. It
is **separate from value writes**: it never touches cell values or
formulas. To write values/formulas use `gsheets_set_cell` /
`gsheets_set_range` (USER_ENTERED); to *style* what's already there use
this tool.

### Shape — `ops` list

The tool takes an `ops` array; each op targets one range with one
`format` block. All ops fan out into a single atomic `batchUpdate`
(internally to `repeatCell` / `updateBorders` / `updateDimensionProperties`):

```json
{
  "spreadsheet_id": "<id>",
  "ops": [
    {
      "sheet": "Ventas",
      "range": "A1:D1",
      "format": { ... }
    }
  ]
}
```

- **`sheet`** — tab name (string) or numeric `sheetId`.
- **`range`** — A1 notation (e.g. `"A1:D1"`, `"B:B"` for a whole
  column). Addressing is 0-based internally.
- **`format`** — all fields optional; only the ones you set are applied.

### `format` fields (all optional)

| Field | Values |
|---|---|
| `text` | `{ bold, italic, underline, strikethrough, font_size, font_family, color }` — `color` is hex `#RRGGBB`. |
| `background_color` | hex `#RRGGBB` |
| `horizontal_alignment` | `LEFT` \| `CENTER` \| `RIGHT` |
| `vertical_alignment` | `TOP` \| `MIDDLE` \| `BOTTOM` |
| `number_format` | `{ type, pattern? }` (e.g. currency / percent / date; optional explicit `pattern`) |
| `wrap` | `OVERFLOW` \| `CLIP` \| `WRAP` |
| `borders` | `{ top, bottom, left, right, inner_horizontal, inner_vertical }`, each `{ style, color? }` (`color` hex `#RRGGBB`) |
| `column_width_px` | integer pixels (applies to the range's columns) |
| `row_height_px` | integer pixels (applies to the range's rows) |

Colors everywhere are hex `#RRGGBB`.

### Non-destructive: precise `fields` mask

Each op is sent with a tight `fields` mask, so setting one attribute
(e.g. just `background_color`) does **not** wipe sibling attributes
(bold, alignment, etc.) already on those cells. There is **no co-edit
guard** — formatting is idempotent and safe to re-apply.

### Example — bold white header on a blue background, centered

```json
{
  "spreadsheet_id": "<id>",
  "ops": [
    {
      "sheet": "Ventas",
      "range": "A1:D1",
      "format": {
        "text": { "bold": true, "color": "#FFFFFF" },
        "background_color": "#1155CC",
        "horizontal_alignment": "CENTER"
      }
    }
  ]
}
```

Source:
`src/libs/colmena/src/gsheets/application/format.rs` (A1→GridRange +
hex→RgbColor helpers) and the `gsheets_format_range` dispatcher.

### Formato presentable por default

El formato rico se fomenta **por default**, no como un extra opcional:

- La `description` de `gsheets_format_range` lleva un nudge always-on que
  empuja al modelo a entregar hojas presentables (moneda, bordes, fila de
  totales) aun con prompts abiertos — mueve el default sin depender de la
  discrecionalidad del modelo.
- La skill built-in **`gsheets-presentable-output`** se auto-enrola en el
  catálogo de carga bajo demanda del agente cada vez que
  `gsheets_format_range` está en su catálogo (gate
  `agent_has_gsheets_format_tool`, espeja el patrón de
  `gdocs-surgical-edits`; honra la exclusión `!gsheets_format_range`).
- El modelo puede `load_skill("gsheets-presentable-output")` para la receta
  completa: paletas, number formats, un template multi-op completo y reglas
  de layout (5 references).

## Pandas analysis flow

Same shape as `crdt_doc_*` analysis (subsystem F). Skill
`gsheets-cross-sheet-analysis` documents 6 patterns
(`pattern-a-cell-diff` through `pattern-f-conditional-transform`).

## Lectura: tabla markdown por defecto + dimensiones

`gsheets_read` devuelve una **tabla markdown** por defecto (ideal para
mostrar el contenido al usuario o inspeccionarlo visualmente). Omitir
`range` lee la hoja entera (el área usada); pasar un `range` en notación
A1 limita la lectura a ese subconjunto.

La respuesta incluye siempre el campo `dimensions {rows, columns}` con la
extensión real de los datos devueltos (máximo ancho de fila, incluyendo la
cabecera). Esto permite consultar primero sin `range` para conocer el tamaño
y luego leer un subconjunto preciso si la hoja es grande.

```json
{
  "spreadsheet_id": "<id>",
  "sheet": "Ventas"
}
```

Respuesta típica:

```json
{
  "ok": true,
  "sheet": "Ventas",
  "range": "A1:D201",
  "dimensions": { "rows": 201, "columns": 4 },
  "markdown": "| producto | qty | precio | total |\n| --- | --- | --- | --- |\n| ..."
}
```

Para obtener los datos en formato estructurado (p.ej. para enviarlos como
argumento a otra herramienta) pasa `format: "json"` — la respuesta incluye
el array `values` (respetando `as_records`) en lugar del campo `markdown`:

```json
{
  "spreadsheet_id": "<id>",
  "sheet": "Ventas",
  "format": "json",
  "as_records": true
}
```

> **Regla:** para *comparar* dos tablas no leas markdown y coteja
> visualmente — usa `gsheets_run_python` (código determinista). La sección
> siguiente explica cómo.

## Comparación de tablas (código, no cotejo visual)

Cuando necesites comparar, cruzar o deduplicar tablas usa
`gsheets_run_python`. El modelo escribe código pandas que corre en el
servidor — los datos nunca pasan por el contexto del LLM.

Un `binding` tiene dos formas posibles:

| Forma | Campos requeridos | Cuándo usarla |
|---|---|---|
| **SHEET** | `var`, `spreadsheet_id`, `sheet`, `range?` | La tabla vive en Google Sheets |
| **INLINE** | `var`, `data: [...]` | La tabla la produjo el modelo (p.ej. extraída de una imagen o construida en texto). `data` es un array de objetos `{col: val}` o un array 2-D con cabecera en la primera fila. |

Dentro del sandbox Python cada binding queda disponible como variable
`<var>` (lista de dicts). Construye un DataFrame con `pd.DataFrame(<var>)`.

### Receta A — imagen vs hoja (inline + sheet binding)

La tabla extraída de la imagen se pasa como `data` inline; la hoja de
referencia se carga como binding de hoja. Se compara con tolerancia
numérica para evitar errores de redondeo:

```json
{
  "bindings": [
    {
      "var": "img_table",
      "data": [
        {"nutriente": "Proteínas", "g": 12},
        {"nutriente": "Grasas",    "g": 3},
        {"nutriente": "Carbohid.", "g": 40}
      ]
    },
    {
      "var": "ref_table",
      "spreadsheet_id": "<id>",
      "sheet": "Nutricional"
    }
  ],
  "code": "import pandas as pd\ndf_img = pd.DataFrame(img_table)\ndf_ref = pd.DataFrame(ref_table)\nmerged = df_img.merge(df_ref, on='nutriente', suffixes=('_img','_ref'))\nmerged['diff'] = (merged['g_img'] - merged['g_ref']).abs()\noutput = merged[merged['diff'] > 0.5][['nutriente','g_img','g_ref','diff']].to_dict('records')"
}
```

### Receta B — hoja vs hoja (dos sheet bindings)

```json
{
  "bindings": [
    {"var": "hoja_a", "spreadsheet_id": "<id>", "sheet": "Datos_A"},
    {"var": "hoja_b", "spreadsheet_id": "<id>", "sheet": "Datos_B"}
  ],
  "code": "import pandas as pd\ndf_a = pd.DataFrame(hoja_a)\ndf_b = pd.DataFrame(hoja_b)\ndiff = df_a.merge(df_b, on='sku', how='outer', indicator=True)\noutput = diff[diff['_merge'] != 'both'][['sku','_merge']].to_dict('records')"
}
```

Para diferencias grandes, escribe las filas en una hoja de resultados con
`output_sheets` (las filas nunca vuelven por el modelo):

```python
output_sheets = {"Diferencias": pd.DataFrame(diff)}
output = {"total_diff": len(diff)}
```

## Bulk analysis without LLM cost: `gsheets_run_python` (E-T14)

`gsheets_read` is fine for inspection (< 50 rows) but burns context
tokens at scale — 5000 rows ≈ 150k tokens just to feed pandas. The
`gsheets_run_python` tool reverses the flow: the LLM describes the
analysis as Python code, the dispatcher fetches every binding
**in parallel** server-side, runs the code in the existing sandbox
(same `execute_sandboxed_helper` used by `crdt_doc_run_python` and
`python_script`), and returns only `output` to the LLM.

```json
{
  "bindings": [
    {"var": "products", "spreadsheet_id": "<id>", "sheet": "Products"},
    {"var": "sales",    "spreadsheet_id": "<id>", "sheet": "Sales", "range": "A1:H5001"}
  ],
  "code": "import pandas as pd\nprods = pd.DataFrame(products)\nsold  = pd.DataFrame(sales)\nmerged = prods.merge(sold, on='sku', how='left')\noutput = merged.nlargest(5, 'qty')[['sku','name','qty']].to_dict('records')"
}
```

Mirrors `crdt_doc_run_python` (subsystem C, §5.6) — same prelude,
postlude, output / stdout / error caps (10 KB each), and 30-second
timeout. Differences vs the CRDT cousin:

- Each binding's records list is bound directly under the user-chosen
  `var` (the LLM calls `pd.DataFrame(<var>)` itself). The CRDT tool
  auto-builds `dfs[<sheet_id>]` because there's no per-binding name.
- Errors carry `loaded_columns: {<var>: [...]}` so the LLM can fix a
  `KeyError` without re-fetching.

Write-back is via `output_sheets = {name: DataFrame | spec_dict}` (see
next section).

Source: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`.

## Write safety: collision policy + `update_in_place` (shipped 2026-06-07)

`output_sheets` accepts a dict whose values are either a bare DataFrame
(mode = `replace`, default) or a spec dict with one of four modes:

| Mode | When to use | What gets written |
|---|---|---|
| `replace` (default) | Create a brand-new tab | Full DataFrame; collision policy applies if tab exists |
| `update_in_place` | Patch existing rows by a UNIQUE key column | Cell-level diff via single `batchUpdate` — only changed cells |
| `update_by_position` | Edit existing rows with NO unique key and NO A1 math; also **append new columns** | Whole-sheet bind, modify df in place, return it whole; cell-level diff by row **index** via single `batchUpdate`; df columns absent from the header are appended (header + values) and reported in `added_columns` |
| `overwrite` | Replace an existing tab entirely | Full DataFrame; schema-change guard rejects unless `allow_schema_change: true` |

### Example — `update_in_place` (the one that saves cells)

```python
import pandas as pd
sales = pd.DataFrame(sales_records)
mask = sales['category'] == 'Electronics'
sales.loc[mask, 'price'] = sales.loc[mask, 'price'] * 0.9  # 10% discount

output_sheets = {
    'Sales': {
        'mode': 'update_in_place',
        'df': sales,
        'key': 'product_id',
        'columns': ['price'],   # optional — only patch this column
    }
}
```

For 47 changed rows in a 1000-row sheet, this issues **one** HTTPS
round-trip with 47 cell updates, leaving 953 rows + 11 columns untouched.

### Example — `update_by_position` (edit rows, no key, no A1 math, shipped 2026-06-26)

When you must edit existing rows but no column is a unique key — or the
LLM keeps miscomputing A1 addresses — bind the **whole sheet** (no
`range`), modify the bound df **in place**, and return the **whole** df:

```python
df = pd.DataFrame(sheet_data)            # whole-sheet binding, no range
df.loc[df['CLIENT ID'] == 'TCI28fa...', 'Importe'] = df['Cantidad'] * df['Tarifa']
output_sheets = {'Hoja 16': {'mode': 'update_by_position', 'df': df}}
```

The dispatcher diffs the returned df against the load snapshot by **row
index** and writes only the changed cells to the right rows/columns — **no
agent-computed A1 address, no unique key** (a repeated id is fine). Pairs
naturally with the `{{Column}}` formula placeholders above. Safety
contract: the returned df index must be exactly `{0..N-1}` (the WHOLE bound
df, modified in place); a filtered subset / `reset_index` /
`sort+reset_index` / `concat` is rejected with a clear error, because it
would silently map rows to the wrong sheet positions. Columns whose header
name is empty or duplicated can't be addressed and are reported in
`skipped_columns`.

**Adding a new column (shipped 2026-06-26, PR #130).** Assigning a column
that does NOT exist in the sheet header appends it after the last column —
the header cell and the values are written in the **same atomic
`batchUpdate`**, and the result reports `added_columns: [{name, column}]`
(e.g. `{"name": "Margen", "column": "H"}`). Formulas resolve `{{Name}}`
against existing **and** newly-added columns, so a new column can reference
another:

```python
df = pd.DataFrame(products)                       # whole-sheet binding
df['Margen'] = '={{price}}-{{cost}}'             # column NOT in the header → appended
output_sheets = {'products': {'mode': 'update_by_position', 'df': df}}
# → changes.columns includes "Margen"; added_columns: [{"name":"Margen","column":"H"}]
```

A new column whose values are entirely null is ignored (no orphan header).
New **rows** are still not added — this mode edits/extends existing rows
only. (Previously such a column was silently dropped with `cells: 0`.)

### Collision policy

When `replace` mode targets an existing tab, the operator-supplied
`on_existing_sheet` setting (wired via `fixed_config.on_existing_sheet`,
default `fail`) decides:

| Policy | Behavior |
|---|---|
| `fail` (default) | Dispatcher cuts before writing, returns structured `SheetExists` error with `current_state` (n_rows, n_cols, columns), `advice`, and `valid_next_moves` (rename / update_in_place / overwrite). Forces the LLM to make an explicit choice. |
| `auto_suffix` | Writes to `"Name (2)"` silently (legacy pre-2026-06-07 default). |
| `overwrite` | Replaces the tab without asking (assumes operator owns the risk). |

The error envelope the LLM sees on `fail`:

```json
{
  "error": "SheetExists",
  "tab": "Sales",
  "spreadsheet_id": "1xyz...",
  "current_state": {
    "n_rows": 4998, "n_cols": 12,
    "columns": ["sale_id","date","product_id","name","category","quantity","list_price","sale_price","revenue","margin","region","channel"]
  },
  "advice": "The tab 'Sales' already exists with data. Recommended: use a different name (e.g., 'Sales_analysis'). If you must touch the existing tab, choose update_in_place (patch specific rows) or overwrite (replace everything — destructive).",
  "valid_next_moves": [
    {"action": "rename", "example_code": "output_sheets = {'Sales_review': df}"},
    {"action": "update_in_place", "example_code": "output_sheets = {'Sales': {'mode':'update_in_place','df':df,'key':'<unique_col>'}}"},
    {"action": "overwrite", "example_code": "output_sheets = {'Sales': {'mode':'overwrite','df':df}}"}
  ]
}
```

### Validations enforced before any write

| Check | Triggers when | Error code |
|---|---|---|
| Key column missing | `key` not in current or new df | `KeyColumnMissing` |
| Duplicate keys in target | Target has 2+ rows with the same `key` value | `DuplicateKeyInTarget` |
| Duplicate keys in input | Input df has 2+ rows with the same `key` value | `DuplicateKeyInInput` |
| Column mismatch | Input df has extra columns not in target (rejects unless `mode=overwrite` with `allow_schema_change: true`) | `ColumnMismatch` / `SchemaChange` |
| Strict match | `strict_match: true` and any input row's key isn't in target | `StrictMatchFailed` |

### Shared modules

- `dag_engine/infrastructure/nodes/llm_synthetic_tools/sheet_collision.rs` — `CollisionPolicy` enum (Fail/AutoSuffix/Overwrite, default Fail) + `parse_policy` + `build_sheet_exists_error`.
- `dag_engine/infrastructure/nodes/llm_synthetic_tools/diff_writer.rs` — pure records diff with NaN-safe equality + the 6 validation variants above. Used by both `gsheets_run_python` and `crdt_doc_run_python` for the `update_in_place` mode.
- `SheetsClient::batch_update_cells(id, sheet, Vec<(A1, CellValue)>)` — new trait method that issues one `spreadsheets.values:batchUpdate` request for N cell-level writes.

### Migration notes

- The legacy `output_sheet` (singular) Python global + `write_to_sheet` arg in `crdt_doc_run_python` have been **removed** — code that used them must switch to `output_sheets = {name: df}`.
- Default collision behavior changed from silent `auto_suffix` to `fail`. Existing graphs that depended on it must set `fixed_config.on_existing_sheet: "auto_suffix"` explicitly.

See [`docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md`](../superpowers/specs/2026-06-06-sheets-write-safety-design.md) for the full design.

## Hexagonal layout

- `src/libs/colmena/src/gsheets/domain/` — `SheetsClient` trait,
  value types, errors.
- `src/libs/colmena/src/gsheets/infrastructure/` — REST adapter
  (`http_client.rs`), auth (`auth.rs`), config (`config.rs`).
- `src/libs/colmena/src/gsheets/application/format.rs` — `gsheets_format_range`
  format builder: A1→`GridRange` + hex→`RgbColor` helpers, `fields`-mask
  assembly, fan-out to `repeatCell` / `updateBorders` / `updateDimensionProperties`.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs` —
  the gsheets dispatchers.

## Out of scope for v1 (BACKLOG)

See "Subsystem E v1.1" in `docs/BACKLOG.md`: charts, conditional
formatting, data validation, revisions, webhooks. (list_spreadsheets
discovery, OAuth user-scoped auth, cell formatting, permissions /
sharing, and the E-T7b xlsx attachment plumbing have all shipped.)

## Spec + plan

- Spec: `docs/superpowers/specs/2026-06-05-google-sheets-design.md`
- Plan: `docs/superpowers/plans/2026-06-05-google-sheets.md`
- Cell formatting (E v1.1) — spec:
  `docs/superpowers/specs/2026-06-22-gsheets-cell-formatting-design.md`,
  plan: `docs/superpowers/plans/2026-06-22-gsheets-cell-formatting.md`,
  E2E graph: `tests/graphs/agents/gsheets_format_range_e2e.json`.
