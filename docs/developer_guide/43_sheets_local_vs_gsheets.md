# Sheets local (CRDT) vs Google Sheets — guía de orientación

Colmena tiene **dos subsistemas de hojas de cálculo** que comparten varias
ideas (`output_sheets`, modos `replace`/`update_in_place`/`overwrite`,
collision policy) pero apuntan a casos de uso distintos. Esta guía te ayuda
a elegir uno u otro y te da un mapa rápido de los grafos ejemplo que ya
existen en el repo para que pruebes cada uno.

---

## TL;DR — ¿cuál usar?

| Si necesitás… | Usá |
|---|---|
| Colaboración real-time entre humano + agente sobre el mismo workbook (browser + LLM editan en paralelo, sync vía WebSocket) | **CRDT (local)** |
| Importar/exportar `.xlsx` (calamine in, rust_xlsxwriter out) sin tocar APIs externas | **CRDT (local)** |
| Trabajar offline o en entornos sin internet (CI, dev local sin auth) | **CRDT (local)** |
| Operar sobre una planilla que el usuario ya tiene en su Google Drive | **gsheets** |
| Compartir el resultado con stakeholders no técnicos via link de Google | **gsheets** |
| Datos sensibles que NO pueden tocar Google (compliance) | **CRDT (local)** |
| Análisis pandas sin que las filas pasen por el LLM (token-cheap) | **Cualquiera** — ambos tienen `run_python` |

---

## Side-by-side overview

|  | **CRDT local** (subsistemas B/C/D/F) | **gsheets** (subsistema E) |
|---|---|---|
| **Backend** | `yrs::Doc` en memoria + snapshot a disco (cada 5s) | Google Sheets API (HTTPS) |
| **Sync** | Yjs v1 protocol sobre WebSocket → Univer en browser | Server-side via batchUpdate calls |
| **Colaboración** | Multi-peer real-time (humano + LLM + Python script editando juntos) | Una operación a la vez; concurrencia = last-writer-wins |
| **Persistencia** | Snapshot `.bin` en disco; opcional Postgres event log | Google Drive |
| **Auth** | Ninguna (proceso local) o sesión Yjs WS | Service Account JSON o ADC |
| **Import xlsx** | ✅ `calamine` + import endpoint | ⚠️ Solo via `gsheets_create_from_xlsx` (Drive auto-conversion) |
| **Export xlsx** | ✅ `rust_xlsxwriter` | ✅ `gsheets_export_xlsx` |
| **Fórmulas server-side** | ✅ subsistema D (`formualizer` recalc cascade) | ✅ Google evalúa (USER_ENTERED) |
| **Token cost en análisis** | Bajo — pandas in-proc, filas no salen del proceso | Bajo — pandas in-proc, filas no salen del agente; HTTPS al fetch |
| **Visibilidad para humanos** | Browser via Univer + endpoint REST | Link de Google Sheets compartible |
| **Mejor para** | Workflows interactivos (canvas + agente) | Workflows batch + entrega a stakeholders |

---

## Tool surface por subsistema

### CRDT (10 tools sintéticos)

Definidos en `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_*.rs`. Activados via bloque `crdt_documents` en el `llm_call` (necesita `artifact_id`).

| Tool | Hace |
|---|---|
| `crdt_doc_list_sheets` | Lista las tabs del workbook actual + formula_count por sheet |
| `crdt_doc_read` | Lee un rango (A1 notation) opcionalmente con fórmulas (`include_formulas`) |
| `crdt_doc_set_cell` | Escribe una celda; `=` se evalúa server-side via formualizer |
| `crdt_doc_set_range` | Bulk write 2D array |
| `crdt_doc_add_sheet` | Crea una nueva tab |
| `crdt_doc_get_recent_changes` | (Subsistema B) Lista cambios desde el último cursor del agente — auto-narration de diffs |
| `crdt_doc_run_python` | Sandbox pandas/numpy/scipy sobre `dfs[sheet_id]`; soporta `output_sheets` write-back |
| `crdt_doc_list_sheets_of` | (Subsistema F) Peek a las sheets de OTRO artifact sin importarlo |
| `crdt_doc_import_sheet` | (Subsistema F) Clona una sheet de otro artifact (snapshot) al actual |
| `list_my_artifacts` / `create_artifact` | Workspace-level: el agente descubre qué workbooks ya existen y crea nuevos |

Activación en `llm_call`:
```json
{
  "type": "llm_call",
  "config": {
    "crdt_documents": {
      "artifact_id": "$DYNAMIC:artifact_id",   // canvas-seeded o auto-creado
      "ws_url": "ws://127.0.0.1:8090/yjs",     // modo WsPeer (producción)
      "storage_root": ".colmena/crdt_documents" // modo local autónomo (fallback)
    },
    "enabled_tools": ["crdt_doc_run_python", "crdt_doc_list_sheets", ...]
  }
}
```

### gsheets (10 tools sintéticos)

Definidos en `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_*.rs`. Activados via toolkit package `["gsheets"]` o entries individuales en `tool_configurations`.

| Tool | Hace |
|---|---|
| `gsheets_create_spreadsheet` | Crea una planilla nueva (⚠️ requiere scope `drive.file` o `drive`) |
| `gsheets_create_from_xlsx` | Sube un `.xlsx` attachment como Google Sheet nativo |
| `gsheets_export_xlsx` | Descarga una planilla como `.xlsx` (registra como attachment) |
| `gsheets_list_sheets` | Lista las tabs + row/col count |
| `gsheets_add_sheet` | Crea una nueva tab |
| `gsheets_delete_sheet` | Elimina una tab (por nombre o numeric sheet_id) |
| `gsheets_read` | Lee un rango; modos `FORMATTED_VALUE` / `UNFORMATTED_VALUE` / `FORMULA` |
| `gsheets_set_cell` | Escribe una celda; `=` se evalúa por Google (USER_ENTERED) |
| `gsheets_set_range` | Bulk write 2D array |
| `gsheets_run_python` | Sandbox pandas/numpy/scipy sobre bindings paralelos; soporta `output_sheets` write-back |

Activación en `llm_call`:
```json
{
  "type": "llm_call",
  "config": {
    "enabled_tools": ["gsheets"]   // o ["gsheets", "load_skill"], etc.
  }
}
```

Auth se resuelve via env var `GOOGLE_APPLICATION_CREDENTIALS` (SA JSON) o ADC.

---

## API de write-back unificada — `output_sheets`

**Esto es lo que comparten los dos subsistemas.** Ambos `crdt_doc_run_python` y `gsheets_run_python` aceptan el mismo shape `output_sheets = {name: DataFrame | spec_dict}` desde el código pandas del LLM, con los 3 modos:

```python
# Modo 1 — replace (default; tab debe NO existir o aplica collision policy)
output_sheets = {'Resumen': df}

# Modo 2 — update_in_place (parchea SOLO las celdas cambiadas)
output_sheets = {
    'Sales': {
        'mode': 'update_in_place',
        'df': df_modificado,
        'key': 'product_id',
        'columns': ['price'],   # opcional
    }
}

# Modo 3 — overwrite (reemplaza la tab; explícito)
output_sheets = {'Sales': {'mode': 'overwrite', 'df': df}}
```

**Solo en `gsheets` (no en CRDT, 2026-06-26):** dos extensiones viven únicamente en `gsheets_run_python`:
- **Modo 4 `update_by_position`** — edita filas existentes sin clave única ni A1: bindeás la hoja COMPLETA, modificás el df in place y devolvés el df entero; el dispatcher diffea por **índice de fila**. Ver §39 "Write safety".
- **Placeholders de fórmula `{{Column}}`** — `df.loc[mask,'Importe'] = '={{Cantidad}}*{{Tarifa}}'` → el dispatcher sustituye el A1 real de la misma fila. Funciona en todos los modos de escritura de gsheets. Ver §39 "Formulas".

La **collision policy** (default `fail` → devuelve `SheetExists` con metadata + advice + valid_next_moves) también es idéntica. Se configura via `fixed_config.on_existing_sheet` (valores: `fail` / `auto_suffix` / `overwrite`). Detalles completos en [`docs/developer_guide/39_gsheets.md`](39_gsheets.md) sección "Write safety".

**¿Por qué shared?** El módulo `diff_writer.rs` calcula el diff (pure JSON records), y cada dispatcher aplica los cell changes via su backend:
- CRDT: `apply_set_cell_in_proc(doc, sheet_id, addr, value)` — formula-aware, recalcula dependientes
- gsheets: `batch_update_cells(id, sheet, Vec<(A1, CellValue)>)` — un solo HTTPS round-trip

---

## Grafos ejemplo — catálogo

Todos los grafos viven en `tests/graphs/`. Los ejecutás con:
```bash
set -a; source .env; set +a
cargo run --release --bin dag_engine -- run <ruta/al/graph.json> --agent-session-id e2e_$(date +%s)
```

### CRDT (local) — `tests/graphs/crdt_documents/`

Estos grafos requieren `$DYNAMIC:artifact_id` que el canvas (ADP) o un setup script provee. **No son standalone CLI** salvo que pre-crees un artifact con la herramienta correcta.

| Grafo | Demuestra |
|---|---|
| [`b_recent_changes_turn1.json`](../../tests/graphs/crdt_documents/b_recent_changes_turn1.json) | Subsistema B turn 1 — agente crea un workbook |
| [`b_recent_changes_turn2.json`](../../tests/graphs/crdt_documents/b_recent_changes_turn2.json) | Subsistema B turn 2 — agente ve los cambios que el humano hizo en el browser entre turnos |
| [`c_pandas_smoke.json`](../../tests/graphs/crdt_documents/c_pandas_smoke.json) | Subsistema C — agente crea datos, los analiza con pandas, escribe resultado como nueva hoja via `output_sheets` |
| [`c_import_analysis.json`](../../tests/graphs/crdt_documents/c_import_analysis.json) | Subsistema C — análisis sobre un xlsx importado (con título + headers desfasados); 2 outputs como hojas nuevas |
| [`d_formulas_smoke.json`](../../tests/graphs/crdt_documents/d_formulas_smoke.json) | Subsistema D — fórmulas server-side, cascade recalc, warning needs_browser |
| [`d_formulas_interactive_demo.json`](../../tests/graphs/crdt_documents/d_formulas_interactive_demo.json) | Subsistema D — demo interactivo con Univer en browser via `crdt-yws-graph` |
| [`f_cross_artifact_smoke.json`](../../tests/graphs/crdt_documents/f_cross_artifact_smoke.json) | Subsistema F — cruce Q3 vs Q4 (3 outputs: row diff, schema diff, join) |
| [`llm_agent_smoke.json`](../../tests/graphs/crdt_documents/llm_agent_smoke.json) | Smoke base local |
| [`llm_agent_smoke_ws_peer.json`](../../tests/graphs/crdt_documents/llm_agent_smoke_ws_peer.json) | Smoke en modo WS peer (producción split) |

**Cómo ejecutar un grafo CRDT standalone** (setup mínimo):
1. Levantá el server crdt-yws: `cargo run --bin dag_engine -- crdt-yws-graph <graph.json>`
2. Esto:
   - Crea un artifact con un id random (o el `--seed-artifact-id` que pases)
   - Arranca el server WS en `127.0.0.1:8090`
   - Sirve el browser canvas en `127.0.0.1:8090/canvas/<artifact_id>`
   - Ejecuta el graph contra ese artifact

Alternativa: pasá `--seed-artifact-id art_<26-char-ULID>` para reusar un artifact existente.

### gsheets — `tests/graphs/agents/`

Estos grafos son standalone CLI siempre que tengas la SA + permiso al spreadsheet target.

| Grafo | Demuestra |
|---|---|
| [`gsheets_smoke.json`](../../tests/graphs/agents/gsheets_smoke.json) | Smoke base — agente crea planilla, agrega tab, escribe SUM, lee back valor + fórmula |
| [`gsheets_package_smoke.json`](../../tests/graphs/agents/gsheets_package_smoke.json) | Toolkit package — `enabled_tools: ["gsheets"]` expande a los 10 tools sin listarlos |
| [`gsheets_update_in_place.json`](../../tests/graphs/agents/gsheets_update_in_place.json) | E2E manual — `update_in_place` aplica 10% discount a Electronics; solo las celdas cambiadas se escriben (no las 1000 filas) |

**Cómo ejecutar:**
```bash
set -a; source .env; set +a
export GOOGLE_APPLICATION_CREDENTIALS=/path/to/sa.json
cargo run --release --bin dag_engine -- run tests/graphs/agents/gsheets_smoke.json \
  --agent-session-id e2e_$(date +%s) > /tmp/colmena_e2e/gsheets_smoke.sse 2>&1
```

El `update_in_place` graph necesita un spreadsheet con tab 'Sales' pre-seeded — edita el `spreadsheet_id` en el JSON para apuntar al tuyo.

---

## Patrones canónicos comparados

### Patrón A — Análisis sin pasar filas por el LLM

**CRDT:**
```json
{
  "tool": "crdt_doc_run_python",
  "args": {
    "sheet_ids": ["sheet_inventory"],
    "code": "import pandas as pd\ndf = pd.DataFrame(dfs['sheet_inventory'])\noutput = {'total': df['qty'].sum()}"
  }
}
```

**gsheets:**
```json
{
  "tool": "gsheets_run_python",
  "args": {
    "bindings": [{"var": "inv", "spreadsheet_id": "1abc", "sheet": "Inventory"}],
    "code": "import pandas as pd\ndf = pd.DataFrame(inv)\noutput = {'total': df['qty'].sum()}"
  }
}
```

**Diferencias:**
- CRDT: el dispatcher auto-construye `dfs[<sheet_id>]` (los sheets se identifican por id opaco).
- gsheets: el LLM elige el nombre del binding (`inv` en este ejemplo) y llama `pd.DataFrame(inv)` manualmente. Cada binding se fetchea **en paralelo** (multi-sheet joins ultra-rápidos).

### Patrón B — Escribir nuevas tabs derivadas

**Ambos** aceptan exactamente el mismo shape — copiá-pegá funciona si el LLM solo cambia `enabled_tools`:

```python
output_sheets = {
    'Resumen': df.groupby('region')['revenue'].sum().reset_index(),
    'Top10':   df.nlargest(10, 'revenue'),
}
output = {'tabs_written': 2}
```

CRDT: el dispatcher escribe al artifact actual (no se necesita `write_to_spreadsheet`).
gsheets: el agente debe pasar `write_to_spreadsheet=<id>` (puede ser el mismo que las bindings o uno diferente).

### Patrón C — Parchear celdas existentes (update_in_place)

```python
df = pd.DataFrame(sales)
df.loc[df['category'] == 'Electronics', 'price'] *= 0.9

output_sheets = {
    'Sales': {
        'mode': 'update_in_place',
        'df': df,
        'key': 'product_id',
        'columns': ['price'],   # solo patchea esta columna
    }
}
```

Ambos backends:
1. Fetchean el estado actual de la tab (CRDT: in-proc; gsheets: HTTPS read)
2. Computan el diff vs el nuevo df (función shared `diff_records()`)
3. Aplican SOLO las celdas cambiadas
4. Devuelven `{mode: 'update_in_place', changes: {rows, cells, columns}, unchanged, skipped}` al LLM

Si el filter pandas no matcheó nada, se devuelve `changes.cells=0` y **NO se hace ninguna llamada de escritura** — safety guard.

### Patrón D — Cruzar dos workbooks distintos

**CRDT:** subsistema F — `crdt_doc_list_sheets_of(art_other)` → `crdt_doc_import_sheet` para clonar al artifact actual → `crdt_doc_run_python` con `sheet_ids=[mi_sheet, sheet_clonado]`.

**gsheets:** un solo `gsheets_run_python` con DOS bindings (cada uno con un `spreadsheet_id` distinto):
```python
{
  "bindings": [
    {"var": "q3", "spreadsheet_id": "1abc", "sheet": "Sales"},
    {"var": "q4", "spreadsheet_id": "1xyz", "sheet": "Sales"}
  ],
  "code": "..."
}
```

gsheets es marcadamente más simple porque no hay concepto de "artifact" — cualquier id de planilla es accesible si la SA tiene permiso.

---

## Convivencia de los dos subsistemas

**No son alternativas excluyentes.** Un mismo grafo puede tener:

```json
"enabled_tools": ["gsheets", "load_skill"],
"crdt_documents": {
  "artifact_id": "$DYNAMIC:artifact_id",
  "ws_url": "ws://127.0.0.1:8090/yjs"
}
```

En ese caso el agente ve ambos toolsets. Útil para flows tipo "el humano edita en Univer (CRDT) pero también traemos datos de un Google Sheet existente para mergear".

---

## Cuándo elegir uno o el otro

### CRDT cuando:

- El usuario está en el **canvas de ADP** y quiere ver el resultado en tiempo real
- Necesitás **multi-peer concurrency** (humano + LLM + script Python en paralelo)
- El input es un **xlsx que el usuario sube** (no vive en Google)
- **Compliance** prohíbe que los datos toquen Google
- Estás en **dev local sin internet**

### gsheets cuando:

- El usuario te pide trabajar sobre **su Google Sheet existente** (te pasa el link)
- El resultado debe ser **compartible via link** con stakeholders no técnicos
- Necesitás que el resultado **sobreviva más allá de la sesión** (Google Drive como storage durable)
- El workflow involucra **múltiples spreadsheets distintos** (gsheets soporta cross-spreadsheet binding en una sola tool call)
- Workflows tipo **"agendador" o "batch"** que corren sin canvas abierto

---

## Referencias

- **CRDT (local):** [`38_crdt_documents.md`](38_crdt_documents.md) — guía completa, persistencia, subsistemas B/C/D/F.
- **gsheets:** [`39_gsheets.md`](39_gsheets.md) — guía completa, auth, write safety.
- **Spec sheets write safety (P1+P2):** [`docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md`](../superpowers/specs/2026-06-06-sheets-write-safety-design.md) — collision policy + update_in_place diseño compartido.
- **CHANGELOG §11:** [`docs/CHANGELOG_2026-06.md`](../CHANGELOG_2026-06.md) — entry del shipping del 2026-06-07.
- **Built-in tools index:** [`41_builtin_tools_index.md`](41_builtin_tools_index.md) — lista completa de tools sintéticos.
