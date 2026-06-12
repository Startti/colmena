# 39. Google Sheets integration (Subsystem E)

> v1 ships 9 synthetic LLM tools mirroring `crdt_doc_*` shape — agents
> read, write, create, and analyse Google Sheets via the Sheets API v4 +
> Drive API. Auth via Service Account JSON or Application Default
> Credentials. No OAuth user-scoped flow in v1.

## Recommended activation

Enable the whole gsheets surface with one alias:

```json
"enabled_tools": ["gsheets"]
```

This expands to all 10 gsheets tools via the toolkit-packages registry.
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
`gsheets_set_range` and Google parses, evaluates, and cascades it. Read
back via `gsheets_read(..., value_render="UNFORMATTED_VALUE")` to get
the computed number, or `value_render="FORMULA"` to get the text.

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
(mode = `replace`, default) or a spec dict with one of three modes:

| Mode | When to use | What gets written |
|---|---|---|
| `replace` (default) | Create a brand-new tab | Full DataFrame; collision policy applies if tab exists |
| `update_in_place` | Patch SOME rows in an existing tab | Cell-level diff via single `batchUpdate` — only changed cells |
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
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs` —
  9 dispatchers.

## Out of scope for v1 (BACKLOG)

See "Subsystem E v1.1" in `docs/BACKLOG.md`: list_spreadsheets discovery,
OAuth user-scoped auth, cell formatting, charts, conditional formatting,
permissions / sharing, revisions, webhooks, plus the E-T7b xlsx
attachment plumbing.

## Spec + plan

- Spec: `docs/superpowers/specs/2026-06-05-google-sheets-design.md`
- Plan: `docs/superpowers/plans/2026-06-05-google-sheets.md`
