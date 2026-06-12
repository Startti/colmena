# Diseño — Insertar attachments (imágenes) en Google Docs vía `gdocs_insert_image_after_text`

**Fecha:** 2026-06-12
**Estado:** spec aprobado para implementación
**Autor:** brainstorming daniel@startti.co + Claude

## 1. Motivación

Colmena puede generar artefactos de imagen (`image_generation`, `image_edit`) y
recibir imágenes del usuario (inline o vía URL firmada). El path (i) URL-only de
`gdocs_insert_image_after_text` (shipped 2026-06-12, CHANGELOG §32) solo acepta
una URL pública directa. Falta poder **pegar en un Google Doc esas imágenes que
son bytes**: generadas, editadas, o subidas por el usuario.

Objetivo: extender `gdocs_insert_image_after_text` para que acepte un
`attachment_id` (cualquier fuente: `image_generation`/`image_edit` output, upload
inline del usuario, o attachment con URL firmada) e insertar esa imagen inline en
el doc.

## 2. El constraint que define el diseño

La Docs API `documents.batchUpdate` con `InsertInlineImageRequest` **solo acepta
una URL públicamente accesible** en el campo `uri`. Google baja la imagen
server-side, de forma anónima (sin el token del agente). No hay forma de mandarle
bytes ni una URL autenticada. Por lo tanto, cualquier imagen que sea bytes **debe**
exponerse en una URL pública, aunque sea transitoriamente.

## 3. Approaches considerados

| | A. Drive upload + público + borrar | B. Signed URL del host | D. Híbrido (passthrough + A) | F. Bucket GCS público |
|---|---|---|---|---|
| Cobertura | Todas | Todas | Todas | Todas |
| Infra nueva | Ninguna (reusa upload Drive existente) | Trait `OutputStorageRepository` + 3 adapters + endpoint ADP | Igual que A | Bucket + wiring |
| Cross-repo (ADP) | No | **Sí** | No | Sí |
| Testeable local | **Sí** (Drive es Google real) | No (signed URL → localhost, Google no alcanza) | Sí | Solo con bucket real |
| Exposición | Ventana de segundos, luego borrado | TTL ~15min, sin artefacto | Igual A | TTL lifecycle |
| Esfuerzo | ~6-8h | ~2-3d | ~7-9h | ~1-2d |

**Decisión: Approach A.** Razones:
- Cobertura total vía `executor.fetch_attachment_bytes` (que ya persiste bytes de
  toda fuente — inline/path/signed-url — en `OutputStorageRepository`).
- Cero infra nueva; reusa el patrón de upload multipart a Drive (ya usado por
  `create_from_docx`/`create_from_markdown`).
- Self-contained en colmena, sin coordinar con ADP.
- Testeable local **y** prod (Drive siempre es Google real).
- El delta de privacidad vs B es marginal: el signed URL de B también es
  "público mientras es válido". B no es "cero exposición", es "exposición
  time-boxed sin artefacto". No justifica el costo cross-repo + el no poder
  E2E-testear local.

Híbrido D (passthrough cuando la fuente ya es `SignedUrl`) queda **fuera de scope
v1** (YAGNI): v1 trata toda fuente uniforme (fetch bytes → upload). Es una
optimización futura.

## 4. Corroboración empírica (2026-06-12, contra Google real)

Los dos únicos riesgos de A se de-riskearon con llamadas directas a las APIs
(simulando lo que hará el feature) sobre un doc real compartido con
`agents@startti.co`:

| Paso | Resultado |
|---|---|
| Upload imagen (multipart, `image/png`) a `/upload/drive/v3/files` | **200** |
| `permissions.create {type:anyone, role:reader}` | **200** |
| `insertInlineImage` con `uri = https://lh3.googleusercontent.com/d/<file_id>` | **200** |
| `files.delete` del archivo temporal | **204** |
| Re-leer el doc tras borrar | imagen presente; `contentUri` = `lh*.googleusercontent.com` |

**Conclusiones (ambas decisiones cerradas):**
1. **Forma de URL:** `https://lh3.googleusercontent.com/d/<file_id>` es aceptada
   por `insertInlineImage` al primer intento, sin interstitial de virus-scan.
   (Fallback documentado: `https://drive.google.com/uc?export=view&id=<id>`.)
2. **Cleanup safe:** Google copia la imagen a su propio host durante el
   `batchUpdate` (síncrono). Borrar el archivo temporal de Drive **justo después
   del 200 es seguro** — la imagen sobrevive en el doc. Cero junk permanente.

## 5. Arquitectura

### 5.1 Superficie del tool (sin tool nuevo)

`gdocs_insert_image_after_text` gana un param `attachment_id`, alternativo a
`image_url`. **Exactamente uno** de los dos es requerido (validación XOR):

```
InsertImageAfterTextArgs {
    doc_id: String,
    anchor: String,
    image_url: Option<String>,      // path (i), existente
    attachment_id: Option<String>,  // path nuevo (A)
    occurrence: Option<u32>,
    width_pt: Option<f64>,
    height_pt: Option<f64>,
}
```

- Ambos presentes o ambos ausentes → `invalid_args`.
- `image_url` presente → comportamiento actual, directo (no toca executor/Drive).
- `attachment_id` presente → flujo A (§5.2).

### 5.2 Flujo del modo `attachment_id`

```
executor.fetch_attachment_bytes(attachment_id)   → StoredBytes { bytes, mime_type }
  ├─ validar mime ∈ {image/png, image/jpeg, image/gif}   (si no → invalid_args)
client.upload_image_to_drive(bytes, mime, name)  → DriveFileId   (multipart, SIN conversión)
client.set_anyone_reader(file_id)                → permiso anyone/reader
  uri = "https://lh3.googleusercontent.com/d/" + file_id
run_insert_image_after_text(ctx, doc_id, {anchor, uri, occurrence, w, h})  → EditResult
client.delete_drive_file(file_id)                → cleanup best-effort tras el 200
return EditResult (+ soft_warning si el delete falló)
```

`run_insert_image_after_text` ya existe (shipped); el modo attachment solo le
provee una `uri` derivada del Drive temporal.

### 5.3 Routing

El dispatcher se rutea **siempre por el camino `via_executor`** (necesita
`executor.fetch_attachment_bytes`), igual que `attachment_run_python` /
`sql_bulk_insert`. Dentro ramifica:
- `image_url` → `run_insert_image_after_text` directo (no usa el executor).
- `attachment_id` → flujo A.

`dispatch_gdocs_insert_image_after_text_via_executor(executor, tool_call, session_id)`
reemplaza el wire actual en `dag_tool_executor.rs` (bloque via_executor temprano,
líneas ~831-871). El dispatcher URL-only existente
(`dispatch_insert_image_after_text`) se mantiene como fallback interno reusado por
el branch `image_url`.

## 6. Métodos nuevos en `DocsClient` (+ HTTP impl)

Aditivos al trait `DocsClient` (interno a colmena; sin impls externas → no rompe ADP):

- `upload_image_to_drive(&self, bytes: Vec<u8>, mime: &str, filename: &str) -> Result<String, DocsError>`
  Multipart a `/upload/drive/v3/files?uploadType=multipart&fields=id`, con el
  `mime` real de la imagen (NO un `mimeType` de conversión a Doc). Devuelve el
  `file_id`. Reusa el patrón de `create_from_docx`.
- `set_anyone_reader(&self, file_id: &str) -> Result<(), DocsError>`
  `permissions.create {role:"reader", type:"anyone"}` sobre `file_id`. Variante
  del `share` existente (que usa `type:"user"`).
- `delete_drive_file(&self, file_id: &str) -> Result<(), DocsError>`
  `DELETE /drive/v3/files/{file_id}`. (Distinto de `delete_permission`, que borra
  un permiso, no el archivo.)

URL de contenido: construida en el dispatcher como
`https://lh3.googleusercontent.com/d/{file_id}` (corroborado §4).

Mock `MockDocsClient` (tests): agregar las 3 con expectativas controlables.

## 7. Manejo de errores (cleanup transaccional)

- mime no-imagen / attachment inexistente → `invalid_args` accionable (antes de
  cualquier upload).
- Falla en `set_anyone_reader` o en el insert (después del upload) → **borrar el
  `file_id` subido** antes de retornar el error (no dejar huérfanos públicos).
- Insert OK pero `delete_drive_file` final falla → **no romper el insert**:
  retornar el `EditResult` con un `soft_warning` indicando que el archivo temporal
  `file_id` quedó sin borrar (para un GC futuro).
- Filename temporal con prefijo reconocible: `colmena-tmp-img-<short_id>.<ext>`,
  para que un GC futuro encuentre huérfanos por nombre.

## 8. Testing

### 8.1 Unit (wiremock / MockDocsClient)
- `attachment_id` mode happy path: mock `fetch_attachment_bytes` (vía un executor
  de test o inyección), mock `upload_image_to_drive`→file_id,
  `set_anyone_reader`→ok, `batch_update`→ok (revisión), `delete_drive_file`→ok.
  Assert la secuencia de llamadas y que la `uri` pasada al batchUpdate es la lh3.
- Cleanup transaccional: si `batch_update` falla, assert que `delete_drive_file`
  se llamó con el `file_id` subido (no huérfano).
- Soft-warning: si `delete_drive_file` final falla, assert `EditResult` OK + warning.
- Validaciones: XOR `image_url`/`attachment_id`; mime no-imagen → `invalid_args`.
- `build_insert_image_request` / lh3 URL builder: unit puro.

### 8.2 E2E live (real Google Docs)
Sobre el doc compartido `agents@startti.co`: usar un attachment real (p.ej. salida
de `image_generation`, o un upload inline) y `gdocs_insert_image_after_text` con
`attachment_id`. Verificar:
1. La imagen aparece en el doc (inlineObject con `contentUri` googleusercontent).
2. El archivo temporal de Drive fue borrado (no queda huérfano) — listar por
   nombre `colmena-tmp-img-*`.
3. La imagen sobrevive (ya corroborado en §4, re-confirmar en el flujo real).

Grafo de referencia: `tests/graphs/agents/gdocs_insert_image_from_attachment_e2e.json`
(doc_id placeholder `<YOUR_DOC_ID>`).

**Gotcha operacional:** los tools gdocs Drive-based (upload/delete) usan el scope
`drive.file` — funcionan sobre archivos **creados por la app** (el temporal que
subimos), así que NO chocan con el caveat de docs compartidos (§45 dev guide). El
`insertInlineImage` usa la Docs API (`documents` scope) → OK sobre docs
compartidos.

## 9. Lista de archivos a tocar

- `src/libs/colmena/src/gdocs/domain/traits.rs` — 3 métodos nuevos en `DocsClient`.
- `src/libs/colmena/src/gdocs/infrastructure/http_client.rs` — impl HTTP de los 3.
- `src/libs/colmena/src/gdocs/application/insert.rs` — sin cambios (reusa
  `run_insert_image_after_text`); quizá exponer un helper si hace falta.
- `.../llm_synthetic_tools/gdocs_tools.rs` — `InsertImageAfterTextArgs` gana
  `attachment_id`; nuevo `dispatch_..._via_executor` con el branch A; validación XOR.
- `.../dag_engine/infrastructure/dag_tool_executor.rs` — rutear
  `gdocs_insert_image_after_text` al bloque `via_executor`.
- `src/libs/colmena/text/tools/gdocs.yaml` — doc del param `attachment_id`.
- `docs/developer_guide/45_gdocs.md` — actualizar la fila del tool.
- `docs/CHANGELOG_2026-06.md` — §nueva.
- `docs/BACKLOG.md` — marcar paths ii/iii como cubiertos por A (o cerrar el item).
- Tests + E2E graph.

## 10. Fuera de scope (v1)

- Híbrido D (passthrough directo cuando la fuente ya es `SignedUrl`) — optimización
  futura. v1 trata todo uniforme (fetch bytes + upload).
- Path `image_url` (i) — intacto, sin cambios.
- Borrar imágenes de un doc (no existe tool de delete-image) — fuera de scope.

## 11. Preguntas abiertas

Ninguna material. Los dos riesgos (forma de URL, cleanup safe) fueron corroborados
empíricamente (§4).
