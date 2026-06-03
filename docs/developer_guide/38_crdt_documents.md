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
