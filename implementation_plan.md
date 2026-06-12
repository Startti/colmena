# Plan — Quick win: `gdocs_insert_image_after_text`

## Hallazgo que simplifica el scope
El backlog asumía "nuevo método `DocsClient::insert_inline_image_after_text`".
**No hace falta.** `insert.rs` ya resuelve el anchor a un índice (`find_anchor`)
y emite un `Request` JSON genérico vía `DocsClient::batch_update`. Insertar
imagen = mismo patrón con un request `insertInlineImage` en vez de `insertText`.
Cero cambios en el trait.

## Decisión de scope — 3 caminos para el `uri`
La Docs API `InsertInlineImageRequest` exige una **URL públicamente accesible**.

| Path | Fuente del uri | Esfuerzo | Cubre |
|------|----------------|----------|-------|
| **(i) URL pública** | arg `image_url` que pasa el LLM | ~2h | URLs públicas, outputs de http_request |
| (ii) attachment SignedUrl | `attachment_id` cuyo source es `SignedUrl` | +1-2h (helper executor + via_executor dispatcher) | adjuntos subidos/fetched con signed URL |
| (iii) attachment Path/Inline | subir bytes a Drive + hacer público | ~6-8h | imágenes generadas (image_generation), inline |

**Recomendación: shippear (i) URL-only ahora** (quick win real, sin tocar
executor/catalog), y dejar (ii)/(iii) como follow-up en BACKLOG. Razón: (i)
es el corte más limpio y desbloquea el caso "tengo una URL de imagen, metela
en el doc"; (ii) y (iii) agregan plumbing de attachments que duplica la
superficie.

## Cambios (path i)
1. **`gdocs/application/insert.rs`** — nuevo `InsertImageInput { tab_id?, anchor,
   occurrence, image_url, width_pt?, height_pt? }` + `run_insert_image_after_text`:
   reusa `find_anchor` → índice; arma
   `{ "insertInlineImage": { "location": {"index", "tabId"?}, "uri", "objectSize"? } }`;
   llama `client.batch_update(doc_id, reqs, Some(&snap.revision_id))`. Devuelve `EditResult`.
2. **`gdocs_tools.rs`** — `TOOL_INSERT_IMAGE_AFTER_TEXT` const + `InsertImageArgs` +
   `tool_insert_image_after_text()` builder + `dispatch_insert_image_after_text`.
3. **Router (`dag_tool_executor.rs`)** — `is_gdocs_tool` + match arm dispatch.
4. **`toolkit_packages.rs`** — agregar al alias `gdocs` (write tool, NO en `gdocsread`);
   actualizar test de conteo.
5. **`text/tools/gdocs.yaml`** — summary + description (con nota: URL pública;
   attachments vienen en v1.1).
6. **`mod.rs`** — re-export.
7. **Docs**: `41_builtin_tools_index.md`, `45_gdocs.md`.
8. **Tests**: insert.rs unit (mock `batch_update`, assert shape `insertInlineImage`
   + índice correcto) + args deserialization.
9. **E2E live**: insertar una imagen pública (URL) después de un anchor en un
   doc compartido; reporte amigable.
10. **CHANGELOG §32 + BACKLOG**: marcar item shipped (path i); (ii)/(iii) quedan
    como follow-up con scope clarificado.

## Riesgos
- La URL debe ser pública y la imagen PNG/JPEG/GIF ≤50MB, uri ≤2000 chars.
  El dispatcher valida shape mínimo; si la URL es inaccesible, Docs devuelve
  error → se paraphrasea al LLM.
- `objectSize` opcional (width_pt/height_pt en PT); si se omite, Docs usa el
  tamaño nativo.
