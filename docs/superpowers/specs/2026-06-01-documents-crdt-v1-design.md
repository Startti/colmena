# Documents CRDT v1 — Design

**Date:** 2026-06-01
**Author:** daniel@startti.co (con asistencia AI)
**Estado:** Diseño aprobado — listo para writing-plans
**Duración objetivo:** 4-6 semanas
**Predecesor:** Spike Fase 0 cerrado con verdict GO ([results](2026-05-31-documents-crdt-spike-results.md))
**Working branch:** `feature/docs` (continúa desde el spike, sin merge intermedio a develop)

---

## 1. Contexto y motivación

El spike validó la arquitectura híbrida **Univer (browser) + Yrs (backend) + IR projection**. Los 7 criterios GO/NO-GO pasaron con holgura:

- R1.1 multi-peer convergence: <1s en LAN, automated + manual.
- R1.2 Univer trabaja sin su collab backend propio: cero requests a `@univerjs/collaboration-server`.
- R2.1 projection p50: **1.38ms** (vs threshold 50ms — 36× margen).
- R2.2 projection LoC: **79** (vs threshold 500 — 6× margen).
- R2.3 50 ediciones browser: capturadas en projection, JSON válido.
- R5.1 `.xlsx` import: 756 cells renderizadas via SheetJS + Univer.
- R5.2 projection captura valores: spot-checks match.

v1 transforma el spike — que vive 100% en memoria y solo cubre Excel single-sheet single-tab-flow — en un MVP demoable end-to-end del **flujo canónico** identificado en el brainstorm original:

> Usuario sube un Excel (o trae uno existente) → quiere procesarlo con AI + Python → agregar una hoja con cálculos → mezclar ediciones manuales del usuario + cambios pedidos al LLM + transformaciones por código → exportar.

## 2. Decisiones de alto nivel (aprobadas en brainstorm)

| Decisión | Valor | Razón |
|---|---|---|
| Lifecycle del artifact | **Workspace/project-level**, NO per-session | Soporta varios chats/agentes tocando el mismo doc; soporta colaboración humano-humano. |
| Auth | **Out of scope para colmena v1** | ADP es dueño de auth via proxy/token validation. Colmena trust caller, expone APIs limpias. |
| Persistencia | **Snapshot-only** (sin append-only log) | Suficiente para MVP; recovery aceptable con cadencia agresiva. v1.1 puede sumar log si el caso lo amerita. |
| Fuente de verdad | **Yjs state** (`Y.Doc` per artifact) | IR es derivada. Yjs state se persiste; IR se computa on-demand. |
| Frontend canvas | **Univer 0.2.10** | Validado en spike; ADP integra el componente directamente. |
| Backend CRDT | **yrs 0.26** con wire-format Yjs v1 implementado manualmente | y-sync 0.4 pina yrs 0.17 (incompatible); reusamos `yjs_protocol.rs` del spike. |
| Formula evaluation | **Out — Univer las calcula en browser; IR captura calculated value** | v1.1 puede sumar engine server-side si LLM/Python necesitan calcular antes que el browser. |
| Export xlsx | `rust_xlsxwriter` desde IR projection | Reusa el patrón del existing `documents/` module. Limitado a cells (sin fórmulas/formato/merges en v1). |
| Módulo Rust | **Rename `dag_engine::spike` → `dag_engine::crdt_documents`** | Reusa el código probado del spike; documentamos la coexistencia con el existing `documents/` module. |
| Coexistencia | **El existing `documents/` module se queda intacto** | Patches inmutables vs CRDT son arquitecturalmente distintos; no se intenta unificar en v1. |

## 3. Scope

### Dentro de v1

#### Foundation

- Persistencia en disco (localfs + GCS feature flag) con snapshot cada 5s + on shutdown + on last-disconnect.
- Reload de snapshots en startup.
- REST + WS API para gestión completa de artifacts.
- Multi-sheet en Y.Doc + projection + bridges (add/delete/rename/reorder).
- Importer `.xlsx` server-side via `calamine`.
- Exporter `.xlsx` server-side via `rust_xlsxwriter` (cells only).
- DocRegistry con atomic `get_or_create` (ya en spike).

#### LLM integration

- `llm_call.config.crdt_documents` block (mirroring existing `documents` config).
- 6 synthetic tools expuestos al LLM:
  - `crdt_doc_list_sheets()` — metadata de sheets en el artifact.
  - `crdt_doc_read(sheet_id, range?)` — celdas como JSON.
  - `crdt_doc_set_cell(sheet_id, addr, value)` — escribe una celda.
  - `crdt_doc_set_range(sheet_id, addr, values_2d)` — bulk write.
  - `crdt_doc_add_sheet(name)` → returns `sheet_id`.
  - `crdt_doc_get_recent_changes(since_event_id?)` — narración legible de cambios.
- LLM no ve CRDT internals; solo opera sobre el modelo IR.
- Tools mutan `Y.Doc` directamente in-proc (mismo patrón que `apply_set_cell_in_proc` del spike).

#### Python integration

- PyO3 bindings expuestos en el `python_node`:
  - `colmena.documents.read_sheet(artifact_id, sheet_id) -> pandas.DataFrame`
  - `colmena.documents.write_sheet(artifact_id, sheet_id, df, mode="replace"|"append")`
  - `colmena.documents.add_sheet(artifact_id, name) -> sheet_id`
  - `colmena.documents.list_sheets(artifact_id) -> list[dict]`
- Comparativos entre sheets (compare 2 sheets by column) usa pandas estándar — sin helpers especiales en v1.

#### Frontend

- Univer + y-websocket setup del spike, generalizado para multi-sheet y multi-artifact.
- Botones de import/export en toolbar.
- Tabs nativas de Univer para sheets.

#### Diff narration

- Server-side: buffer rotativo (hasta 1000 events) de Yjs updates con metadata `{timestamp, origin, summary}`.
- `crdt_doc_get_recent_changes(since)` retorna texto natural-language.

### Fuera de v1 (explícito)

| Tema | Por qué | Cuándo |
|---|---|---|
| Auth con ADP | ADP es dueño | Cuando ADP integre |
| Formula evaluation server-side | Univer ya las evalúa en browser; IR captura calculated values | v1.1 si LLM/Python necesitan |
| Multi-cursor presence visual | Nice-to-have, no bloquea flujo canónico | v2 |
| Versioning + rollback nombrado | Snapshots ya hay; named versions ≠ MVP | v1.1 |
| Charts, pivot tables, formato condicional, merges | Univer los soporta visualmente, IR no los captura todavía | v1.1 — plan concreto en [BACKLOG.md → "CRDT Documents v1.1 — formato visual en xlsx"](../../BACKLOG.md) |
| Send-safe channel para WS Subscription | Thread-spawn funciona para <100 conexiones | v1.1 si escalamos |
| Word, HTML, Google Sheets | Excel fue el flujo canónico priorizado | v2 |
| LLM streaming de cambios live (vs solo en get_recent_changes) | Patrón "pull" es suficiente para MVP | v1.1 |

## 4. Arquitectura

```
┌──────────────────────────────────────────────────────────────────────┐
│  ADP frontend (futuro, no en v1)                                     │
│  - Iframe / embed con Univer                                         │
│  - Auth proxy hacia colmena                                          │
└────────────┬─────────────────────────────────────┬───────────────────┘
             │                                     │
             │ HTTPS (REST)                        │ WSS (Yjs sync v1)
             │                                     │
┌────────────▼─────────────────────────────────────▼───────────────────┐
│  colmena dag_engine binary                                           │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ HTTP server (axum)                                            │   │
│  │  POST   /documents              GET  /documents               │   │
│  │  DELETE /documents/{id}                                       │   │
│  │  POST   /documents/{id}/import  (.xlsx upload)                │   │
│  │  GET    /documents/{id}/export.xlsx                           │   │
│  │  GET    /documents/{id}/projection.json                       │   │
│  │  WS     /documents/{id}/yjs     (Yjs sync)                    │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                  │                                   │
│                                  ▼                                   │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ DocRegistry  HashMap<ArtifactId, Arc<yrs::Doc>>               │   │
│  │   atomic get_or_create (DashMap.entry)                        │   │
│  │   load all from disk on startup                               │   │
│  │   spawn snapshot-writer task per artifact                     │   │
│  └──────────────────────────────────────────────────────────────┘   │
│         │                          │                  │              │
│         ▼                          ▼                  ▼              │
│  ┌──────────────┐         ┌─────────────────┐  ┌─────────────────┐  │
│  │ projection   │         │ snapshot writer │  │ change tracker  │  │
│  │ Yrs → IR     │         │ → localfs / GCS │  │ for narration   │  │
│  │ JSON         │         │ every 5s        │  │ buffer 1000 evs │  │
│  └──────────────┘         └─────────────────┘  └─────────────────┘  │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ llm_call node                                                 │   │
│  │   config.crdt_documents block                                 │   │
│  │   → registers 6 synthetic tools                               │   │
│  │   → tools mutate DocRegistry's Y.Doc directly in-proc         │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ python_node                                                   │   │
│  │   colmena.documents module (PyO3)                             │   │
│  │   → read_sheet, write_sheet, add_sheet, list_sheets          │   │
│  └──────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────┘
```

### Mapping desde el spike

Spike actual: `src/libs/colmena/src/dag_engine/spike/`
v1 destino: `src/libs/colmena/src/crdt_documents/` (root del crate, mismo nivel que el existing `documents/` module — ver §11).

| Spike (`dag_engine::spike::*`) | v1 (`crdt_documents::*`) | Cambio |
|---|---|---|
| `doc_registry.rs` | `doc_registry.rs` | + `load_from_disk`, `start_snapshot_writer`, per-artifact metadata |
| `projection.rs` | `projection.rs` | + Multi-sheet output (iterate all sheets, not just `[0]`) |
| `yjs_protocol.rs` | `yjs_protocol.rs` | Sin cambios mayores; quizás fix awareness gaps menores |
| `server.rs` | `server.rs` | Routes renombrados de `/yjs/:artifact` → `/documents/:id/yjs`; añadir REST endpoints |
| `agent_peer.rs` | renombrado `tool_executor.rs` | Expone funciones públicas para los LLM tools (in-proc) |
| `static/index.html`, `static/minimal.html` | mantenidos como dev tools en `static/` | Producción usará el embed en ADP |
| `static/index.html` Univer bootstrap | Convertir a componente reusable que ADP pueda embeber | Quizás bundle con Vite si esm.sh sigue siendo frágil |

Los nodos del dag_engine (`crdt_documents_*`) van bajo `src/libs/colmena/src/dag_engine/infrastructure/nodes/` siguiendo el patrón existente.

## 5. APIs

### REST endpoints

#### `POST /documents`

Crear nuevo artifact vacío.

**Body:**
```json
{ "name": "Q3 Sales Report" }
```

**Response 201:**
```json
{ "artifact_id": "art_01H...", "created_at": "2026-06-01T..." }
```

#### `GET /documents`

Listar artifacts conocidos.

**Response 200:**
```json
{ "artifacts": [{ "artifact_id": "...", "name": "...", "updated_at": "...", "sheet_count": 3 }] }
```

#### `DELETE /documents/{id}`

Borra artifact + snapshot.

**Response 204.**

#### `POST /documents/{id}/import`

Sube un `.xlsx` que reemplaza el state actual del artifact. Multipart upload con campo `file`.

**Response 200:**
```json
{ "sheets_imported": 3, "cells_imported": 1247 }
```

Implementación: `calamine` lee xlsx → walk cells/sheets → escribe al `Y.Doc` en una sola transacción → triggers snapshot.

#### `GET /documents/{id}/export.xlsx`

Genera xlsx desde IR projection actual.

**Response 200:** `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet` bytes.

Implementación: project Y.Doc → walk sheets → `rust_xlsxwriter` → emit bytes.

#### `GET /documents/{id}/projection.json`

Retorna IR JSON actual. Mismo formato que el spike pero multi-sheet:

```json
{
  "artifact_id": "...",
  "sheets": [
    { "id": "s1", "name": "Sales", "cells": { "A1": "Product", "B1": "Qty" } },
    { "id": "s2", "name": "Summary", "cells": { ... } }
  ]
}
```

#### `WS /documents/{id}/yjs`

Yjs sync v1 protocol endpoint. Idéntico al `/yjs/:artifact` del spike, solo cambia el path.

### LLM tools (synthetic, inyectados via `llm_call.config.crdt_documents`)

#### `crdt_doc_list_sheets()`

```json
[{ "sheet_id": "s1", "name": "Sales" }, { "sheet_id": "s2", "name": "Summary" }]
```

#### `crdt_doc_read(sheet_id, range?)`

`range` es opcional, formato `"A1:D10"`. Sin range → toda la sheet.

```json
{
  "sheet_id": "s1",
  "range": "A1:B2",
  "cells": { "A1": "Product", "A2": "Apple", "B1": "Qty", "B2": 10 }
}
```

#### `crdt_doc_set_cell(sheet_id, addr, value)`

Sets una celda. Value puede ser string | number | boolean | null (null = delete).

#### `crdt_doc_set_range(sheet_id, start_addr, values_2d)`

Bulk write. `values_2d` es array de arrays (row-major).

```json
{
  "sheet_id": "s1",
  "start_addr": "A1",
  "values_2d": [["Product", "Qty"], ["Apple", 10], ["Pear", 20]]
}
```

#### `crdt_doc_add_sheet(name)`

```json
{ "sheet_id": "s3" }
```

#### `crdt_doc_get_recent_changes(since_event_id?)`

Si no se pasa `since_event_id`, retorna last 50 events. Si se pasa, retorna desde ese punto.

```json
{
  "since_event_id": "evt_42",
  "current_event_id": "evt_50",
  "narration": "User edited Sales!A1 from 'Product' to 'SKU'. Agent 'planner' added sheet 'Calculations'. 5 cells in column B of Sales were updated by user."
}
```

### Python helper API

Importable desde `python_node`:

```python
from colmena.documents import read_sheet, write_sheet, add_sheet, list_sheets

# Read sheet as pandas DataFrame.
df = read_sheet(artifact_id="art_01H...", sheet_id="s1")

# Mutate with pandas.
df["Total"] = df["Qty"] * df["Price"]

# Write back. `mode="replace"` overwrites existing cells; "append" keeps them.
write_sheet(artifact_id="art_01H...", sheet_id="s1", df=df, mode="replace")

# Create a new sheet from a DataFrame.
new_sheet_id = add_sheet(artifact_id="art_01H...", name="Calculations")
write_sheet(artifact_id="art_01H...", sheet_id=new_sheet_id, df=summary_df)

# List sheets.
sheets = list_sheets(artifact_id="art_01H...")
```

Implementación: PyO3 bindings que llaman a las mismas funciones in-proc que los LLM tools usan. Comparten lockless access al `DocRegistry`.

## 6. Persistencia

### Storage layout (localfs)

```
{storage_root}/
  documents/
    {artifact_id}/
      meta.json        # { name, created_at, updated_at, sheet_count }
      state.yjs        # Y.encodeStateAsUpdate output (binary)
      change_log.json  # buffer of last N events for diff narration
```

Configurable via `--storage-root` CLI flag o env `COLMENA_DOCUMENTS_STORAGE_ROOT`. Default: `./.colmena/crdt_documents`.

### Storage layout (GCS, feature flag `gcs`)

```
gs://{bucket}/{prefix}/{artifact_id}/meta.json
gs://{bucket}/{prefix}/{artifact_id}/state.yjs
```

### Snapshot writer

- Per-artifact tokio task que escucha:
  - `notify`/`tokio::sync::Notify` que se dispara por cada mutación.
  - Tick periódico cada 5 segundos.
  - Channel de shutdown.
- Coalesce: si llegan varias notifs en menos de 5 segundos, solo se snapshota una vez (siguiente tick o on-demand).
- Triggers explícitos: graceful shutdown + on last-client-disconnect.
- Atomic writes: write a temp file (`state.yjs.tmp`) + `rename` → `state.yjs`.
- Dirty bit por artifact para evitar snapshots cuando no hubo mutaciones.

### Reload on startup

- Walk `{storage_root}/documents/` directory.
- For each artifact: read `state.yjs` → `Y.applyUpdate` on a fresh `Doc` → register.

## 7. Multi-sheet

Y.Doc structure unchanged from spike:

```
Y.Doc
  Y.Map "workbook"
    Y.Array "sheets"
      Y.Map { id, name, cells: Y.Map<addr, Y.Map{v, t}> }
```

v1 adds:
- `apply_add_sheet(doc, name) -> sheet_id` mutation function.
- `apply_delete_sheet(doc, sheet_id) -> ()`.
- `apply_rename_sheet(doc, sheet_id, new_name)`.
- `apply_reorder_sheets(doc, new_order: Vec<String>)`.

Projection iterates all sheets:

```json
{
  "sheets": [
    { "id": "s1", "name": "Sales", "cells": {...} },
    { "id": "s2", "name": "Summary", "cells": {...} }
  ]
}
```

Frontend: Univer's `sheetOrder` array in `initialState` is populated from `sheets[*].id`. Inbound bridge dispatches the right `unitId/subUnitId` based on which sheet's `cells` Y.Map fired the observer.

## 8. xlsx round-trip

### Import (`POST /documents/{id}/import`)

Server-side via `calamine` crate (already a Rust ecosystem standard for reading xlsx).

```rust
let workbook = calamine::open_workbook_auto(...)?;
let mut txn = doc.transact_mut();
let wb_map = txn.get_or_insert_map("workbook");
let sheets_arr = ensure_y_array(&mut txn, &wb_map, "sheets");
for sheet_name in workbook.sheet_names() {
    let range = workbook.worksheet_range(sheet_name)?;
    let sheet_id = generate_sheet_id();
    let sheet_map = sheets_arr.push_back(&mut txn, MapPrelim::default());
    sheet_map.insert(&mut txn, "id", sheet_id.as_str());
    sheet_map.insert(&mut txn, "name", sheet_name.as_str());
    let cells = sheet_map.insert(&mut txn, "cells", MapPrelim::default());
    for (row, col, cell) in range.cells() {
        let addr = format_a1(row, col);
        let cell_map = cells.insert(&mut txn, addr.as_str(), MapPrelim::default());
        cell_map.insert(&mut txn, "v", calamine_value_to_yrs_any(cell));
        cell_map.insert(&mut txn, "t", type_tag(cell));
    }
}
```

Limitación v1: formulas → reads calamine's pre-computed value if available; otherwise stores the formula string literal (browser will re-evaluate via Univer's formula engine when displayed).

### Export (`GET /documents/{id}/export.xlsx`)

Server-side via `rust_xlsxwriter` (already a Rust ecosystem standard, used by existing `documents/` module).

```rust
let projection = project(&doc);
let mut workbook = rust_xlsxwriter::Workbook::new();
for sheet in &projection["sheets"] {
    let ws = workbook.add_worksheet().set_name(sheet["name"])?;
    for (addr, value) in sheet["cells"].as_object().unwrap() {
        let (row, col) = parse_a1(addr);
        match value {
            Value::String(s) => ws.write_string(row, col, s)?,
            Value::Number(n) => ws.write_number(row, col, n.as_f64().unwrap())?,
            Value::Bool(b) => ws.write_boolean(row, col, *b)?,
            _ => continue,
        };
    }
}
workbook.save_to_buffer()?
```

Limitación v1: solo cells (no formulas, no formato, no merges). v1.1 puede sumar formatting si el IR lo captura.

## 9. Diff narration

### Server-side buffer

Cada `Y.Doc` tiene un `observer_update_v1` subscription que escribe a un buffer rotativo:

```rust
struct ChangeEvent {
    event_id: u64,       // monotonic
    timestamp: i64,      // unix millis
    origin: String,      // peer id from WS handshake, or "agent:<name>" for in-proc tools
    summary: String,     // pre-rendered "User added cell A1=hola in Sales" etc.
}

struct ChangeBuffer {
    events: VecDeque<ChangeEvent>,  // capacity 1000
    next_event_id: u64,
}
```

**Cómo se genera el `summary`:** el observer recibe el `update_v1` bytes + transaction origin. La implementación decodifica el update con `yrs::Update::decode_v1` y walk el delta para identificar mutaciones por nivel:

- Set de una celda → `"User edited <sheet>!<addr> to <new_value>"`.
- Add sheet → `"Origin added sheet '<name>'"`.
- Bulk de N cells en misma columna → `"<N> cells in column <col> of <sheet> updated by <origin>"`.

Threshold para agregación: si una transacción toca >5 cells en la misma columna/row, agregamos en un solo summary line. Si toca celdas dispersas, listamos hasta 5 cells individuales luego "and X more cells".

El `origin` viene del WS handshake (peer id derivado del primer `sync_step1` frame con un random) o del agent tool (`agent:<llm_call_node_id>`).

### LLM tool query

```json
{
  "since_event_id": "42",  // optional
  "current_event_id": "57",
  "narration": "Since the last check:\n- User edited cell A1 in Sales from 'Product' to 'SKU'.\n- Agent 'planner' added a new sheet 'Calculations'.\n- 5 cells in column B of Sales were updated by user."
}
```

El LLM puede pasar el `current_event_id` retornado como `since_event_id` en la próxima llamada para solo ver cambios incrementales.

## 10. Coexistencia con el existing `documents/` module

El existing `src/libs/colmena/src/documents/` module se queda **intacto**. Casos de uso:

| Caso | Módulo |
|---|---|
| LLM genera un Excel/Word/HTML one-shot, sin colaboración en tiempo real | Existing `documents/` (patches versionados) |
| Usuario colabora en real-time con LLM + Python sobre un Excel | New `crdt_documents/` (Yjs CRDT) |
| Documentos Word/HTML | Existing `documents/` por ahora; v2 puede migrar |

Documentamos esto en `docs/developer_guide/` con una nueva sección **"38. CRDT Documents (v1)"** que apunta a este spec.

## 11. Renombrar el módulo del spike

Spike: `src/libs/colmena/src/dag_engine/spike/`
v1: `src/libs/colmena/src/dag_engine/crdt_documents/` (en root del crate, no bajo `dag_engine/`).

Razón: el módulo crece más allá de "infraestructura del dag_engine" — tiene su propio dominio (artifacts, persistencia, importer/exporter). Espejea el patrón del existing `documents/` module que también vive en root.

Subcomandos CLI:
- `dag_engine crdt-yws` (renombrado de `spike-yws`) — server standalone.
- `dag_engine crdt-agent` (renombrado de `spike-agent`) — herramienta debug.

## 12. Testing strategy

| Layer | Tests |
|---|---|
| `projection.rs` | Unit tests (multi-sheet, empty doc, malformed cells) — extender los del spike. Benchmark R2.1 sigue siendo el smoke perf. |
| `yjs_protocol.rs` | Unit tests del spike (convergence + awareness skip) sin cambios mayores. |
| `doc_registry.rs` | Unit tests del spike + nuevos: `load_from_disk`, `snapshot_writer_persists`, `reload_round_trip`. |
| `server.rs` REST endpoints | Integration tests con axum's `oneshot` para cada endpoint. |
| Multi-peer WS | Reusar `spike_convergence_test.rs`, expandir a multi-sheet. |
| LLM tools | Rust integration test que arranca un `llm_call` node con mock provider y verifica que los tools mutan el `Y.Doc` correcto. |
| Python helper | Python tests en `python/tests/` con `.venv/bin/pytest`. |
| xlsx round-trip | Integration test: importar `spike/fixtures/test.xlsx` → verify cells → export → re-import → verify isomorphism. |
| Reload | Integration test: write, snapshot, restart server, verify state preserved. |

## 13. Plan de trabajo

Time-boxed a 4-6 semanas. Cada semana entrega algo demoable.

| Semana | Entrega | Demo de la semana |
|---|---|---|
| 1 | Module rename + persistencia + multi-sheet projection + REST CRUD (`POST/GET/DELETE /documents`) | Crear artifact via curl, ver listado, borrar, reload server preserva estado |
| 2 | xlsx import/export + WS endpoint `/documents/:id/yjs` + Univer frontend multi-sheet | Subir xlsx, abrir browser, ver multi-sheet, descargar xlsx |
| 3 | LLM tools (3-4 primeros: read, set_cell, set_range, list_sheets) + `crdt_documents` config block en `llm_call` | Graph con `llm_call` que setea celdas vía tool, visible en browser en real-time |
| 4 | LLM tools restantes (add_sheet, get_recent_changes) + Python helper (read_sheet, write_sheet) | python_node lee sheet como DataFrame, transforma, escribe; cambios visibles en browser |
| 5 | Diff narration (server-side tracker + LLM tool) + polish: error handling, edge cases | LLM hace cambios, usuario edita, LLM llama `get_recent_changes` y ve narración. |
| 6 | Testing exhaustivo + docs + cleanup + buffer para retrasos | Suite verde, docs publicados, demo end-to-end grabado |

## 14. Entregables

- Código bajo `src/libs/colmena/src/crdt_documents/` y `src/libs/colmena/src/dag_engine/infrastructure/nodes/crdt_documents_*` (los nodos LLM tools).
- PyO3 bindings expuestos.
- REST API documentada (vía `docs/developer_guide/38_crdt_documents.md`).
- Tests pasando.
- Demo end-to-end (graph JSON + xlsx) reproducible.
- Migration notes para cuando ADP integre.

## 15. Riesgos

| Riesgo | Probabilidad | Mitigación |
|---|---|---|
| `calamine` no maneja bien xlsx complejos (merged, formato condicional) | Media | Acotar v1 a "leer cells", documentar limitaciones. v1.1 puede sumar formato. |
| `rust_xlsxwriter` export pierde fórmulas que el browser calculó | Alta | Documentar como limitación esperada de v1. Univer puede re-importar el export sin las fórmulas y el usuario las re-emite si quiere. |
| Snapshot cada 5s saturará disk I/O en alta concurrencia | Baja | Solo se snapshota si hubo mutaciones. v1.1 puede sumar throttling. |
| PyO3 bindings + tokio runtime interaction problems | Media | El spike ya valida pyo3+tokio coexistencia (python_script + axum). |
| LLM tools concurrent mutations corrupting state | Baja | CRDT semantics garantiza convergencia; per-artifact actor pattern si surge. |
| Coexistencia con existing `documents/` module confunde a operadores | Media | Docs claras + ejemplos. Ver §10. |
| Performance del IR projection en docs con miles de cells | Baja | R2.1 mostró 1.38ms en 1000 cells; v1.1 puede cachear projection si surge bottleneck. |
| Univer 0.2.10 quirks emergen con multi-sheet / import/export round-trips | Media | Iterar en frontend siguiendo el patrón del spike (DevTools-driven discovery). |

## 16. Referencias

- Spike spec: [2026-05-31-documents-crdt-spike-design.md](2026-05-31-documents-crdt-spike-design.md)
- Spike results: [2026-05-31-documents-crdt-spike-results.md](2026-05-31-documents-crdt-spike-results.md)
- Existing documents module: [docs/developer_guide/27_documents_library.md](../../developer_guide/27_documents_library.md)
- Univer: https://github.com/dream-num/univer
- yrs: https://crates.io/crates/yrs
- calamine: https://crates.io/crates/calamine
- rust_xlsxwriter: https://crates.io/crates/rust_xlsxwriter
