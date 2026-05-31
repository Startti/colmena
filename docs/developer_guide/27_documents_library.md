# 27. Librería de Documentos

La librería de documentos (`DocumentRuntime`) permite a un agente LLM **crear y editar documentos de forma incremental** —archivos Excel (`.xlsx`), Word (`.docx`) y presentaciones/reportes HTML (`.html`)— con versionado inmutable, historial completo, almacenamiento *pluggable* (disco local o Google Cloud Storage) y exposición transparente como herramientas del LLM. Además, expone un canal aparte para **assets binarios** (imágenes referenciables desde el HTML) vía `AssetStore`.

A diferencia de subir/bajar binarios completos, la librería trabaja sobre una **representación intermedia (IR) en JSON** que es la única fuente de verdad. Cada cambio es un `Patch` con operaciones tipadas y atómicas (`set_cell`, `insert_block`, `insert_html_block`, etc.), y el binario `.xlsx`/`.docx`/`.html` se renderiza *on-demand* desde la IR.

> Código del módulo: [src/libs/colmena/src/documents/](../../src/libs/colmena/src/documents/)
> Diseño completo (interno): [docs/superpowers/specs/2026-04-21-documents-feature-design.md](../superpowers/specs/2026-04-21-documents-feature-design.md)

---

## 1. Arquitectura

Hexagonal estricta — domain / application / infrastructure — exactamente como el resto del crate (ver [`01_architecture.md`](./01_architecture.md)):

| Capa | Carpeta | Responsable de |
|------|---------|----------------|
| **Domain** | [documents/domain/](../../src/libs/colmena/src/documents/domain/) | IDs (`ArtifactId`, `AssetId`, `SessionId`), IR (excel/word/html), `Patch`/`PatchOp`, errores y *traits* (`ArtifactStore`, `AssetStore`, `IRRenderer`, `IRValidator`, `IdGenerator`, `SessionArtifactIndex`). |
| **Application** | [documents/application/](../../src/libs/colmena/src/documents/application/) | 9 use-cases — documentos: `CreateDocument`, `ApplyPatch`, `ReadDocument`, `GetHead`, `ListVersions`, `Rollback`; assets: `UploadAsset`, `ListAssets`, `DeleteAsset`. Más el bundler [`DocumentRuntime`](../../src/libs/colmena/src/documents/application/runtime.rs). Los applier por kind (`apply_excel_ops`, `apply_word_ops`, `apply_html_ops`) viven aquí también. |
| **Infrastructure** | [documents/infrastructure/](../../src/libs/colmena/src/documents/infrastructure/) | Stores de artifacts (`LocalFsStore`, `GcsArtifactStore`) y de assets (`LocalFsAssetStore`, `GcsAssetStore`), `ExcelRenderer` (`rust_xlsxwriter`), `WordRenderer` (`docx-rs`), `HtmlRenderer` (`maud` + temas CSS + `slides_runtime.js` + Chart.js empotrado), validadores (`ExcelValidator`, `WordValidator`, `HtmlValidator`), `UlidIdGenerator`. |
| **DAG nodes** | [dag_engine/.../document_nodes.rs](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs) | Nodos `document_create`, `document_edit`, `document_read`. |
| **LLM tools** | [dag_engine/.../document_tools.rs](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs) | 7 *synthetic tools* expuestos al modelo cuando `llm_call.config.documents` está presente. |

### Flujo de una edición LLM-iniciada

```
LLM ──tool_call("document_apply_patch", {...})──▶ dag_tool_executor
                                                       │
                                                       ▼
                                          DocumentToolsContext
                                          (resuelve session_id
                                           server-side)
                                                       │
                                                       ▼
                                          ApplyPatchUseCase
                                                       │
                       ┌───────────────────┬───────────┴──────────────┐
                       ▼                   ▼                          ▼
                IRValidator          ArtifactStore                IRRenderer
              (schema, IDs)        (HEAD + version)            (xlsx | docx)
```

---

## 2. Conceptos centrales

### Artifact y kind
Un *artifact* es un documento individual identificado por `ArtifactId` (string opaco con prefijo `art_`, ULID-based). El enum `ArtifactKind` tiene tres variantes y determina la forma de la IR y el binario derivado:

| Kind | Extensión | MIME |
|------|-----------|------|
| `excel` | `xlsx` | `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet` |
| `word` | `docx` | `application/vnd.openxmlformats-officedocument.wordprocessingml.document` |
| `html` | `html` | `text/html; charset=utf-8` |

Definiciones en [domain/ids.rs](../../src/libs/colmena/src/documents/domain/ids.rs) (ver `ArtifactKind` en línea 88).

Aparte de los artifacts, existen los **assets** — blobs binarios (típicamente imágenes) referenciables por `AssetId` desde bloques `image` del HTML IR. Se manejan por separado (`AssetStore`, use cases dedicados) porque no participan del versionado/IR.

### IR (Intermediate Representation)
JSON estructurado almacenado por versión. Ejemplo mínimo de un Excel vacío:

```json
{
  "kind": "excel",
  "artifact_id": "art_xxx",
  "version_id": "v1",
  "schema_version": "1.0.0",
  "workbook": {
    "sheets": [
      { "id": "s1", "name": "Hoja1", "order": 0,
        "columns": [], "cells": {}, "tables": [] }
    ],
    "named_styles": {}
  }
}
```

Esquema detallado en [domain/ir/excel.rs](../../src/libs/colmena/src/documents/domain/ir/excel.rs), [domain/ir/word.rs](../../src/libs/colmena/src/documents/domain/ir/word.rs) y [domain/ir/html.rs](../../src/libs/colmena/src/documents/domain/ir/html.rs). El IR HTML modela `slides` (con `layout`, `title`, `subtitle`, `notes`) que contienen `blocks` tipados (paragraph, heading, list, table, image, chart, callout, columns, page_break, etc.), más propiedades de documento (`theme`, `layout_mode`, `doc_props`).

### Patch y `PatchSource`
Un `Patch` agrupa una lista ordenada de `PatchOp` que se aplican **atómicamente** sobre `base_version`. Si una falla, ninguna se aplica.

```json
{
  "artifact_id": "art_xxx",
  "base_version": "v3",
  "source": "agent",
  "ops": [
    { "op": "set_cell", "sheet_id": "s1", "address": "A1", "value": "Hola" }
  ]
}
```

`source` puede ser `"agent"` (por el LLM, valor por defecto) o `"user"` (edición humana directa). Solo los patches `user` generan narración en lenguaje natural que el agente puede consumir vía `document_get_head`.

### Versionado inmutable
- Cada patch produce una nueva versión: `v1`, `v2`, `v3`, …
- Retención por defecto: **20 versiones** (constante `DEFAULT_RETENTION` en [runtime.rs:32](../../src/libs/colmena/src/documents/application/runtime.rs#L32)). `v1` siempre se conserva aunque caiga fuera de la ventana.
- Por versión se persiste: IR JSON, render binario y metadatos del patch aplicado.
- `document_rollback` es **no destructivo**: copia la IR de la versión objetivo a un nuevo HEAD; el historial completo se mantiene.

### Aislamiento por sesión
- Cada artifact pertenece a un `SessionId`.
- El `session_id` se **resuelve siempre del lado del servidor** (input `__colmena_session_id` > input `session_id` > config `session_id` > `"default"` — ver [`resolve_session_id`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs#L36)).
- **Nunca** aparece en los schemas que ve el LLM. Hay un test (`no_tool_schema_exposes_session_id`) que falla la build si alguno lo filtra.
- El `SessionArtifactIndex` (opcional, Postgres o in-memory) habilita `document_list_my_artifacts`.

### Concurrencia y narración (`get_head`)
Cuando el HEAD del servidor es más reciente que `base_version`:
- Si los ops del agente **no chocan** con las ediciones intermedias → auto-rebase silencioso, el patch se aplica.
- Si **chocan** → respuesta estructurada `{"error":"VersionConflict","current_version":"v9","conflicts":[...]}`. El agente debe llamar `document_get_head(artifact_id, since_version=YOUR_BASE)` para recibir una narración legible de los cambios del usuario, reformular sus ops sobre el HEAD nuevo y reintentar.

---

## 3. Capacidades — operaciones soportadas

### Excel (16 ops)

| Op | Propósito |
|----|-----------|
| `set_cell` | Escribir una celda (valor, tipo, formato, estilo nombrado). |
| `set_range` | Escritura masiva rectangular (valores 2D *row-major*). |
| `clear_range` | Borrar celdas en un rango (no elimina filas/columnas). |
| `insert_row` / `delete_row` | Insertar/eliminar fila (1-indexed). |
| `insert_column` / `delete_column` | Insertar/eliminar columna (0-indexed, A=0). |
| `add_sheet` | Crear hoja nueva (el server genera el `sheet_id` y lo devuelve en `diff_summary`). |
| `rename_sheet` / `delete_sheet` | Renombrar / eliminar hoja por `sheet_id`. |
| `reorder_sheets` | Reordenar pasando la lista completa de IDs. |
| `create_table` / `resize_table` / `delete_table` | Tablas nombradas. |
| `set_column_width` | Ancho de columna. |
| `define_style` | Crear/actualizar un estilo nombrado referenciable vía `style_ref`. |

> Convención clave: las ops referencian la hoja por su **`sheet_id` estable** (ej. `"s1"`), nunca por el nombre visible. Para una hoja recién creada con `add_sheet`, **aplica un patch separado**: lee el ID generado en `diff_summary` y úsalo en el patch siguiente.

### Word (14 ops)

| Op | Propósito |
|----|-----------|
| `insert_block` | Insertar párrafo, heading, lista, tabla, imagen o page-break. Posicionar con `before` / `after`; sin ambos → append. |
| `replace_block` / `delete_block` / `move_block` | Manipular bloques completos por `block_id`. |
| `set_heading_level` | Cambiar el nivel de un heading (1-6). |
| `replace_run_text` | Reemplazar el texto de un *run* dentro de un párrafo/heading. |
| `set_run_style` | Patch parcial de estilo (bold/italic/underline/size/color). |
| `insert_run` / `delete_run` | Manipular *runs* dentro de un bloque. |
| `insert_list_item` / `replace_list_item` / `delete_list_item` | Ítems de lista. |
| `insert_table_row` / `delete_table_row` / `update_table_cell` | Filas y celdas de tablas. |

### HTML (19 ops)

Las ops HTML se agrupan por nivel: slide / block / table / list / documento.

| Op | Propósito |
|----|-----------|
| `add_slide` | Crear slide (`layout`: `title` \| `content` \| `section_divider` \| `blank`; opcional `at_index`, `title`, `subtitle`). El server genera el `slide_id`. |
| `delete_slide` | Eliminar slide por `slide_id`. |
| `reorder_slides` | Reordenar pasando la lista completa de `slide_ids`. |
| `set_slide_layout` | Cambiar el layout (`SectionDivider`/`Title` requieren `title` no vacío). |
| `set_slide_title` | Set/clear título y subtítulo del slide (independiente de blocks). |
| `set_slide_notes` | Set/clear las speaker notes. |
| `insert_html_block` | Insertar block en un slide (paragraph/heading/list/table/image/chart/callout/columns/page_break…). Posicionar con `before` / `after`; sin ambos → append. Devuelve `block_id`. |
| `delete_html_block` / `replace_html_block` / `move_html_block` | Manipular blocks completos por `block_id`. |
| `insert_html_table_row` / `delete_html_table_row` / `update_html_table_cell` | Filas y celdas de tablas. |
| `insert_html_list_item` / `delete_html_list_item` / `update_html_list_item` | Ítems de listas. |
| `set_theme` | Cambiar tema visual (`executive` \| `minimal` \| `vibrant` \| `dark`). |
| `set_doc_props` | Set parcial de `title` / `author` / `date` / `locale` (`en` \| `es`). |

Bloques `image` referencian assets via `AssetId` (ver §5). El render embebe Chart.js y `slides_runtime.js` (carpeta [`infrastructure/render/html_assets/`](../../src/libs/colmena/src/documents/infrastructure/render/html_assets/)) para tablas/listas/charts/navegación en el `.html` final.

Definiciones canónicas (con docs por campo) en [domain/patch.rs](../../src/libs/colmena/src/documents/domain/patch.rs).

---

## 4. Storage backends

Configurables vía el campo `storage_backend` (en config de nodo o de `llm_call.config.documents`):

### `localfs` (default)

```json
{
  "storage_backend": "localfs",
  "storage_root": "/tmp/colmena_documents",
  "asset_storage_root": "/tmp/colmena_documents_assets",
  "default_retention": 20
}
```

- `storage_root` por defecto: `./.colmena/documents` (constante `DEFAULT_STORAGE_ROOT` en [runtime.rs](../../src/libs/colmena/src/documents/application/runtime.rs)).
- `asset_storage_root` por defecto: directorio hermano `<storage_root>_assets` (o `.colmena/documents/assets` si tampoco se setea `storage_root`).
- Layout en disco (artifacts):
  ```
  {storage_root}/artifacts/{artifact_id}/
    meta.json
    HEAD
    versions/{version_id}/
      ir.json
      render.{xlsx|docx|html}
      patch_applied.json
  ```
- Layout en disco (assets):
  ```
  {asset_storage_root}/{session_id}/{asset_id}.{ext}
  ```
- Escrituras atómicas vía archivo temporal + `rename`.
- Implementaciones: [storage/local_fs_store.rs](../../src/libs/colmena/src/documents/infrastructure/storage/local_fs_store.rs) y [storage/local_fs_asset_store.rs](../../src/libs/colmena/src/documents/infrastructure/storage/local_fs_asset_store.rs).

### `gcs` (feature flag `gcs`)

```json
{
  "storage_backend": "gcs",
  "gcs_bucket": "mi-bucket",
  "gcs_prefix": "colmena/documents",
  "asset_gcs_prefix": "colmena/documents/assets"
}
```

- `gcs_prefix` por defecto: `colmena/documents`.
- `asset_gcs_prefix` por defecto: `colmena/documents/assets` (constante `DEFAULT_ASSET_GCS_PREFIX`). Comparte el mismo bucket (`gcs_bucket`).
- Compilación condicional: requiere `--features gcs`. Sin el flag, `from_config` devuelve un error claro: *"storage_backend `gcs` requires the `gcs` feature flag — rebuild with `--features gcs`"*.
- HEAD se escribe con **CAS optimista** (`set_if_generation_match`) para evitar pisadas concurrentes.
- Implementaciones: [storage/gcs_store.rs](../../src/libs/colmena/src/documents/infrastructure/storage/gcs_store.rs) y [storage/gcs_asset_store.rs](../../src/libs/colmena/src/documents/infrastructure/storage/gcs_asset_store.rs).

Cualquier otro valor (ej. `"s3"`) se rechaza al construir el runtime — fallo temprano, no en runtime.

### Resumen de campos JSON aceptados por `DocumentRuntime::from_config`

| Campo | Tipo | Default | Aplica a |
|-------|------|---------|----------|
| `storage_backend` | string | `"localfs"` | siempre |
| `storage_root` | string (path) | `./.colmena/documents` | localfs |
| `asset_storage_root` | string (path) | `<storage_root>_assets` (sibling) | localfs |
| `gcs_bucket` | string | — (obligatorio) | gcs |
| `gcs_prefix` | string | `"colmena/documents"` | gcs |
| `asset_gcs_prefix` | string | `"colmena/documents/assets"` | gcs |
| `default_retention` | u32 | `20` | siempre |
| `max_asset_size_bytes` | u64 | `10_485_760` (10 MiB) | siempre (assets) |
| `allowed_asset_mimes` | string[] | `["image/png","image/jpeg","image/gif","image/webp"]` | siempre (assets) |

Campos no reconocidos se ignoran silenciosamente para permitir crecimiento del schema sin romper grafos existentes.

---

## 5. Renderizado y validación

| Etapa | Implementación | Archivo |
|-------|----------------|---------|
| Validación de IR | `ExcelValidator`, `WordValidator`, `HtmlValidator` | [infrastructure/validation/](../../src/libs/colmena/src/documents/infrastructure/validation/) |
| Render Excel → `.xlsx` | `rust_xlsxwriter` | [render/excel_renderer.rs](../../src/libs/colmena/src/documents/infrastructure/render/excel_renderer.rs) |
| Render Word → `.docx` | `docx-rs` | [render/word_renderer.rs](../../src/libs/colmena/src/documents/infrastructure/render/word_renderer.rs) |
| Render HTML → `.html` | `maud` + temas CSS + `slides_runtime.js` + Chart.js (todo empotrado) | [render/html_renderer.rs](../../src/libs/colmena/src/documents/infrastructure/render/html_renderer.rs) y [render/html_assets/](../../src/libs/colmena/src/documents/infrastructure/render/html_assets/) |
| Generación de IDs | ULID | [infrastructure/ids.rs](../../src/libs/colmena/src/documents/infrastructure/ids.rs) |

La validación corre **antes** de renderizar y persistir. Verifica: schema (campos requeridos, tipos), unicidad de IDs (sheet_id, block_id, run_id, list_item_id, slide_id), formato A1 de direcciones, validez de rangos, integridad de referencias HTML (bloques pertenecen a slides existentes, `image` apunta a `asset_id` válido, layouts compatibles con `title`). Si falla → la versión no se escribe y el use-case retorna error.

### Assets (binarios fuera del IR)

Los blobs binarios (imágenes referenciadas desde el HTML IR) viven en `AssetStore` — un port separado de `ArtifactStore` porque no participan del versionado del IR. Tres use cases dedicados:

| Use case | Archivo | Función |
|----------|---------|---------|
| `UploadAssetUseCase` | [application/upload_asset.rs](../../src/libs/colmena/src/documents/application/upload_asset.rs) | Valida tamaño (`max_asset_size_bytes`) y MIME (`allowed_asset_mimes`), genera `AssetId`, persiste vía `AssetStore::upload` y devuelve un `AssetSummary`. Errores tipados (`AssetError::TooLarge`, `AssetError::MimeNotAllowed`). |
| `ListAssetsUseCase` | [application/list_assets.rs](../../src/libs/colmena/src/documents/application/list_assets.rs) | Lista assets de una sesión. |
| `DeleteAssetUseCase` | [application/delete_asset.rs](../../src/libs/colmena/src/documents/application/delete_asset.rs) | Elimina un asset (con hook opcional al `SessionArtifactIndex` para no romper referencias activas). |

Implementaciones del port: [`LocalFsAssetStore`](../../src/libs/colmena/src/documents/infrastructure/storage/local_fs_asset_store.rs) (siempre disponible) y [`GcsAssetStore`](../../src/libs/colmena/src/documents/infrastructure/storage/gcs_asset_store.rs) (feature flag `gcs`). Ambos se construyen automáticamente desde `DocumentRuntime::from_config` según el `storage_backend` elegido.

> Limitación actual: los use-cases de assets **no** están expuestos aún como nodos del DAG ni como synthetic tools del LLM — son consumibles solo desde código Rust vía `DocumentRuntime { upload_asset, list_assets, delete_asset, ... }`. La integración con `image` blocks del HTML IR ocurre a nivel del `HtmlRenderer`, que resuelve `asset_id` → bytes vía `AssetStore` al renderizar.

---

## 6. Uso desde el DAG (3 nodos)

Los 3 nodos comparten un `DocumentRuntime` por configuración via `OnceCell` (mismo patrón que `SqlNode`). Todos los campos de config admiten `$DYNAMIC` y `$ref`.

### `document_create`

| Campo | Origen | Notas |
|-------|--------|-------|
| `kind` | input o config | `"excel"` \| `"word"` (requerido). |
| `initial_ir` | input o config | Objeto IR completo (opcional; si se omite, doc vacío). |
| `label` | input o config | Etiqueta legible (opcional, auto-generada si no se da). |
| `retention_limit` | input o config | u32, opcional. |
| `storage_backend`, `storage_root` | config | Ver §4. |

**Output:** `{ "output": { "artifact_id", "version_id", "label" } }`.

### `document_edit`

| Campo | Origen | Notas |
|-------|--------|-------|
| `artifact_id` | input o config | requerido |
| `base_version` | input o config | requerido |
| `ops` | input o config | array de `PatchOp` (requerido) |

**Output (éxito):** `{ "output": { "version_id", "diff_summary": [...] } }`
**Output (conflicto):** `{ "output": { "error": "VersionConflict", "current_version": "v9", "conflicts": [...] } }`.

### `document_read`

| Campo | Origen | Notas |
|-------|--------|-------|
| `artifact_id` | input o config | requerido |
| `version` | input o config | opcional; default = HEAD actual |

**Output:** `{ "output": { "ir", "version_id" } }`.

### Ejemplo end-to-end (test graph)

[tests/graphs/documents/smoke_create_edit_read.json](../../tests/graphs/documents/smoke_create_edit_read.json) — encadena los 3 nodos:

```json
{
  "nodes": {
    "create_step": {
      "type": "document_create",
      "config": {
        "kind": "excel",
        "label": "Smoke test workbook",
        "storage_root": "/tmp/colmena_smoke_documents",
        "initial_ir": { "kind": "excel", "version_id": "v1", "schema_version": "1.0.0",
          "workbook": { "sheets": [{ "id": "s1", "name": "Hoja1", "order": 0,
            "columns": [], "cells": {}, "tables": [] }], "named_styles": {} } }
      }
    },
    "edit_step": {
      "type": "document_edit",
      "config": {
        "storage_root": "/tmp/colmena_smoke_documents",
        "ops": [
          { "op": "set_cell", "sheet_id": "s1", "address": "A1", "value": "Hola" },
          { "op": "set_cell", "sheet_id": "s1", "address": "B1", "value": 42 }
        ]
      }
    },
    "read_step": {
      "type": "document_read",
      "config": { "storage_root": "/tmp/colmena_smoke_documents" }
    },
    "log_step": { "type": "log" }
  },
  "edges": [
    { "from": "create_step.output.artifact_id", "to": "edit_step.artifact_id" },
    { "from": "create_step.output.version_id",  "to": "edit_step.base_version" },
    { "from": "create_step.output.artifact_id", "to": "read_step.artifact_id" },
    { "from": "edit_step.output.version_id",    "to": "read_step.version" },
    { "from": "read_step",                       "to": "log_step" }
  ]
}
```

Ejecutar:
```bash
cargo run --bin dag_engine -- run tests/graphs/documents/smoke_create_edit_read.json
```

---

## 7. Uso como herramientas del LLM

Para exponer documentos al modelo, agrega un bloque `documents` a la config del nodo `llm_call`:

```json
{
  "type": "llm_call",
  "config": {
    "provider": "openai",
    "model": "gpt-4o-mini",
    "documents": {
      "storage_backend": "localfs",
      "storage_root": "/tmp/agente_docs"
    }
  }
}
```

Esto:
1. Construye un `DocumentRuntime` con esa configuración (lazy, una sola vez por nodo).
2. Registra **7 *synthetic tools*** automáticamente con esquema generado por `schemars`.
3. Inyecta `DOCUMENTS_SYSTEM_PRELUDE` (manual de uso de las 7 tools) al system prompt.
4. Resuelve el `session_id` del contexto de ejecución y lo cablea en el `DocumentToolsContext` — **el LLM nunca lo ve**.

### Las 7 herramientas

| Tool | Propósito | Parámetros visibles al LLM |
|------|-----------|----------------------------|
| `document_create` | Crear nuevo Excel/Word | `kind`, `initial_ir?`, `label?`, `retention_limit?` |
| `document_apply_patch` | Aplicar patch atómico | `artifact_id`, `base_version`, `ops` |
| `document_read` | Leer IR (full o parcial) | `artifact_id`, `version?`, `slice?` (sheets, block_ids, cell_ranges) |
| `document_get_head` | HEAD + narración de cambios usuario | `artifact_id`, `since_version?` |
| `document_list_versions` | Historial reciente | `artifact_id`, `limit?` |
| `document_rollback` | Rollback no destructivo | `artifact_id`, `to_version` |
| `document_list_my_artifacts` | Listar artifacts de la sesión | (sin parámetros) |

Definiciones en [document_tools.rs:30-138](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs#L30-L138).

> `document_list_my_artifacts` requiere un `SessionArtifactIndex` configurado en el runtime; sin él devuelve `{"error":"session_index_not_configured", ...}` y el resto de tools sigue funcionando.

### Ejemplo: agente que crea un Excel a partir de prompt

[tests/graphs/documents/llm_tool_integration.json](../../tests/graphs/documents/llm_tool_integration.json):

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/test-document-tools",
        "method": "POST",
        "test_payload": {
          "prompt": "Crea un Excel con una hoja 'Ventas' con A1='Producto', B1='Precio', A2='Manzana', B2=100, A3='Pera', B3=150. Devuélveme el artifact_id."
        }
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "${OPENAI_API_KEY}",
        "stream": false,
        "documents": {
          "storage_backend": "localfs",
          "storage_root": "/tmp/colmena_llm_tools_documents"
        }
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "trigger", "to": "agent" },
    { "from": "agent",   "to": "log" }
  ]
}
```

Ejecutar:
```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/documents/llm_tool_integration.json
```

El agente —siguiendo el system prelude— hará: `document_create({kind:"excel"})` → leerá el `artifact_id` y `version_id:"v1"` → `document_apply_patch({artifact_id, base_version:"v1", ops:[add_sheet "Ventas"]})` → leerá el `sheet_id` generado del `diff_summary` → otro `document_apply_patch` con `set_cell` x6 → reportará el `artifact_id`.

---

## 8. Workflow estándar recomendado

El `DOCUMENTS_SYSTEM_PRELUDE` enseña al modelo este patrón ([document_tools.rs:284-378](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs#L284-L378)):

1. **Crear** con `document_create` (omite `initial_ir` salvo que necesites algo muy específico — es más simple shapearlo con patches).
2. **Patchar** con `document_apply_patch`. Pasa el `version_id` más reciente como `base_version`.
3. **Leer/reportar**: `document_read` para verificar y luego informar al usuario el `artifact_id`.

**Reglas prácticas:**
- Patches angostos > reescritura completa.
- Nunca usar un `sheet_id` recién creado en el mismo patch — siempre en uno posterior.
- En `VersionConflict` → `document_get_head(since_version=tu_base)` → reformular ops → reintentar.

---

## 9. Tests y ejemplos

| Recurso | Qué demuestra |
|---------|---------------|
| [tests/graphs/documents/smoke_create_edit_read.json](../../tests/graphs/documents/smoke_create_edit_read.json) | DAG mínimo create→edit→read→log. |
| [tests/graphs/documents/llm_tool_integration.json](../../tests/graphs/documents/llm_tool_integration.json) | LLM con las 7 tools. |
| Tests inline en [document_nodes.rs](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/document_nodes.rs) | Roundtrip create→read, advance de versión, conflict detection. |
| Tests en [document_tools.rs](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs) | Schema visible, ausencia de `session_id`, slicing por rangos A1. |
| Tests en [runtime.rs](../../src/libs/colmena/src/documents/application/runtime.rs) | Default localfs, rechazo de gcs sin feature, rechazo de backends desconocidos. |

Correr todo lo de documentos:
```bash
cargo test --lib documents
cargo test --lib document_nodes
cargo test --lib document_tools
```

---

## 10. Limitaciones conocidas (v1)

- **Sin ingesta de binarios existentes**: no se pueden subir `.xlsx`/`.docx`/`.html` y editarlos. Solo creación desde IR. (La excepción son los assets de imagen, que sí se suben como blobs vía `UploadAssetUseCase`.)
- **Sin colaboración en tiempo real**: el modelo de concurrencia es agente + usuario intercalando ediciones, con detección de conflicto y rebase. No hay CRDT ni edición simultánea fluida.
- **Excel — features fuera de alcance**: imágenes, charts, pivot tables, celdas merged, formato condicional, validación de datos.
- **Word — features fuera de alcance**: headers/footers, footnotes, tracked changes, macros, comentarios.
- **HTML — assets/tools aún no expuestos al DAG/LLM**: `ArtifactKind::Html` ya está soportado en el runtime (create/edit/read funcionan via los 3 nodos y las 7 synthetic tools si se pasa `kind: "html"`), pero los use-cases de assets (`upload_asset`, `list_assets`, `delete_asset`) solo se pueden invocar desde Rust por ahora — sin nodos DAG ni synthetic tools dedicados. El agente puede crear bloques `image` con un `asset_id` previamente subido por código externo.
- **Sin export PDF**: roadmap v2+.
- **Sin frontend**: la librería es backend-only. Para una UI editable se espera integrar Univer/Luckysheet (Excel) o Tiptap/OnlyOffice (Word) consumiendo la IR. El `.html` renderizado ya es un artefacto presentable en cualquier navegador (incluye `slides_runtime.js` para navegación de slides).

---

## 11. Referencias rápidas

- Bundler runtime: [`DocumentRuntime::from_config`](../../src/libs/colmena/src/documents/application/runtime.rs#L58)
- Catálogo de ops: [`PatchOp`](../../src/libs/colmena/src/documents/domain/patch.rs#L34)
- Manual del LLM: [`DOCUMENTS_SYSTEM_PRELUDE`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs#L284)
- Diseño completo (interno): [docs/superpowers/specs/2026-04-21-documents-feature-design.md](../superpowers/specs/2026-04-21-documents-feature-design.md)
