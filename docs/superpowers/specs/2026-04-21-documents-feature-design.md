# Documents Feature — Design Spec

**Fecha:** 2026-04-21
**Autor:** Daniel Garcia (con asistencia de Claude)
**Estado:** Draft — pending user review
**Scope:** v1 del módulo `documents/` de Colmena

---

## 1. Overview

Esta feature agrega a Colmena la capacidad de **generar y editar documentos Word y Excel como artefactos**, con estas propiedades clave:

- **Edición granular por segmentos** mediante un protocolo de patches tipados (no regeneración completa)
- **Source of truth en JSON** (IR — Intermediate Representation); los binarios `.xlsx`/`.docx` se renderizan on-demand
- **Versionado inmutable** con retention configurable
- **Concurrencia agent ↔ user** mediante optimistic concurrency + auto-rebase
- **Dual storage** backend: LocalFS y Google Cloud Storage, seleccionable por config
- **Integración dual surface**: tools LLM + nodos DAG sobre el mismo core
- **Documentación rica** del schema para el LLM vía JSON Schema + skill built-in

El feature no introduce dependencia a LibreOffice en el hot path; usa librerías Rust nativas OOXML (`rust_xlsxwriter`, `docx-rs`).

---

## 2. Goals & Non-Goals

### Goals

- Permitir que un agente LLM cree un documento Excel o Word desde cero y lo edite por segmentos sin regenerarlo completo
- Soportar edición paralela: agente y usuario pueden editar el mismo documento sin perder cambios
- Inyectar a la conversación del agente los cambios recientes del usuario cuando el agente los consulta
- Exponer las capacidades uniformemente como tools LLM (synthetic) y como nodos DAG
- Permitir deployment tanto en entornos locales (FS) como en GCP (bucket)
- Documentar el schema de forma que el LLM pueda usarlo correctamente sin ambigüedad

### Non-Goals (v1)

- No soporta ingesta de documentos existentes (subir un `.xlsx` y editarlo) — closed loop v1
- No soporta edición colaborativa multi-humano en vivo (Google Docs style) — requiere CRDT/OT
- No soporta features avanzadas: imágenes, charts, pivot tables, formato condicional, merged cells, tracked changes, comentarios, footnotes, macros, headers/footers de página
- No soporta export a PDF en v1
- No incluye frontend; se asume que el integrador usa Univer/Luckysheet/Tiptap/OnlyOffice o similar

Todos estos quedan como **v2+ roadmap**, habilitados por decisiones de diseño v1 sin refactor estructural.

---

## 3. Scope

### In Scope v1

| Área | Incluido |
|---|---|
| Formatos | Excel (`.xlsx`), Word (`.docx`) |
| Tipos de bloque Word | heading, paragraph, list, table, runs con estilos inline básicos |
| Tipos de objeto Excel | cells (value/type/format/style), named tables, column widths, named styles |
| Storage | LocalFS + GCS (dual), seleccionable por config |
| Concurrencia | Escenario A: agent + single user interleaved, con auto-rebase server-side |
| Versioning | Inmutable, retention configurable con `pin_initial` |
| Session indexing | Tabla en DB de memoria existente (SQLite/Postgres) |
| Integration | Synthetic tools LLM + nodos DAG |
| Documentation | Schemars-derived JSON Schema + skill built-in `document_authoring` |

### Out of Scope v1 (Roadmap v2+)

- Imágenes inline (bloque `image` + blobs binarios)
- Merged cells
- Formato condicional
- Charts (Excel, Word)
- Pivot tables
- Round-trip de documentos existentes (parser OOXML → IR)
- Export a PDF (via LibreOffice headless como adapter opcional)
- Escenario B (CRDT/OT real-time multi-user collab)
- Event bus / push automático de user edits al LLM (v1 usa pull explícito)

---

## 4. Architecture

Nuevo módulo `documents/` al lado de `llm/` y `dag_engine/`, siguiendo el layout hexagonal del resto del repo.

```
src/libs/colmena/src/documents/
  domain/
    mod.rs
    error.rs                      # DocumentError, StorageError
    ir.rs                         # ExcelIR, WordIR, Block, Run, Cell, Sheet, ...
    patch.rs                      # Patch, PatchOp (enum tagged)
    ports.rs                      # ArtifactStore, IRRenderer, IRValidator,
                                  # ConflictDetector, SessionArtifactIndex
    artifact.rs                   # Artifact, ArtifactMeta, Version, VersionId
    session.rs                    # SessionId re-exported from llm::domain

  application/
    mod.rs
    create_document.rs            # CreateDocumentUseCase
    apply_patch.rs                # ApplyPatchUseCase (incluye rebase)
    read_document.rs              # ReadDocumentUseCase (con slice)
    list_versions.rs              # ListVersionsUseCase
    rollback.rs                   # RollbackUseCase
    get_head.rs                   # GetHeadUseCase (incluye summary desde last_seen)
    list_artifacts.rs             # ListArtifactsBySessionUseCase
    download_artifact.rs          # DownloadArtifactUseCase (sec 11.4)
    rebase_service.rs             # RebaseService
    diff_service.rs               # IR diff + narración natural-language

  infrastructure/
    mod.rs
    storage/
      mod.rs
      config.rs                   # DocumentStorageConfig enum + factory
      local_fs_store.rs           # LocalFsStore adapter
      gcs_store.rs                # GcsStore adapter
    render/
      excel_renderer.rs           # IR → .xlsx via rust_xlsxwriter
      word_renderer.rs             # IR → .docx via docx-rs
    validation/
      excel_validator.rs
      word_validator.rs
    conflict/
      excel_conflict_detector.rs
      word_conflict_detector.rs
    diff/
      ir_diff_engine.rs
      narration_templates.rs
    index/
      sqlite_session_artifact_index.rs
      postgres_session_artifact_index.rs
      in_memory_session_artifact_index.rs
      index_factory.rs
```

Wrappers (en `dag_engine/infrastructure/`):

```
nodes/
  document_create.rs
  document_edit.rs
  document_read.rs
llm_synthetic_tools/
  document_tools.rs   # document_create, document_apply_patch, document_read,
                      # document_get_head, document_list_versions,
                      # document_rollback, document_list_my_artifacts
```

**Reglas arquitectónicas:**
- `domain/` tiene cero dependencias de infrastructure
- Todo I/O pasa por traits en `domain/ports.rs`
- Nodos DAG y synthetic tools delegan a use cases de `application/`
- `thiserror` para errores de dominio, `anyhow` permitido en infrastructure

---

## 5. IR Schema

### 5.1 Common envelope

Todos los IRs llevan:

```json
{
  "kind": "excel" | "word",
  "artifact_id": "art_<ulid>",
  "version_id": "v<N>",
  "schema_version": "1.0.0"
}
```

`schema_version` se usa para futuras migraciones. IDs prefijados para legibilidad en logs/diffs:
- `art_*` — artifact
- `sheet_*` — sheet (Excel)
- `tbl_*` — named table
- `blk_*` — block (Word)
- `run_*` — run (Word)
- `row_*` — table row (Word)
- `li_*` — list item (Word)

IDs stable: sobreviven reorderings, renames y edits sobre otros elementos.

**Scope de uniqueness de IDs:**
- `artifact_id`, `sheet_id`, `tbl_*`, `blk_*` — **globales** dentro del documento (deben ser únicos a lo largo de todo el IR)
- `run_*` — **scoped al bloque que los contiene** (puede haber `run_01` en `blk_01` y otro `run_01` en `blk_02`; se direccionan siempre como `(block_id, run_id)`)
- `row_*`, `li_*` — scoped a su table/list block
- `IRValidator` enforza esto: IDs globales duplicados fallan; IDs scoped duplicados dentro de su scope fallan

### 5.2 Excel IR

```json
{
  "kind": "excel",
  "artifact_id": "art_abc123",
  "version_id": "v3",
  "schema_version": "1.0.0",
  "workbook": {
    "sheets": [
      {
        "id": "sheet_01",
        "name": "Ventas",
        "order": 0,
        "columns": [
          {"index": 0, "width": 14},
          {"index": 1, "width": 20}
        ],
        "cells": {
          "A1": {"value": "Producto", "type": "string", "style_ref": "style_header"},
          "B1": {"value": "Monto", "type": "string", "style_ref": "style_header"},
          "A2": {"value": "Widget", "type": "string"},
          "B2": {"value": 1500, "type": "number", "format": "#,##0"}
        },
        "tables": [
          {
            "id": "tbl_sales",
            "name": "Sales",
            "range": "A1:B10",
            "header_row": true,
            "style_preset": "medium_blue"
          }
        ]
      }
    ],
    "named_styles": {
      "style_header": {
        "font": {"bold": true, "size": 12, "color": "000000"},
        "fill": "EEEEEE"
      }
    }
  }
}
```

**Cell types**: `string`, `number`, `boolean`, `date`, `formula`. `value` es siempre `serde_json::Value`; el type indica cómo interpretarlo. Para `formula`, `value` es un string literal que empieza con `=`.

**Addressing**:
- Sheet: `sheet_id`
- Cell: `(sheet_id, a1_address)` ej. `("sheet_01", "B5")`
- Range: `(sheet_id, range_notation)` ej. `("sheet_01", "A1:C10")`
- Table: `(sheet_id, table_id)`

### 5.3 Word IR

```json
{
  "kind": "word",
  "artifact_id": "art_xyz789",
  "version_id": "v3",
  "schema_version": "1.0.0",
  "document": {
    "blocks": [
      {
        "id": "blk_01",
        "type": "heading",
        "level": 1,
        "runs": [
          {"id": "run_01", "text": "Informe Q3", "bold": true}
        ]
      },
      {
        "id": "blk_02",
        "type": "paragraph",
        "runs": [
          {"id": "run_01", "text": "En este trimestre "},
          {"id": "run_02", "text": "superamos", "bold": true},
          {"id": "run_03", "text": " las metas."}
        ]
      },
      {
        "id": "blk_03",
        "type": "list",
        "style": "bullet",
        "items": [
          {"id": "li_01", "runs": [{"id": "run_01", "text": "Ventas +20%"}]},
          {"id": "li_02", "runs": [{"id": "run_01", "text": "Costos -5%"}]}
        ]
      },
      {
        "id": "blk_04",
        "type": "table",
        "rows": [
          {
            "id": "row_01",
            "cells": [
              {"runs": [{"id": "run_01", "text": "Q1"}]},
              {"runs": [{"id": "run_01", "text": "100"}]}
            ]
          },
          {
            "id": "row_02",
            "cells": [
              {"runs": [{"id": "run_01", "text": "Q2"}]},
              {"runs": [{"id": "run_01", "text": "120"}]}
            ]
          }
        ]
      }
    ],
    "named_styles": {}
  }
}
```

**Block types v1**: `heading` (level 1–6), `paragraph`, `list` (bullet/numbered), `table`.

**Run properties v1**: `text` (required), `bold`, `italic`, `underline`, `size`, `color` (hex string).

**Addressing**:
- Block: `block_id`
- Run in paragraph/heading: `(block_id, run_id)`
- Run in table cell: `(table_block_id, row_id, col_index, run_id)`
- Run in list item: `(list_block_id, item_id, run_id)`

**Nota sobre run IDs**: los runs usan IDs estables (no índices posicionales). Esto habilita rebase trivial bajo inserts/deletes de otros runs.

### 5.4 Validation

Un `IRValidator` trait-based verifica invariantes por formato. Fallos comunes:

- IDs duplicados dentro del documento
- Referencias a `style_ref` inexistente en `named_styles`
- Cell `type: "number"` con `value` no-numérico
- Table `range` fuera de los bounds válidos de A1 notation
- Block `heading` con `level` fuera de 1..=6
- Runs vacíos (text empty) — warning, no error
- Circular references — no aplica en v1 (no hay referencias entre bloques)

Validación estructural ejecuta **antes** de persistir una versión. IR inválido → `IRValidationFailed { path, reason }`, donde `path` es JSON-Pointer-style.

### 5.5 ID generation

IDs generados por un `IdGenerator` trait:
- Default: ULID-like corto (12 chars base32, más el prefijo semántico)
- Test: contador determinístico para reproducibilidad

---

## 6. Patch Protocol

### 6.1 Envelope

```json
{
  "artifact_id": "art_abc123",
  "base_version": "v3",
  "source": "agent" | "user",
  "ops": [ ... ]
}
```

- `base_version`: versión sobre la que el caller construyó el patch. Usado para conflict detection y auto-rebase.
- `source`: origen del patch. Solo los patches con `source: "user"` generan narración para el LLM (los del propio LLM no se renarran).

### 6.2 Aplicación atómica

El patch entero es atómico. Secuencia:

1. Validar sintácticamente cada op (JSON Schema)
2. Leer IR de `base_version` (o el actual si hay que rebasear)
3. Si `base_version < current_version`: invocar `RebaseService`
   - Si rebase exitoso (no conflicts): producir patch rebaseado y seguir
   - Si hay conflicts: devolver `VersionConflict` sin modificar nada
4. Aplicar ops secuencialmente sobre un clone del IR; si cualquier op falla, abortar todo
5. Validar el IR resultante con `IRValidator`
6. Renderizar binario con `IRRenderer`
7. Persistir versión nueva (`v{N+1}`) atómicamente vía `ArtifactStore`
8. Actualizar `current_version` en `SessionArtifactIndex`
9. Si `source: "user"`, generar narración y guardarla en el metadata de la versión

### 6.3 Excel ops

| Op | Args | Semántica |
|---|---|---|
| `set_cell` | `sheet_id, address, value, value_type?, format?, style_ref?` | Set/create cell |
| `set_range` | `sheet_id, range, values: [[...]], value_types?` | Bulk write 2D array |
| `clear_range` | `sheet_id, range` | Remove cells in range |
| `insert_row` | `sheet_id, before_row, values?` | Insert row, shift down |
| `delete_row` | `sheet_id, row_index` | Delete row, shift up |
| `insert_column` | `sheet_id, before_col, values?` | Insert column, shift right |
| `delete_column` | `sheet_id, col_index` | Delete column, shift left |
| `add_sheet` | `name, at_index?` | Create new sheet |
| `rename_sheet` | `sheet_id, new_name` | Rename (ID stable) |
| `delete_sheet` | `sheet_id` | Remove sheet |
| `reorder_sheets` | `order: [sheet_id, ...]` | Reorder sheets |
| `create_table` | `sheet_id, range, name, header_row?, style_preset?` | Define named table |
| `resize_table` | `table_id, new_range` | Change table extent |
| `delete_table` | `table_id` | Remove table (cells persist) |
| `set_column_width` | `sheet_id, col, width` | Set column width |
| `define_style` | `style_ref, definition` | Create/update named style |

### 6.4 Word ops

| Op | Args | Semántica |
|---|---|---|
| `insert_block` | `{before?, after?}, block` | Insert block at position |
| `delete_block` | `block_id` | Remove block |
| `replace_block` | `block_id, block` | Replace block content entirely |
| `move_block` | `block_id, after_block_id` | Reorder |
| `set_heading_level` | `block_id, level` | Change heading level |
| `replace_run_text` | `block_id, run_id, new_text` | Update run text |
| `set_run_style` | `block_id, run_id, style_patch` | Update run style props |
| `insert_run` | `block_id, at_index, run` | Insert run at index |
| `delete_run` | `block_id, run_id` | Remove run |
| `insert_list_item` | `list_block_id, at_index, runs` | Insert item |
| `replace_list_item` | `list_block_id, item_id, runs` | Replace item runs |
| `delete_list_item` | `list_block_id, item_id` | Remove item |
| `insert_table_row` | `table_block_id, {before?, after?}, cells` | Insert row |
| `delete_table_row` | `table_block_id, row_id` | Remove row |
| `update_table_cell` | `table_block_id, row_id, col_index, runs` | Replace cell runs |
| `define_style` | `style_ref, definition` | Named style |

Ops con mix de ID-based y position-based inputs: `insert_run` y `insert_table_row` aceptan `at_index`/`before`/`after` para creación (no hay ID antes de insertar). Una vez creado, el resto de ops sobre ese run/row usan IDs.

### 6.5 Schema generation y documentation

`PatchOp` se define como un enum Rust con `#[derive(Serialize, Deserialize, JsonSchema)]` y `#[serde(tag = "op")]`. Cada variante y field lleva `#[schemars(description = "...")]` con texto que explica:

- **Op**: cuándo usar vs otras ops similares
- **Fields**: qué representa, restricciones de formato (ej. "A1-style cell address"), valores válidos

El JSON Schema generado se expone como input schema del synthetic tool `document_apply_patch`. El LLM ve todas las descriptions.

---

## 7. Storage & Versioning

### 7.1 Layout (idéntico para LocalFS y GCS)

```
{storage_root}/artifacts/{artifact_id}/
  meta.json
  HEAD
  versions/
    v1/
      ir.json
      render.xlsx   (o .docx)
      patch_applied.json
      blobs/           (vacío v1; reservado para imágenes/charts v2+)
    v2/
    v3/
```

En GCS, el `storage_root` se compone como `gs://{bucket}/{prefix}`, y los "archivos" son objetos GCS con las keys correspondientes.

### 7.2 `meta.json`

```json
{
  "artifact_id": "art_abc123",
  "kind": "excel",
  "created_at": "2026-04-21T14:00:00Z",
  "updated_at": "2026-04-21T14:35:00Z",
  "current_version": "v7",
  "retention_limit": 20,
  "pin_initial": true,
  "schema_version": "1.0.0",
  "session_id": "sess_789",
  "label": "Informe Q3 Ventas",
  "tags": {}
}
```

### 7.3 `HEAD`

Archivo (u objeto GCS) con un único string: `v{N}`. Update atómico:
- LocalFS: write-to-temp + rename
- GCS: PUT con `ifGenerationMatch` para CAS (Escenario C)

### 7.4 `patch_applied.json`

```json
{
  "patch": {
    "artifact_id": "art_abc123",
    "base_version": "v6",
    "source": "user",
    "ops": [...]
  },
  "applied_at": "2026-04-21T14:35:00Z",
  "resulted_in": "v7",
  "summary": {
    "natural_language": [
      "El usuario cambió la celda B5 en 'Ventas' de 1000 a 1500"
    ],
    "structured": [...]
  }
}
```

La versión `v1` (inicial, creada por `document_create`) tiene un `patch_applied.json` sintético con `ops: []` y `source: "agent" | "user"` según quién creó. Las creadas por `rollback` llevan un op sintético `{op: "rollback_from", target: "v3"}`.

### 7.5 Retention & pruning

- `retention_limit` en `meta.json` (default global `DOCUMENTS_DEFAULT_RETENTION`, valor inicial `20`)
- Tras cada commit exitoso, se ejecuta prune best-effort:
  - Listar versiones
  - Si `pin_initial: true`, proteger `v1`
  - Mantener las `retention_limit` más recientes
  - Eliminar el resto
- Prune falla → log warning, no afecta el commit
- Prune corre async sin bloquear el response

### 7.6 `ArtifactStore` trait

```rust
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn create_artifact(&self, meta: &ArtifactMeta) -> Result<(), StorageError>;

    async fn write_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
        data: &VersionData,
    ) -> Result<(), StorageError>;

    async fn read_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
    ) -> Result<VersionData, StorageError>;

    async fn read_current(&self, id: &ArtifactId) -> Result<VersionData, StorageError>;

    async fn list_versions(&self, id: &ArtifactId) -> Result<Vec<VersionId>, StorageError>;

    /// Atomic set-if-match. Returns VersionConflict if generation doesn't match.
    async fn set_head(
        &self,
        id: &ArtifactId,
        expected_current: Option<&VersionId>,
        new: &VersionId,
    ) -> Result<(), StorageError>;

    async fn delete_version(
        &self,
        id: &ArtifactId,
        version: &VersionId,
    ) -> Result<(), StorageError>;

    async fn read_meta(&self, id: &ArtifactId) -> Result<ArtifactMeta, StorageError>;

    async fn update_meta(&self, id: &ArtifactId, meta: &ArtifactMeta) -> Result<(), StorageError>;

    async fn delete_artifact(&self, id: &ArtifactId) -> Result<(), StorageError>;
}

pub struct VersionData {
    pub ir: serde_json::Value,
    pub rendered_binary: Vec<u8>,
    pub rendered_extension: &'static str,
    pub patch_applied: PatchApplied,
    pub blobs: Vec<(String, Vec<u8>)>,   // vacío en v1
}
```

### 7.7 Storage config y factory

```rust
pub enum DocumentStorageConfig {
    Local { root: PathBuf },
    Gcs {
        bucket: String,
        prefix: Option<String>,
        auth: GcpAuthConfig,
    },
}

pub enum GcpAuthConfig {
    ApplicationDefault,                           // ADC (env, metadata server)
    ServiceAccountFile(PathBuf),
    ServiceAccountJson(String),                   // contenido inline, para tests
}

pub fn build_artifact_store(cfg: &DocumentStorageConfig)
    -> Result<Arc<dyn ArtifactStore>, StorageError>;
```

Env vars soportadas:

| Var | Descripción | Default |
|---|---|---|
| `DOCUMENTS_STORAGE_BACKEND` | `local` \| `gcs` | `local` |
| `DOCUMENTS_STORAGE_ROOT` | Path raíz local | `./artifacts` |
| `DOCUMENTS_STORAGE_BUCKET` | Bucket GCS | (required si `gcs`) |
| `DOCUMENTS_STORAGE_PREFIX` | Prefix dentro del bucket | `colmena/artifacts` |
| `GOOGLE_APPLICATION_CREDENTIALS` | Path a SA JSON (estándar GCP) | ADC fallback |
| `DOCUMENTS_DEFAULT_RETENTION` | Retention por defecto | `20` |

### 7.8 GCS adapter

Usa crate `google-cloud-storage` (o equivalente maduro en tiempo de implementación). Características:

- **Auth**: ADC por defecto, respeta `GOOGLE_APPLICATION_CREDENTIALS`, también acepta SA file/JSON explícito
- **Atomic HEAD update**: PUT con `ifGenerationMatch={previous_generation}`; 412 Precondition Failed → `VersionConflict`
- **Retries**: backoff exponencial sobre 5xx/429 (5 reintentos, cap 30s)
- **Errors**: 403 y 404 se propagan directo
- **Content types**: se setea correctamente por tipo de archivo (`application/json` para IR, MIME OOXML para renders)

Tests de integración corren contra `fake-gcs-server` (docker) en CI.

---

## 8. Concurrency & Rebase (Escenario A)

### 8.1 Scope

Soportado en v1:
- Múltiples patches al mismo artefacto desde agente y usuario en orden arbitrario
- Auto-rebase server-side cuando las ops no conflictan
- Conflict response estructurado cuando no se puede rebasear

NO soportado en v1:
- Real-time multi-user collab (dos humanos editando live) — requiere CRDT/OT, Escenario B

### 8.2 `ConflictDetector` trait

```rust
pub trait ConflictDetector: Send + Sync {
    fn check(&self, incoming: &PatchOp, against: &PatchOp) -> ConflictCheck;
    fn rebase(&self, op: &PatchOp, after: &PatchOp) -> RebasedOp;
}

pub enum ConflictCheck {
    None,
    Conflicts(ConflictReason),
    RequiresShift,
}

pub enum RebasedOp {
    Unchanged,
    Shifted(PatchOp),
    Dropped,          // target eliminado upstream
}

pub enum ConflictReason {
    SameCellModified { sheet: SheetId, address: String },
    SameBlockReplaced { block: BlockId },
    TargetDeleted { entity: String },
    SameRunModified { block: BlockId, run: RunId },
    ...
}
```

Implementaciones por formato: `ExcelConflictDetector`, `WordConflictDetector`.

### 8.3 `RebaseService`

Entrada: patch entrante `P` con `base_version=v_base`, artefacto con `current_version=v_head` donde `v_head > v_base`.

Algoritmo:

```
intermediate_patches = load_patches_between(v_base, v_head)  # v_base+1..v_head
rebased_ops = []
conflicts = []

for op in P.ops:
    current_op = op
    for intermediate_op in intermediate_patches.flatten():
        check = detector.check(current_op, intermediate_op)
        match check:
            None:
                continue
            Conflicts(reason):
                conflicts.push({incoming: op, against: intermediate_op, reason})
                break
            RequiresShift:
                rebased = detector.rebase(current_op, intermediate_op)
                match rebased:
                    Unchanged:
                        continue
                    Shifted(new_op):
                        current_op = new_op
                    Dropped:
                        # el target fue eliminado upstream
                        conflicts.push({incoming: op, reason: TargetDeleted})
                        break
    if current_op was not dropped and no conflict:
        rebased_ops.push(current_op)

if conflicts.is_empty():
    return Rebased(Patch { base_version: v_head, ops: rebased_ops })
else:
    return VersionConflict { conflicts, current: v_head }
```

### 8.4 Conflict rules por op pair (summary)

**Excel**:
- `set_cell(s, a)` vs `set_cell(s, a)`: conflict (same cell)
- `set_cell(s, a)` vs `clear_range(s, r)` where `a ∈ r`: conflict
- `set_cell(s, a)` vs `delete_row(s, r)` where `a.row == r`: conflict
- `set_cell(s, a)` vs `delete_row(s, r)` where `a.row > r`: shift (row -1)
- `set_cell(s, a)` vs `insert_row(s, r)` where `a.row >= r`: shift (row +1)
- Análogo para columns
- `set_cell(s, _)` vs `delete_sheet(s)`: conflict
- `add_sheet(_)` vs cualquier otra: no conflict
- `rename_sheet(s, n1)` vs `rename_sheet(s, n2)` where n1 != n2: last-write-wins (no conflict)

**Word**:
- `replace_run_text(b, r)` vs same: conflict
- `replace_run_text(b, r)` vs `delete_run(b, r)`: conflict
- `replace_run_text(b, r)` vs `delete_block(b)`: conflict
- `insert_block(_)` vs cualquier otra op (diferente ID): no conflict (IDs son estables)
- `replace_block(b, _)` vs `replace_block(b, _)`: conflict
- `delete_block(b)` vs cualquier op que referencia `b`: conflict
- `insert_list_item(l, _)` vs `delete_list_item(l, _)` con IDs distintos: no conflict

Catálogo completo en `infrastructure/conflict/*.rs` con matriz de tests.

### 8.5 Response de conflicto

Cuando el rebase falla, el use case devuelve:

```rust
DocumentError::VersionConflict {
    artifact: ArtifactId,
    base: VersionId,
    current: VersionId,
    conflicts: Vec<ConflictDetail>,
}

pub struct ConflictDetail {
    pub incoming_op: PatchOp,
    pub conflicting_with: PatchOp,
    pub in_version: VersionId,
    pub reason: ConflictReason,
}
```

Serializado a JSON para tools LLM y nodos DAG. El caller decide cómo resolver (release latest y retry, abortar, escalar a usuario).

---

## 9. Session-Artifact Index

### 9.1 Propósito

Mapear `session_id ↔ [artifact_ids]` para que el agente pueda listar sus artefactos, y para validar isolation básico entre sesiones.

### 9.2 Schema

Nueva tabla (migración SQLite + Postgres):

```sql
CREATE TABLE document_artifacts (
    artifact_id       TEXT PRIMARY KEY,
    session_id        TEXT NOT NULL,
    kind              TEXT NOT NULL,          -- 'excel' | 'word'
    label             TEXT,
    current_version   TEXT NOT NULL,
    created_at        TIMESTAMP NOT NULL,
    updated_at        TIMESTAMP NOT NULL,
    retention_limit   INTEGER NOT NULL
);

CREATE INDEX idx_document_artifacts_session ON document_artifacts(session_id);
```

### 9.3 `SessionArtifactIndex` trait

```rust
#[async_trait]
pub trait SessionArtifactIndex: Send + Sync {
    async fn register(
        &self,
        session: &SessionId,
        id: &ArtifactId,
        meta: &ArtifactMeta,
    ) -> Result<(), IndexError>;

    async fn list_by_session(
        &self,
        session: &SessionId,
    ) -> Result<Vec<ArtifactSummary>, IndexError>;

    async fn lookup(
        &self,
        id: &ArtifactId,
    ) -> Result<Option<ArtifactSummary>, IndexError>;

    async fn update_head(
        &self,
        id: &ArtifactId,
        version: &VersionId,
        updated_at: DateTime<Utc>,
    ) -> Result<(), IndexError>;

    async fn unregister(&self, id: &ArtifactId) -> Result<(), IndexError>;
}

pub struct ArtifactSummary {
    pub artifact_id: ArtifactId,
    pub session_id: SessionId,
    pub kind: ArtifactKind,
    pub label: Option<String>,
    pub current_version: VersionId,
    pub updated_at: DateTime<Utc>,
}
```

### 9.4 Adapters

- `SqliteSessionArtifactIndex` — reutiliza el `SqlitePool` del `PgPoolRegistry` equivalente (mismo patrón que `llm::infrastructure::persistence`)
- `PostgresSessionArtifactIndex` — idem
- `InMemorySessionArtifactIndex` — para standalone CLI y tests sin DB

### 9.5 `connection_url` inheritance

Sigue el patrón de memoria LLM: `connection_url` se pasa en el config del grafo/tool. Si no hay, se usa in-memory. Las migraciones aplican automáticamente al conectarse (embedded SQL scripts via `sqlx::migrate!`).

### 9.6 Session isolation

Opcional en v1 (toggle via config), default **on**:

- `document_read`, `document_apply_patch`, `document_get_head`, etc. reciben `session_id` exclusivamente del context del caller (ver sec 11.1 sobre por qué el LLM no lo setea)
- Antes de operar, consultan `SessionArtifactIndex.lookup(artifact_id)` y validan que `session_id` match
- Si no match: `DocumentError::SessionIsolationViolation`

Se puede desactivar (para admins o casos especiales) con flag `enforce_session_isolation: false` en config del store.

---

## 10. Rendering

### 10.1 Crates

- **Excel**: `rust_xlsxwriter` (MIT) — write-only, mantenido, API completa para v1
- **Word**: `docx-rs` (MIT) — write-only, OOXML docx

No se usa LibreOffice en v1. LibreOffice queda reservado como adapter de PDF export opcional v2+.

### 10.2 `IRRenderer` trait

```rust
#[async_trait]
pub trait IRRenderer: Send + Sync {
    type IR;
    async fn render(&self, ir: &Self::IR) -> Result<Vec<u8>, RenderError>;
    fn target_extension(&self) -> &'static str;
    fn target_mime(&self) -> &'static str;
}
```

Implementaciones:
- `ExcelRenderer` → `.xlsx`, `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`
- `WordRenderer` → `.docx`, `application/vnd.openxmlformats-officedocument.wordprocessingml.document`

### 10.3 Render pipeline por commit

1. `ApplyPatchUseCase` valida + aplica patch → new IR
2. `IRValidator` verifica invariantes
3. `IRRenderer` produce bytes
4. `ArtifactStore.write_version` persiste atómicamente

### 10.4 Performance targets

- Excel render (~10 sheets, ~10k cells total): < 500ms
- Word render (~50 páginas equivalentes): < 200ms

Si se rebasan en uso real, considerar caching incremental (no aplica v1).

---

## 11. Integration Surface

### 11.1 Synthetic LLM tools

Registradas en `llm_synthetic_tools/document_tools.rs`, siguiendo el patrón de `load_skill_tool`.

| Tool | Input | Output |
|---|---|---|
| `document_create` | `{kind, initial_ir?, label?, retention_limit?}` | `{artifact_id, version_id, label, head_summary}` |
| `document_apply_patch` | `{artifact_id, base_version, ops}` | `{version_id, diff_summary}` o `VersionConflict` |
| `document_read` | `{artifact_id, version?, slice?}` | `{ir}` (full o sliced) |
| `document_get_head` | `{artifact_id, since_version?}` | `{version_id, updated_at, last_source, summary_since, versions_in_window}` |
| `document_list_versions` | `{artifact_id, limit?}` | `[{version_id, created_at, source, summary}]` |
| `document_rollback` | `{artifact_id, to_version}` | `{new_version_id, summary}` |
| `document_list_my_artifacts` | `{session_id?}` | `[ArtifactSummary]` |

**Default label en `document_create`**: cuando el caller no provee `label`, se autogenera: `"Untitled {Kind} {YYYY-MM-DD HH:MM}"` (ej. `"Untitled Excel 2026-04-21 14:32"`). El label es editable posteriormente (op futura o via update_meta).

**Slice en `document_read`**: `{sheets?: [SheetId], block_ids?: [BlockId], cell_ranges?: [{sheet_id, range}]}`. Permite traer solo la parte necesaria.

**`document_get_head` con `since_version`**: devuelve summary de todos los user edits entre `since_version` y `current_version`. Esto es el mecanismo de **pull explícito** para que el LLM vea qué cambió el usuario.

**Scope y seguridad de `session_id`**: `session_id` **nunca aparece en el schema de las tools expuestas al LLM**. Se inyecta exclusivamente desde el context de ejecución (LLM node context, DAG context, o API caller). Esto es una regla dura:

- Las tool definitions que ve el LLM no tienen `session_id` como parámetro
- El servidor (o el use case invocador) resuelve `session_id` desde su context y lo pasa al use case de `application/`
- Si un LLM intentara incluir `session_id` en el JSON de tool call, se ignora silenciosamente
- El agente, por construcción, solo puede operar sobre artefactos de su propia session

Esto aplica también a cualquier otro campo de identidad/autorización futuro (user_id, org_id, etc.) — el LLM nunca los llena.

### 11.2 DAG nodes

`document_create`, `document_edit`, `document_read` como nodos nativos.

**`document_create` config**:
```json
{
  "type": "document_create",
  "config": {
    "kind": "excel" | "word",
    "initial_ir": { ... } | "$DYNAMIC" | "$ref:input.ir",
    "label": "...",
    "retention_limit": 20,
    "session_id": "$ref:context.session_id"
  }
}
```

**`document_edit` config**:
```json
{
  "type": "document_edit",
  "config": {
    "artifact_id": "$ref:create_step.output.artifact_id",
    "base_version": "$ref:create_step.output.version_id" | "$DYNAMIC",
    "ops": [...] | "$DYNAMIC" | "$ref:input.ops"
  }
}
```

**`document_read` config**:
```json
{
  "type": "document_read",
  "config": {
    "artifact_id": "...",
    "version": "v3" | null,
    "slice": {...} | null
  }
}
```

Todos los nodos devuelven `{output: ...}` siguiendo la convención del repo. Errores `VersionConflict` se propagan como node failures con payload estructurado.

### 11.3 Compatibilidad con `$DYNAMIC` y exposición como LLM tools

Los nodos `document_create`/`document_edit`/`document_read` soportan `$DYNAMIC` en sus configs. Se pueden exponer como tools de un `llm_call` node igual que HTTP y SQL nodes, via el `dag_tool_executor` existente.

Esto da **dos caminos** equivalentes para el agente:
1. **Synthetic tools directas** — más simple, sin necesidad de armar un grafo con nodos separados
2. **Nodos expuestos como tools** — útil cuando el grafo ya orquesta múltiples steps

Ambos caminos convergen en los mismos use cases de `application/`.

### 11.4 Descarga de binarios

La exposición del binario rendereado (`.xlsx`/`.docx`) tiene dos niveles:

**Nivel librería (siempre disponible):**

Use case `DownloadArtifactUseCase` en `application/`:

```rust
pub struct DownloadArtifactInput {
    pub artifact_id: ArtifactId,
    pub version: Option<VersionId>,    // None = current
    pub session_id: SessionId,          // para isolation check
}

pub struct DownloadArtifactOutput {
    pub bytes: Vec<u8>,
    pub filename: String,               // ej. "Informe Q3.xlsx"
    pub mime_type: &'static str,
    pub version_id: VersionId,
}
```

Cualquier código (bindings Python/TypeScript, DAG node futuro, integrator externo) puede invocarlo.

**Nivel HTTP (solo en modo `serve` del DAG engine):**

Cuando el CLI se invoca con `cargo run --bin dag_engine -- serve ...`, el servidor HTTP expone adicionalmente:

```
GET /artifacts/:artifact_id/versions/:version/render
GET /artifacts/:artifact_id/render              # shortcut → current
```

Respuesta: bytes del binario con `Content-Type` y `Content-Disposition: attachment; filename=...`. Autenticación y session scoping heredados del mecanismo de `serve` existente (no se define acá; es el mismo patrón que el resto de endpoints).

**En modo CLI `run`** (ejecución puntual sin servidor), no hay endpoint HTTP. La descarga se hace via use case directo o leyendo del `storage_root` configurado.

**No se expone como synthetic tool LLM ni como DAG node en v1**. Razón: el LLM no necesita recibir bytes (inflaría el contexto); el grafo típicamente no consume el binario directamente sino que pasa el path/URL al caller. Si aparece un caso de uso claro, se agrega en v2.

---

## 12. Documentation Strategy

### 12.1 Capa 1 — Schema self-documentado

Usar `schemars` para derivar JSON Schema desde los tipos Rust. Todos los campos llevan `#[schemars(description = "...")]`.

Ejemplo concreto:

```rust
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op")]
pub enum PatchOp {
    /// Set the value of a single cell. Creates it if missing, overwrites if
    /// present. Use for isolated changes. For contiguous bulk updates, prefer
    /// `set_range`.
    #[serde(rename = "set_cell")]
    SetCell {
        /// Stable sheet ID (e.g. "sheet_01"). NOT the display name.
        sheet_id: SheetId,

        /// A1-style cell address (e.g. "B5", "AA120"). Case-insensitive.
        address: String,

        /// The value. Type inferred from JSON type unless `value_type` overrides.
        value: serde_json::Value,

        /// Optional: override the inferred type. Use for numbers stored as text,
        /// or formula strings (prefix "=").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value_type: Option<CellType>,

        /// Optional: number format spec (e.g. "#,##0", "0.00%").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,

        /// Optional: reference to a style defined in `named_styles`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style_ref: Option<String>,
    },
    // ...
}
```

El schema generado se expone como input schema del tool `document_apply_patch`. El LLM ve el schema completo con descripciones inline.

### 12.2 Capa 2 — Skill built-in `document_authoring`

Shipped con la librería, cargable vía `load_skill`. Estructura:

```
skills/document_authoring/
  README.md
  overview.md            # mental model: IR + versioning + patches + rebase
  excel/
    ir_schema.md         # estructura completa con examples
    ops_catalog.md       # cada op con when-to-use y ejemplos
    patterns.md          # recetas: reporte con tabla, dashboard, etc.
    pitfalls.md          # errores comunes (usar name vs ID, etc.)
  word/
    ir_schema.md
    ops_catalog.md
    patterns.md
    pitfalls.md
  conflicts.md           # manejo de VersionConflict
  user_edits.md          # interpretación del pull de get_head
  concurrency.md         # cuándo rebase, cuándo reintentar
```

Se carga on-demand. Solo ocupa contexto cuando el agente la pide.

### 12.3 Capa 3 — Golden examples en `tests/graphs/documents/`

Grafos JSON ejecutables que sirven de test + tutorial:
- `excel_create_and_edit.json`
- `word_report_generation.json`
- `excel_with_llm_tool_calling.json`
- `word_with_llm_tool_calling.json`
- `concurrent_edits.json`
- `rollback_example.json`

### 12.4 Capa 4 — Developer guide

Crear `docs/developer_guide/25_documents_guide.md` cubriendo:
- Setup (env vars, storage backends)
- API de use cases
- Nodos DAG config reference
- Synthetic tools reference
- Retention tuning
- Migración v1 → v2 path

Extender:
- `docs/node_configurations.json` con los configs de nodos nuevos
- `docs/agent_context/node_ports_reference.md` con los ports
- `docs/DEVELOPER_GUIDE.md` index con la sección 25

---

## 13. User-Edit Narration (Pull Explicit v1)

### 13.1 Modelo

Cuando un patch con `source: "user"` se commitea, el `diff_service.narrate(ops, ir_before, ir_after)` produce:

```rust
pub struct PatchSummary {
    pub natural_language: Vec<String>,
    pub structured: Vec<StructuredChange>,
}
```

Se guarda en `patch_applied.json` de la versión.

### 13.2 Entrega al LLM vía pull

El LLM llama `document_get_head(artifact_id, since_version=<última versión que el LLM vio>)`. El tool devuelve:

```json
{
  "artifact_id": "art_abc123",
  "current_version": "v7",
  "updated_at": "2026-04-21T14:35:00Z",
  "last_source": "user",
  "summary_since": [
    "[v6, user, 14:33] Celda B5 en 'Ventas': 1000 → 1500",
    "[v7, user, 14:35] Tabla 'Sales': fila agregada [Widget3, 200, 3000]"
  ],
  "versions_in_window": ["v6", "v7"]
}
```

El LLM integra ese texto en su razonamiento del turno actual.

### 13.3 Generación de natural language

Reglas deterministas por tipo de op (no usa LLM — sería overkill y costoso). Templates en `infrastructure/diff/narration_templates.rs`.

Ejemplos:
- `set_cell` → "Cell {address} in sheet '{sheet_name}': {before} → {after}"
- `insert_row` → "Row inserted at position {row} in sheet '{sheet_name}'"
- `replace_run_text` → "Paragraph '{block_label}' text changed: '{before}' → '{after}'"
- `delete_block` → "Block '{block_label}' removed"

Para combinaciones complejas (ej. reordering de 5 sheets), fallback a descripción genérica.

### 13.4 Not in scope v1

Push automático (inyectar edits al LLM al inicio del turno sin que los pida) — queda para v2 si se ve que es necesario. Con pull explícito, el agente consulta cuando decide que le interesa el estado actual.

---

## 14. Errors

```rust
#[derive(thiserror::Error, Debug, Serialize)]
pub enum DocumentError {
    #[error("artifact not found: {0}")]
    ArtifactNotFound(ArtifactId),

    #[error("version not found: {artifact}/{version}")]
    VersionNotFound { artifact: ArtifactId, version: VersionId },

    #[error("version conflict: requested base {base}, current is {current}")]
    VersionConflict {
        artifact: ArtifactId,
        base: VersionId,
        current: VersionId,
        conflicts: Vec<ConflictDetail>,
    },

    #[error("IR validation failed at {path}: {reason}")]
    IRValidationFailed { path: String, reason: String },

    #[error("invalid patch op: {reason}")]
    InvalidPatchOp { reason: String, op: serde_json::Value },

    #[error("render failed: {0}")]
    RenderFailed(String),

    #[error("storage error: {0}")]
    StorageError(#[from] StorageError),

    #[error("index error: {0}")]
    IndexError(#[from] IndexError),

    #[error("session isolation violation: artifact {0} does not belong to session {1}")]
    SessionIsolationViolation(ArtifactId, SessionId),
}

#[derive(thiserror::Error, Debug, Serialize)]
pub enum StorageError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("precondition failed (generation mismatch): {0}")]
    PreconditionFailed(String),

    #[error("transient error: {0}")]
    Transient(String),

    #[error("backend error: {0}")]
    Backend(String),
}
```

Errores serializables para cruzar boundaries (LLM tools, DAG nodes). `VersionConflict` incluye `Vec<ConflictDetail>` con info estructurada.

---

## 15. Testing

### 15.1 Unit tests (`#[cfg(test)]` inline)

- **Patch ops**: cada op aplicada a un IR sintético, verificar IR resultante
- **ConflictDetector**: matriz de pares de ops (Excel y Word), verificar cada celda
- **RebaseService**: secuencias de ops que exigen shifts (especialmente Excel row/col inserts)
- **IRValidator**: casos malos (IDs duplicados, style refs colgantes, tipos inconsistentes)
- **diff_service.narrate**: fixtures de ops → texto esperado

### 15.2 Integration tests (`tests/`)

- Ciclo completo create → edit → read → rollback contra `LocalFsStore` en tempdir
- Mismo suite contra `GcsStore` apuntando a `fake-gcs-server` (docker en CI)
- Round-trip IR → render → parser externo (`calamine` read-side para xlsx, crate read de docx) para verificar que el binario abre sin errores
- Concurrencia: dos `apply_patch` paralelos sobre el mismo artifact; uno gana, el otro rebase o `VersionConflict`
- Session isolation: dos sessions, una no puede leer artefactos de la otra

### 15.3 Graph tests (`tests/graphs/documents/`)

- `excel_create_and_edit.json`
- `word_report_generation.json`
- `excel_with_llm_tool_calling.json`
- `word_with_llm_tool_calling.json`
- `concurrent_edits.json`
- `rollback_example.json`

### 15.4 Python tests (`python/tests/`)

Smoke tests verificando que los bindings PyO3 funcionan end-to-end.

### 15.5 Coverage target

`> 85%` line coverage en `documents/` antes de ship.

---

## 16. V2 Roadmap (referenciado desde el diseño)

Cada item habilitado por decisiones de v1, aditivo (sin breaking changes):

1. **Merged cells** (Excel) — `merges: []` por sheet, op `merge_range` / `unmerge_range`
2. **Imágenes inline** — bloque `image` en Word, object `image` anclado a range en Excel, binarios en `versions/vN/blobs/`
3. **Formato condicional** (Excel) — `conditional_rules: []` por sheet, ops CRUD
4. **Charts** (Excel) — objeto `chart` con type/data_ref/series, ops CRUD
5. **Pivot tables** (Excel) — bloque pivot con rows/cols/values/filters
6. **Charts en Word** — embedding de xlsx data dentro de docx
7. **Round-trip de docs existentes** — parser OOXML → IR, habilitando upload de docs externos
8. **Export a PDF** — adapter `LibreOfficePdfExporter` opcional
9. **Push automático de user edits** — event hook que inyecta edits al LLM al inicio del turno
10. **Real-time multi-user collab (Escenario B)** — adopción de CRDT (yjs/automerge) para IR; cambio arquitectural grande, pendiente de necesidad real
11. **Tracked changes y comentarios** — metadata paralela al IR

---

## 17. Open Questions

Ninguna — todas las decisiones del spec están tomadas. Las decisiones finales quedan así:

### 17.1 Crate GCS: `google-cloud-storage`

Del ecosystem [`google-cloud-rust`](https://github.com/yoshidan/google-cloud-rust) de yoshidan.

**Por qué:**
- Activamente mantenido (releases frecuentes en 2024–2025)
- Tokio-native async → consistente con el runtime del resto del repo
- Soporte nativo de `if-generation-match` como parámetro de write → requerido para el CAS atómico de `HEAD` (sec 7.3, 8.1)
- Rustls-compatible, no arrastra `openssl-sys` → consistente con `reqwest` en `rustls-tls` del repo
- Parte de un ecosystem coherente (`google-cloud-auth`, `google-cloud-pubsub`, etc.) — futuro-proof

**Fallbacks documentados** (usar solo si blocker aparece durante implementación):
- `cloud-storage` crate (ThouCheese) — API más simple pero menos activa
- Ad-hoc con `gcp_auth` + `reqwest` — máximo control, máximo trabajo

**Validación en implementación**: confirmar durante el spike inicial del módulo GCS que (a) `if-generation-match` funciona end-to-end, (b) ADC + SA JSON + workload identity todos soportados, (c) no hay arrastre de `openssl-sys`.

### 17.2 Migrations layout: `migrations/documents/`

Directorio `migrations/documents/` a nivel de workspace, usando `sqlx::migrate!` con path explícito. Se evalúa durante implementación el naming/orden para convivir con las migraciones de memoria LLM existentes (si las hay) sin colisionar en el mismo schema namespace.

---

---

## 18. Summary for Implementation Plan

El plan debería descomponer esto en fases, sugerido:

1. **Fase 1** — domain (IR types, PatchOp enum, errors, ports) + application use cases esqueleto (create, apply_patch, read) con `InMemoryArtifactStore` para prototyping
2. **Fase 2** — LocalFsStore + ExcelRenderer + rendering test
3. **Fase 3** — WordRenderer (paralelo a Fase 2, independiente)
4. **Fase 4** — SessionArtifactIndex (SQLite + Postgres + InMemory) + migrations
5. **Fase 5** — ConflictDetector + RebaseService + tests de matriz
6. **Fase 6** — GcsStore + test contra fake-gcs-server
7. **Fase 7** — DAG nodes (document_create, document_edit, document_read)
8. **Fase 8** — Synthetic LLM tools (document_tools.rs)
9. **Fase 9** — diff_service + narration templates
10. **Fase 10** — Skill `document_authoring` + developer guide + golden graphs
11. **Fase 11** — E2E integration tests + Python bindings smoke tests
12. **Fase 12** — Documentation final (node_configurations.json, node_ports_reference.md)

Orden dentro de cada fase flexible. Fases 2/3 y 4/5 y 7/8 son independientes (parallelizables).

El plan detallado se escribirá en un documento separado siguiendo la skill `writing-plans` tras aprobar este spec.
