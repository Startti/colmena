# 38. CRDT Documents (v1)

El módulo `crdt_documents` (`src/libs/colmena/src/crdt_documents/`) implementa **workbooks Excel colaborativos en tiempo real**: múltiples humanos + agentes LLM + scripts Python pueden editar el mismo documento simultáneamente sobre un `yrs::Doc` (CRDT), con sincronización vía WebSocket, persistencia en disco, e ingesta/exportación de `.xlsx`. Esta es la implementación de v1 del feature.

> **Código:** [`src/libs/colmena/src/crdt_documents/`](../../src/libs/colmena/src/crdt_documents/)
> **Spec:** [`docs/superpowers/specs/2026-06-01-documents-crdt-v1-design.md`](../superpowers/specs/2026-06-01-documents-crdt-v1-design.md)
> **Spike:** [`docs/superpowers/specs/2026-05-31-documents-crdt-spike-design.md`](../superpowers/specs/2026-05-31-documents-crdt-spike-design.md) + [`results`](../superpowers/specs/2026-05-31-documents-crdt-spike-results.md)

---

## 1. Diferencias con la librería `documents` (§27)

| Aspecto | `documents` (§27) | `crdt_documents` (este, §38) |
|---|---|---|
| Modelo de mutación | Patches versionados (`v1`, `v2`, …) | CRDT (yrs) con sync incremental |
| Concurrencia | Agente + usuario por turnos (rebase + VersionConflict) | Multi-peer simultáneo (humanos + agentes en paralelo) |
| Frontend | Backend-only; espera Univer/Tiptap a futuro | Univer 0.2.10 + y-websocket integrado |
| Persistencia | IR JSON por versión + binario rendered | Snapshot Yjs binario (cada 5 s + on shutdown) |
| Tipos de documento | Excel, Word, HTML | Solo Excel en v1 |
| Versionado | Sí, inmutable (20 versiones default) | No (snapshot único; named versions = v1.1) |
| Formato visual (colores, merges, fórmulas) | Soportado en IR | Solo valores en v1 ([roadmap v1.1](../BACKLOG.md)) |

Los dos módulos **coexisten** y resuelven casos de uso distintos:
- Si el LLM genera un documento one-shot sin colaboración en tiempo real → usar `documents`.
- Si el usuario y el LLM colaboran en vivo sobre un workbook → usar `crdt_documents`.

---

## 2. Arquitectura

```
┌────────────────────────────────────────────────────────────────┐
│  Browser (Univer + y-websocket)                                │
│  ws://host/documents/:id/yjs  ◄────── Yjs sync v1 ──────►      │
└────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌────────────────────────────────────────────────────────────────┐
│  colmena (dag_engine binary, subcomando crdt-yws)              │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ axum router                                               │  │
│  │   POST/GET/DELETE /documents                              │  │
│  │   POST   /documents/:id/import (xlsx → calamine)          │  │
│  │   GET    /documents/:id/export.xlsx (rust_xlsxwriter)    │  │
│  │   GET    /documents/:id/projection.json                  │  │
│  │   WS     /documents/:id/yjs                              │  │
│  └──────────────────────────────────────────────────────────┘  │
│                              │                                  │
│                              ▼                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ CrdtDocumentsRuntime (bundler)                            │  │
│  │   ├─ DocRegistry: HashMap<ArtifactId, RegisteredArtifact> │  │
│  │   ├─ ArtifactStorage trait (LocalFs / Gcs feature-gated) │  │
│  │   └─ ChangeTracker buffer (1000 events / artifact)      │  │
│  └──────────────────────────────────────────────────────────┘  │
│              │              │              │                   │
│              ▼              ▼              ▼                   │
│  ┌──────────┐ ┌────────────────┐ ┌──────────────┐              │
│  │projection│ │snapshot writer │ │ tool_executor│              │
│  │ Yrs→IR   │ │ per-artifact   │ │ apply_set_cell│             │
│  │ JSON     │ │ task (5s tick) │ │ apply_add_sht │             │
│  └──────────┘ └────────────────┘ └──────────────┘              │
└────────────────────────────────────────────────────────────────┘
                              ▲
                              │
            ┌─────────────────┼────────────────────┐
            │                 │                    │
┌───────────┴────────┐ ┌──────┴────────┐ ┌─────────┴───────┐
│ llm_call node      │ │ python_node   │ │ Direct CLI      │
│ (.crdt_documents   │ │ (colmena.docs │ │ (crdt-agent)    │
│  config block)     │ │  PyO3 bindings│ │                 │
│ 6 synthetic tools  │ │ + pandas helpr│ │                 │
└────────────────────┘ └───────────────┘ └─────────────────┘
```

---

## 3. IR (Intermediate Representation)

El `Y.Doc` por artifact tiene la siguiente estructura:

```
Y.Doc
  └─ Y.Map "workbook"
       └─ Y.Array "sheets"
            └─ Y.Map (per sheet)
                 ├─ "id"    : string (sh_<ulid>)
                 ├─ "name"  : string
                 └─ "cells" : Y.Map<addr, Y.Map>
                                 └─ Y.Map { "v": any, "t": "s"|"n"|"b" }
```

La projection a JSON (`projection::project(&Doc)`) emite:

```json
{
  "sheets": [
    {
      "id": "sh_01H...",
      "name": "Sales",
      "cells": { "A1": "Product", "B1": 10.5, "C2": true }
    }
  ]
}
```

**Performance**: p50 = 1.38ms en 1000 celdas (validado en spike R2.1, ver results).

---

## 4. APIs REST

| Endpoint | Método | Body / Params | Respuesta |
|---|---|---|---|
| `/documents` | POST | `{name}` | 201 `{artifact_id, created_at}` |
| `/documents` | GET | — | 200 `{artifacts: [{artifact_id, name, created_at, updated_at, sheet_count}]}` |
| `/documents/:id` | DELETE | — | 204 (idempotente) |
| `/documents/:id/import` | POST | `application/octet-stream` xlsx bytes | 200 `{sheets_imported, cells_imported}` |
| `/documents/:id/export.xlsx` | GET | — | 200 `Content-Type: application/vnd.openxmlformats-...sheet` |
| `/documents/:id/projection.json` | GET | — | 200 IR JSON |
| `/documents/:id/yjs` | WS | Yjs sync v1 protocol | upgrade 101 |
| `/yjs/:id` | WS | alias para y-websocket clients | upgrade 101 |

### Ejemplo end-to-end

```bash
# Crear artifact
ID=$(curl -s -X POST http://localhost:8090/documents \
  -H 'content-type: application/json' \
  -d '{"name":"Q3 Sales"}' | jq -r .artifact_id)

# Importar xlsx
curl -X POST "http://localhost:8090/documents/$ID/import" \
  --data-binary @report.xlsx

# Ver projection
curl "http://localhost:8090/documents/$ID/projection.json" | jq .

# Exportar
curl "http://localhost:8090/documents/$ID/export.xlsx" -o updated.xlsx
```

---

## 5. LLM tools (synthetic)

Cuando un `llm_call` node incluye un bloque `crdt_documents` en su config, se exponen automáticamente **6 synthetic tools**:

| Tool | Args | Retorna |
|---|---|---|
| `crdt_doc_list_sheets` | (ninguno) | `{sheets: [{sheet_id, name}]}` |
| `crdt_doc_read` | `sheet_id`, `range?` (A1-style) | `{sheet_id, cells: {addr: value}}` |
| `crdt_doc_set_cell` | `sheet_id`, `addr`, `value` | `{ok: true}` |
| `crdt_doc_set_range` | `sheet_id`, `start_addr`, `values_2d` | `{ok: true, cells_written}` |
| `crdt_doc_add_sheet` | `name` | `{sheet_id}` |
| `crdt_doc_get_recent_changes` | `since_event_id?` | `{current_event_id, narration}` |

### Config block — tres modos

El bloque `crdt_documents` selecciona uno de tres modos según qué campos estén presentes. Prioridad descendente:

#### Modo 1 — WsPeer (producción recomendada)

```json
"crdt_documents": {
  "artifact_id": "art_01H1234567890ABCDEFGHJKMNP",
  "ws_url": "ws://crdt-service.internal:8090/yjs"
}
```

El agente abre una conexión WebSocket al CRDT documents server, hace el handshake Yjs sync v1, y construye una **réplica local del Y.Doc**. Las mutaciones se propagan al server vía el protocolo de sync; el server hace fan-out a browsers y otros agentes conectados al mismo artifact.

**Cuándo**: deployment en producción donde el WS server vive en su propio Cloud Run (separado del worker que ejecuta el graph). Persistencia es responsabilidad del server.

**On-failure**: fail-fast. Si la conexión muere mid-call, las tool calls subsiguientes devuelven `artifact_not_found` y el LLM ve el error. Auto-reconnect está deferido a v1.1.

#### Modo 2 — Local con singleton (monolito / dev colocalizado)

```json
"crdt_documents": {
  "artifact_id": "art_01H1234567890ABCDEFGHJKMNP"
}
```

(Sin `ws_url`.) Si el proceso instaló un `CrdtDocumentsRuntime` global via `process_runtime::set_global`, el agente lo reusa — comparte `Arc<DocRegistry>` con el WS server colocalizado. Mutaciones son escrituras directas al `Doc` en RAM; el server observa el doc y broadcastea a peers WS.

**Cuándo**: deployment monolítico donde el worker hostea tanto la ejecución del graph como el WS server (ej. `crdt-yws-graph` subcommand para dev).

#### Modo 3 — Local autónomo (CLI / tests)

```json
"crdt_documents": {
  "artifact_id": "art_01H1234567890ABCDEFGHJKMNP",
  "storage_backend": "localfs",
  "storage_root": ".colmena/crdt_documents"
}
```

(Sin `ws_url`, sin singleton.) Cada `execute()` construye un runtime nuevo via `CrdtDocumentsRuntime::from_config`. Persistencia a disco local (localfs) o GCS. No hay live server — los cambios solo viven en disco hasta que algo más los lea.

**Cuándo**: pipelines batch sin componente live (`dag_engine run` standalone), tests automatizados que no levantan server.

### Cómo se elige el modo (en código)

En `llm.rs` el orden es:
1. ¿Hay `ws_url` en config? → **WsPeer**, vía `WsPeerArtifact::connect`.
2. ¿Hay `process_runtime::get_global()` instalado? → **Local con singleton**.
3. Default → **Local autónomo**, `from_config`.

`artifact_id` siempre es required y el LLM nunca lo ve.

### Flujo típico del agente

1. `crdt_doc_list_sheets()` para descubrir qué sheets hay.
2. `crdt_doc_read(sheet_id, "A1:Z100")` para leer datos relevantes.
3. `crdt_doc_get_recent_changes(since_event_id=<last>)` para ver qué cambió el usuario desde el turn pasado.
4. Decidir mutaciones; aplicar con `crdt_doc_set_cell` / `crdt_doc_set_range`.
5. Reportar al usuario el `current_event_id` para anclarse en el próximo turn.

### 5.5 Recent changes awareness + discovery

#### Auto-injected summary block

Cuando `llm_call` corre con `crdt_documents` configurado AND con `agent_session_id`, el sistema auto-inyecta un bloque corto al `system_message` con los cambios que OTROS peers hicieron al workbook desde el último turn del agente:

```
---
Workbook changes since your last turn (5 events, 2 peers):
- Inventory: 3 changes by peer:browser
- Pricing: 2 changes by agent:orchestrator
Use `crdt_doc_get_recent_changes(sheet_id?)` for cell-level detail.
---
```

Reglas:
- Solo se inyecta si hay >0 eventos relevantes (filtra mutaciones del propio agente vía `origin = agent:{session_id}`).
- Tope 10 sheets en el listado; overflow muestra `...and N more sheet/peer groups changed`.
- Eventos de browser tienen `sheet_id` null en v1 (limitación documentada en BACKLOG); aparecen como bucket "Workbook (sheet unknown)".
- El cursor del agente se actualiza al **final del turn** vía `backend.upsert_cursor` — si el turn falla mid-way, el cursor no se mueve.

#### Tools nuevos / extendidos

| Tool | Args | Returns |
|------|------|---------|
| `crdt_doc_get_recent_changes` | `since_event_id?`, `sheet_id?`, `limit?` (default 50) | `{current_event_id, events[], truncated}` |
| `crdt_doc_list_my_artifacts` | `limit?` (default 50) | `{artifacts[{artifact_id, name, created_at, last_accessed_at}]}` |
| `crdt_doc_create_artifact` | `name` | `{artifact_id, name}` — para mutar el nuevo artifact requiere otro turn que lo pinee en `crdt_documents.artifact_id` (limitación v1; multi-artifact write es subsistema F) |

#### Tablas SQL

Tres tablas se crean automáticamente al startup via migrations (`src/libs/colmena/migrations/{sqlite,postgres}/20260603000000_crdt_doc_changes.sql`):

- `crdt_doc_events` — log append-only (`id, artifact_id, sheet_id?, origin, summary, created_at`). Indexado para drill-down por `(artifact_id, id)` y `(artifact_id, sheet_id, id)`.
- `crdt_doc_session_cursors` — cursor por `(agent_session_id, artifact_id)` → `last_event_id`.
- `crdt_doc_session_artifacts` — ownership: qué artifacts pertenecen a qué session (`agent_session_id, artifact_id, name, created_at, last_accessed_at`).

#### Backend abstraction

`CrdtDocsContext` ahora tiene un campo `backend: Arc<dyn CrdtBackend>` con dos implementaciones:

- `DirectBackend`: Local mode — usa `ChangeTrackerStore` directo (worker = server colocalizado).
- `RestBackend`: WsPeer mode — hace HTTP al server (worker stateless, server dedicado).

Los tool dispatchers no saben qué modo está activo — solo llaman `ctx.backend().record_event(...)` etc.

#### Attribution de origin

- Mutaciones del agente: `origin = format!("agent:{session_id}")`.
- Mutaciones de browsers: `origin = "peer:browser"` (atributo definido en el WS handshake vía query param `peer_type`).
- Filtro de own-events en `get_recent_changes` y auto-summary: `WHERE origin != "agent:{session_id}"`.

### 5.6 Python/pandas analysis (subsistema C)

Tool: `crdt_doc_run_python(sheet_ids, code, write_to_sheet?)`.

#### Por qué existe

Para Excel grandes (>1000 filas), pasar todo el contenido al LLM en su contexto es prohibitivo en tokens (~125k tokens para un workbook de 10k filas). La pattern: agente lee solo un sample con `crdt_doc_read("A1:Z10")` para entender el schema, después llama `run_python` con código que opera sobre el dataset completo server-side.

Ahorro típico: 10x-1000x en tokens dependiendo del tamaño.

#### Cómo se usa típicamente

```
Turn 1 — exploración (cheap):
   crdt_doc_list_sheets()
   crdt_doc_read(sh_inventory, "A1:Z10")
   
Turn 2 — análisis:
   crdt_doc_run_python(
       sheet_ids=["sh_inventory"],
       code="
           df = dfs['sh_inventory']
           output = df.groupby('Region')['Sales'].sum().to_dict()
       "
   )
   → output = {"North": 450, "South": 320, ...}
   
Turn 3 — persistir resultado en una nueva hoja:
   crdt_doc_run_python(
       sheet_ids=["sh_inventory"],
       code="
           df = dfs['sh_inventory']
           output_sheet = df.groupby('Region').agg({'Sales': 'sum', 'Qty': 'mean'}).reset_index()
       ",
       write_to_sheet="Summary by Region"
   )
   → wrote_sheet = {sheet_id: "sh_summary", name: "Summary by Region", n_rows: 4, preview: [...]}
```

#### Sandbox + librerías

Reusa la infra `restricted` de `python_script` (AST validation + import whitelist + banned builtins). v1 agrega `pandas`, `numpy`, `scipy` a la whitelist. Bloqueados (sin cambio): `open, exec, eval, compile, __import__` + cualquier import fuera de la whitelist (incluye `requests`, `urllib`, `os`, `subprocess`, etc.).

#### Convenciones de I/O

- **Input**: `dfs: dict[sheet_id, pd.DataFrame]` — una DataFrame por sheet pedido. Row 1 del workbook = column names. Headers ausentes/no-string → fallback `col_A`, `col_B`.
- **Output al LLM**: variable `output` (cualquier JSON-serializable). Cap 10KB; trunca con `_output_truncated: true`.
- **Write-back**: variable `output_sheet` (pd.DataFrame). Solo se escribe si `write_to_sheet` está en args. Headers as row 1, sin index. Name collisions → auto-suffix `" (2)"`, `" (3)"`. Cap 100k rows; trunca con `truncated_at` en response.

#### Límites v1 (hardcoded, deuda técnica)

| Límite | Valor | Path v1.1 |
|---|---|---|
| Combined records load | 100 MB | Configurable via `crdt_documents.run_python_limits.max_load_mb` |
| Code execution timeout | 30s | Idem (`timeout_secs`) |
| `output` to LLM | 10 KB | Idem |
| `stdout` / `error` | 10 KB cada uno | Idem |
| `output_sheet` rows | 100K | Idem + chunked writes para evitar transact_mut gigante |
| Sheet name | 31 chars (Excel xlsx limit) | Stays — hard limit |

Ver `docs/BACKLOG.md` → "Configurable limits para `crdt_doc_run_python`".

#### Modo Local vs WsPeer

Mismo comportamiento. En WsPeer mode el worker tiene la réplica Y.Doc local via WS, entonces la construcción del DataFrame es local, sin roundtrip. Las escrituras de `output_sheet` van como mutaciones Y.Doc → propagan al server via WS → fan-out a browsers.

#### Requisito de runtime

Pandas, numpy y scipy deben estar disponibles en el Python embebido por PyO3 del worker. En el `.venv` del proyecto:

```bash
.venv/bin/pip install pandas numpy scipy
```

En producción ADP el worker container debe incluir estas deps. Si no están, los tests `#[ignore]` correspondientes se skipean y el tool retorna error de "module not found" en ejecución.

---

### 5.7 Cross-sheet & cross-artifact analysis (subsistema F)

**Por qué existe.** Los workflows reales con xlsx tienen dos formatos: (a) un workbook con varias hojas que se comparan entre sí, (b) dos+ workbooks separados que se quieren cruzar. F unifica ambos casos vía clonado: cualquier sheet de cualquier artifact se puede traer al artifact actual y a partir de ahí todo es multi-sheet pandas (que ya funcionaba desde C).

**Tools nuevos:**

- `crdt_doc_list_sheets_of({artifact_id})` — peek a otro artifact sin clonar. Devuelve `{artifact_id, name, sheets:[{sheet_id, name, n_rows, n_cols}]}`.
- `crdt_doc_import_sheet({source_artifact_id, source_sheet_id, new_name?})` — clona la sheet completa al artifact actual. Snapshot (no live link). Resuelve collisions con sufijo ` (2)`, ` (3)`, …
- `crdt_doc_get_recent_changes` extendido — ahora acepta `artifact_id?` opcional para auditar otros artifacts.

**Caps:** `MAX_IMPORT_BYTES = 100 MB` (mismo que `run_python`), `MAX_SHEETS_PER_ARTIFACT = 100` (defensivo).

**Skill builtin:** `crdt-doc-cross-sheet-analysis` documenta 6 patrones canónicos pandas (cell-diff, row-diff por key, schema-diff, statistical, join/enrich, conditional transform) con snippets verbatim. Activación: `config.skills.builtin: ["crdt-doc-cross-sheet-analysis"]`.

**Auditoría cross-session.** El evento del import incluye el artifact origen en el summary (`"imported sheet 'X' (N rows × M cols) from artifact art_xxxx"`), entonces el log de cambios recientes muestra qué entró desde dónde sin importar quién creó el origen.

**Limitaciones v1:**
- Snapshot only — cambios posteriores en el source NO se propagan al clone (live linking es BACKLOG).
- No hay `crdt_doc_delete_sheet` para limpiar sheets clonadas (BACKLOG).
- Permisos por artifact: cualquier agente con el `artifact_id` puede leer e importar; modelo de permisos es BACKLOG (bloqueante para subsistema A).
- Cross-session discovery sigue scoped a `list_my_artifacts` (session-only); cuando shippeemos workspace concept los tools de F siguen funcionando sin cambios.

Spec completa: [`docs/superpowers/specs/2026-06-04-crdt-cross-sheet-analysis-design.md`](../superpowers/specs/2026-06-04-crdt-cross-sheet-analysis-design.md).

### 5.8 Formulas (Subsystem D)

Cells with a value starting with `=` are treated as Excel-style formulas:

- **Parsed and evaluated server-side** by [`formualizer`](https://crates.io/crates/formualizer)
  before persistence (`apply_set_cell_in_proc`).
- Cell state grows two optional keys: `f` (formula text) and
  `fs` (source: `"be"` backend, `"fe"` browser, `"needs_browser"`).
- Dependent formulas in the same sheet recalculate immediately
  (intra-sheet eager — cross-sheet recalc is v1.1).
- Reads return scalars by default. Pass `include_formulas: true`
  to `crdt_doc_read` to see the `{v, f, fs}` shape.
- `crdt_doc_list_sheets` returns `formula_count` per sheet so agents can
  decide whether to bother with the formula-aware read.
- Unsupported functions (functions formualizer doesn't recognize): cell is
  written with `fs:"needs_browser"`, value = formula text as placeholder.
  Tool result includes `warnings: [{kind:"needs_browser", addr, functions}]`.
- pandas (`run_python`) writing back over a formula cell removes `f`/`fs`,
  records a `FormulaReplacement` in `DfWriterOutcome`, and emits a
  `formula_replaced_by_literal` CRDT event.

The full design is at
[`docs/superpowers/specs/2026-06-04-crdt-formulas-design.md`](../superpowers/specs/2026-06-04-crdt-formulas-design.md).
Agent-facing patterns live in the skill at
`src/libs/colmena/skills/crdt-doc-formulas/`.

#### Tool result shape changes

`crdt_doc_set_cell` returns:

```json
{
  "ok": true,
  "cells_recalculated": 3,
  "warnings": [
    {"kind": "needs_browser",  "addr": "B1", "functions": ["XLOOKUP"]},
    {"kind": "eval_error",     "addr": "C1", "error": "#DIV/0!"},
    {"kind": "cycle",          "chain": [["Sheet1","A1"], ["Sheet1","B1"]]},
    {"kind": "parse_error",    "addr": "D1", "error": "..."}
  ]
}
```

`crdt_doc_set_range` aggregates the same warnings across the batch +
`total_cells_recalculated`.

`crdt_doc_read(include_formulas: true)` cells become objects:

```json
{
  "cells": {
    "A1": {"v": 5},
    "B1": {"v": 10, "f": "=A1*2", "fs": "be"}
  }
}
```

Cells with no formula stay as `{v}` only.

`crdt_doc_list_sheets` each sheet entry adds `formula_count: <integer>`.

#### 5.8.1 Frontend integration contract (for ADP frontend team)

El backend escribe cada celda como `{v, t, f?, fs?}`. Cualquier frontend que
sincronice contra el yrs Doc vía el protocolo WS sync v1 **DEBE preservar
`f` y `fs`** en cualquier escritura de celda que no sea un reemplazo
literal explícito (por el usuario) de la fórmula. No preservarlos rompe el
cascade de recalc — el observer en `crdt_documents::recalc_observer` (D-T15)
depende de que `f` esté presente en el yrs Doc para identificar dependientes.

Los valores de `fs` son:
- `"be"` — evaluado por el backend (`formula_engine` vía formualizer).
- `"fe"` — evaluado por el frontend (Univer u equivalente).
- `"needs_browser"` — función no soportada por el backend; el frontend
  debe evaluar y escribir el `v` computado más `fs="fe"`.

Para Univer específicamente, el demo estático en
`src/libs/colmena/src/crdt_documents/static/index.html` usa
`SET_RANGE_VALUES_MUTATION` para el inbound (yrs → Univer). Esto funciona
visualmente pero **NO registra la fórmula con `UniverFormulaEnginePlugin`**,
así que el cascade client-side y la preservación round-trip están rotos.
Ver entrada de BACKLOG "Univer ↔ yrs formula round-trip" para los tres
candidatos de fix.

El protocolo es:
- **WS sync** para datos de celda (yjs sync v1).
- **REST endpoints** para projection (`GET /documents/:id/projection.json`),
  artifact CRUD, change events.
- **HTTP POST `/documents/:id/import-sheet`** para imports de xlsx.

La superficie de tools que ve el agente (`crdt_doc_*`) está documentada en
`docs/node_as_tools_reference.json` y nunca cruza el límite WS — los agentes
hablan in-proc con el runtime vía `apply_set_cell_in_proc` y compañía.

---

## 6. Python helper (PyO3 + pandas)

El módulo nativo `colmena.documents` se expone vía bindings PyO3 cuando se compila con `--features python`:

```python
from colmena import documents as native
sheets = native.list_sheets("art_01H...")  # [{"sheet_id": "...", "name": "..."}]
```

Y un wrapper de conveniencia con pandas en `python/colmena_documents/`:

```python
import colmena_documents as cd
import pandas as pd

# Crear sheet + escribir DataFrame
sheet_id = cd.add_sheet("art_01H...", "Calculations")
df = pd.DataFrame({"Product": ["Apple", "Pear"], "Qty": [10, 20]})
cd.write_sheet("art_01H...", sheet_id, df)

# Leer como DataFrame (primera fila = header)
df = cd.read_sheet("art_01H...", sheet_id)
df["Total"] = df["Qty"] * 2
cd.write_sheet("art_01H...", sheet_id, df, mode="replace")
```

El binding requiere un **tokio runtime activo** en el proceso Python — se obtiene automáticamente cuando se ejecuta vía `colmena` CLI o con `maturin develop` + pytest.

### Instalación dev

```bash
.venv/bin/pip install maturin pandas python-ulid pytest
.venv/bin/maturin develop --features python
.venv/bin/pytest python/tests/test_crdt_documents_roundtrip.py -v
```

---

## 7. Persistencia

### Layout en disco (localfs)

```
{storage_root}/
  {artifact_id}/
    meta.json          # { artifact_id, name, created_at, updated_at, sheet_count }
    state.yjs          # Y.encodeStateAsUpdate (binary)
```

- Default `storage_root`: `.colmena/crdt_documents`.
- Override: env `COLMENA_CRDT_DOCUMENTS_STORAGE_ROOT` (para PyO3) o el campo `storage_root` del config block.

> ⚠️ **El default es RELATIVO al cwd.** Si arrancás `cargo run --bin dag_engine -- crdt-yws` desde un directorio y la siguiente vez desde otro, vas a perder acceso a los artifacts persistidos (el `DocRegistry::load_from_disk` no los va a encontrar). Síntoma típico: pegabas `?artifact=art_…` en el browser y veías sheets vacías o el agente devolvía `sheets: []` aunque "ayer funcionaba". Soluciones:
> - Arrancar siempre desde el repo root (`cd ~/proyectos/colmena && cargo run …`).
> - O pasar un path absoluto: `--dump-dir /Users/me/colmena_data`.
> - Verificar al arranque: el server imprime `storage → <abs path>` y `loaded → N artifact(s) from disk`. Si N=0 y esperabas más, el path está mal.

### Snapshot writer

- Per-artifact tokio task.
- Triggers: `notify()` post-mutación + tick periódico cada **5 segundos**.
- Coalesce: si dirty flag está `true`, flush en el siguiente tick o notify.
- Graceful shutdown: oneshot channel que `DocRegistry::delete` espera antes de borrar storage.

### Backend GCS (feature flag)

```toml
# Compile con --features gcs
[dependencies.colmena_dag_engine]
features = ["gcs"]
```

Config:

```json
{ "storage_backend": "gcs", "gcs_bucket": "my-bucket", "gcs_prefix": "colmena/crdt_documents" }
```

⚠️ GCS backend **stub para v1** — la implementación efectiva queda para v1.1 cuando se necesite.

---

## 8. Frontend (Univer + y-websocket)

`crdt_documents/static/index.html` carga Univer 0.2.10 desde CDN (`esm.sh`) + y-websocket 1.5.4. El bridge bidireccional traduce entre Univer's command bus (`sheet.mutation.set-range-values`) y el Y.Doc:

- **Outbound** (Univer → Y.Doc): subscribirse al command bus; al detectar `SetRangeValues`, escribir las celdas al Y.Map.
- **Inbound** (Y.Doc → Univer): `observeDeep` por sheet, dispatchar `SetRangeValues` syncExecuteCommand al canvas.
- **Feedback-loop guard**: flag `applyingFromYDoc` que el outbound respeta para no re-emitir mutaciones del inbound.
- **Initial replay**: al conectar a un artifact con celdas existentes, walk del Y.Map y dispatch single `SetRangeValues` para que Univer renderice.

### Multi-sheet

Univer mostraría las tabs automáticamente cuando el initialState tiene `sheetOrder` y `sheets`. Los IDs del Y.Doc (`sh_<ulid>`) se usan como `subUnitId` de Univer, así que los bridges enrutan correctamente entre sheets.

### Página `/minimal` (sin Univer)

`crdt_documents/static/minimal.html` es una página de diagnóstico que NO carga Univer — solo y-websocket + un input HTML. Útil para aislar bugs de transporte vs. de canvas.

---

## 9. CLI

```bash
# Levantar server
cargo run --bin dag_engine -- crdt-yws --port 8090 --dump-dir .colmena/crdt_documents

# Agente diagnóstico (vía WS, como peer Yjs)
cargo run --bin dag_engine -- crdt-agent ws \
  --url ws://localhost:8090/documents/<id>/yjs \
  --sheet <sheet_id> --addr A1 --value "hola"

# Agente diagnóstico (in-proc HTTP)
cargo run --bin dag_engine -- crdt-agent inproc \
  --base-url http://localhost:8090 \
  --artifact <id> --sheet <sheet_id> --addr A1 --value "hola"
```

---

## 10. Limitaciones de v1 — qué falta

| Tema | Status | Cuándo |
|---|---|---|
| Auth con ADP | OUT | Cuando ADP integre |
| Formato visual (fills, fonts, merges, formulas) | OUT | v1.1 — [BACKLOG.md](../BACKLOG.md) tiene plan concreto |
| Charts, pivot tables, formato condicional | OUT | v1.1 → v2 |
| Multi-cursor presence visual | OUT | v2 |
| Named versioning + rollback | OUT (solo snapshots) | v1.1 |
| Send-safe channel para WS Subscription | OUT (thread-spawn workaround) | v1.1 si escalamos a 100+ conexiones |
| Word, HTML, Google Sheets | OUT | v2 |
| Formula evaluation server-side | OUT | v1.1 si LLM/Python lo necesitan |
| WS narration por update (per-cell narration) | OUT — coarse "peer update (N bytes)" actual | v1.1 cuando refactor handle_socket capture pre-state |

---

## 11. Tests

| Categoría | Comando | Notas |
|---|---|---|
| Unit tests del módulo | `cargo test -p colmena_dag_engine --lib crdt_documents` | ~40 tests + 1 ignored (R2.1 benchmark) |
| Tools synthetic | `cargo test -p colmena_dag_engine --lib crdt_doc_tools` | 8 tests |
| R2.1 benchmark | `cargo test --lib crdt_documents::projection::tests::r2_1 -- --ignored --nocapture` | Mide p50 — debería ser <50ms |
| Convergencia multi-agente (WS + in-proc) | `cargo test --test crdt_documents_convergence_test` | 1 PASS |
| REST CRUD | `cargo test --test crdt_documents_rest_test` | 2 PASS |
| xlsx round-trip | `cargo test --test crdt_documents_xlsx_roundtrip_test` | 1 PASS |
| LLM tools E2E | `cargo test --test crdt_documents_llm_tools_test` | 1 PASS |
| Persistencia | `cargo test --test crdt_documents_persistence_test` | 1 PASS (toma ~7s, snapshot tick) |
| Python helper round-trip | `pytest python/tests/test_crdt_documents_roundtrip.py -v` | Requiere `maturin develop --features python` |

---

## 12. Referencias

- Spec v1: [`docs/superpowers/specs/2026-06-01-documents-crdt-v1-design.md`](../superpowers/specs/2026-06-01-documents-crdt-v1-design.md)
- Plan implementación: [`docs/superpowers/plans/2026-06-01-documents-crdt-v1.md`](../superpowers/plans/2026-06-01-documents-crdt-v1.md)
- Spike Fase 0 (verdict GO): [`docs/superpowers/specs/2026-05-31-documents-crdt-spike-design.md`](../superpowers/specs/2026-05-31-documents-crdt-spike-design.md) + [results](../superpowers/specs/2026-05-31-documents-crdt-spike-results.md)
- Backlog v1.1 (formato visual): [`docs/BACKLOG.md`](../BACKLOG.md)
- Librería `documents` legacy (§27): [`developer_guide/27_documents_library.md`](./27_documents_library.md)
- Univer: https://github.com/dream-num/univer
- yrs: https://crates.io/crates/yrs
- calamine: https://crates.io/crates/calamine
- rust_xlsxwriter: https://crates.io/crates/rust_xlsxwriter
