# Diseño — Resolución viva de attachments para tools `fetch_attachment_bytes`

**Fecha:** 2026-06-20
**Estado:** spec aprobado para implementación (Approach A)
**Autor:** brainstorming daniel@startti.co + Claude

## 1. Problema

Los attachments **generados/editados en el mismo turno** (output de
`image_generation`/`image_edit`/`tts` mid-loop) no son resolubles por los tools
que usan `DagToolExecutor::fetch_attachment_bytes` / `fetch_attachment_stream`:
`gdocs_insert_image_after_text` (modo attachment), `sql_bulk_*`,
`attachment_run_python`. Falla con `no attachment_catalog / not in catalog`.

**Root cause:** `lookup_storage_key` resuelve **solo** contra
`self.attachment_catalog`, un snapshot que `llm.rs` arma una vez al inicio del
llm_call. Un attachment registrado mid-loop no está en el snapshot.

**Asimetría clave:** `http_request` (`$attachment:<id>`) NO sufre esto — usa un
`AttachmentStreamResolver` (`AttachmentStreamResolverImpl`) que consulta el
`AttachmentRegistry` **en vivo** (`lookup_by_document_id(session, document_id)`
→ `storage.read_stream(storage_key)`). Por eso http puede mandar una imagen
recién generada a un API call en el mismo turno; los demás tools no.

## 2. Objetivo

Que cualquier imagen/artefacto generado, editado o subido por el usuario sea
usable por **cualquier** tool consumidor de attachments en el mismo turno —
igual que ya lo es para `http_request`. Unificar el path de resolución.

## 3. Approach A (elegido)

Cablear el `AttachmentRegistry` vivo dentro del `DagToolExecutor` y hacer que
`lookup_storage_key` resuelva **snapshot-first, live-registry-fallback**.

Descartado **B** (re-sync del snapshot mid-loop): más frágil (hay que hookear
cada tool productor) y no unifica con el path de http. **A** reusa infra ya
probada (`AttachmentRegistry`, el mismo que usa el resolver de http) y arregla
todos los tools `fetch_attachment_bytes` de una.

## 4. Cambios

### 4.1 `DagToolExecutor` (`dag_tool_executor.rs`)
- Nuevo campo `attachment_registry: Option<Arc<dyn AttachmentRegistry>>`
  (default `None`).
- Nuevo builder `with_attachment_registry(self, reg) -> Self`.
- `lookup_storage_key` pasa de **sync** a **async**:
  1. Si hay snapshot y el `document_id` está → devolver su `storage_key`
     (fast-path, sin DB).
  2. Si no (snapshot ausente o miss) **y** hay `attachment_registry` **y**
     `agent_session_id` → `registry.lookup_by_document_id(agent_session_id,
     document_id).await?`:
     - row con `storage_key` → devolver.
     - row sin `storage_key` → error existente "no storage_key".
     - `None` → error "not found in catalog nor live registry".
  3. Si no hay snapshot ni registry → error "not wired" existente.
- `fetch_attachment_bytes` / `fetch_attachment_stream` ya son `async` → solo
  agregan `.await` al `lookup_storage_key`.

**Nota:** preservar `agent_session_id` (ya existe como campo del executor) para
la query del registry. Verificar todos los callers de `lookup_storage_key`
(hoy: `fetch_attachment_bytes`, `fetch_attachment_stream`; revisar
`lookup_attachment_meta`) y propagar el `async` o mantener una variante sync
que solo mire el snapshot si algún caller no puede ser async.

### 4.2 `llm.rs` (sitio de construcción del executor)
Donde se arma el `DagToolExecutor` (mismo bloque donde ya está
`attachment_registry` + `agent_session_id` para construir el snapshot, ~línea
2024), pasar el registry al executor vía `.with_attachment_registry(reg.clone())`.
El snapshot se sigue construyendo igual (fast-path); el registry es el fallback.

## 5. Semántica / compatibilidad
- **Aditivo.** El campo default `None` → comportamiento idéntico cuando no se
  wirea (snapshot-only). ADP no requiere cambios: el executor lo construye
  `llm.rs` (colmena) desde el `attachment_registry` que ADP ya provee al LLM
  node; si está, el fallback queda activo automáticamente.
- **Perf:** 1 query extra al registry **solo** en el miss del snapshot
  (típicamente un attachment generado mid-turn). Los pre-turn siguen siendo
  in-memory.
- **Errores:** se mantienen; el mensaje de "not found" se amplía a "ni en el
  snapshot ni en el registry vivo" cuando hay registry.

## 6. Testing
- **Unit (executor):** mock `AttachmentRegistry` que devuelve una row para un
  `document_id` que NO está en el snapshot → `fetch_attachment_bytes` lo
  resuelve vía el fallback vivo (con un mock storage que devuelve bytes).
  + caso snapshot-hit (no toca el registry) + caso both-miss (error claro).
- **E2E live:** el grafo `gdocs_insert_image_from_attachment_e2e.json`
  (generate_image → gdocs_insert attachment_id en el mismo turno) debe pasar
  **contra el worker** (registry wired). Local sigue limitado (CLI no cablea el
  registry — ver caveat). Alternativamente, integration test con registry +
  storage wired.

## 7. Caveat
El CLI `dag_engine run` local no cablea el `attachment_registry`, así que el
fallback vivo no opera localmente — la verificación full-loop va contra el
worker desplegado, o vía integration test con un registry real. El código del
fallback es unit-testeable con mocks.

## 8. Fuera de scope
- Cambiar `image_generation`/`edit`/`tts` (ya registran al registry — el smoke
  graph de http lo prueba).
- Tocar el path de `http_request` (ya usa el resolver vivo).
- Wirear el registry en el CLI local (item aparte).
