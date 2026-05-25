# Attachment Uniform Resolution — Design Spec

**Status:** Approved (2026-05-25)
**Author:** daniel@startti.co + Claude
**Component:** `llm/domain/attachments/`, `llm/infrastructure/attachments/`, `dag_engine/infrastructure/nodes/{http,llm,image_generation,image_edit,tts}.rs`, `conversation_attachments` table

## Motivation

El nodo LLM acepta documentos por tres caminos (inline base64, URL firmada, generado por un tool previo como `image_generation`/`tts`), pero los tres se comportan de forma asimétrica río abajo. Hoy:

1. **Inline:** los bytes se descartan tras subirlos al provider. Si en un turno posterior el LLM quiere reenviar el doc a un endpoint downstream (ej. KB de ADP vía `http_request` multipart), no hay forma de recuperar los bytes. **Roto.**
2. **URL firmada:** los bytes no se persisten localmente, sólo la URL. El placeholder `$attachment:<id>` falla porque `OutputStorageRepository::read_stream` no conoce las URLs firmadas; la única vía hoy es que el LLM tenga la URL como string literal y la pase como tal — pero el catálogo no le expone la URL al modelo. **Parcialmente roto.**
3. **Generado:** funciona. Los bytes están en `OutputStorageRepository` y `$attachment:<attachment_id>` resuelve directo.

Adicionalmente hay **dos namespaces de ID** conviviendo (`document_id` para el catálogo de `load_attachment`, `attachment_id` para los tool outputs generados), lo cual genera confusión cuando el LLM tiene que construir un placeholder.

Y un costo de tokens no controlado: **el contenido de los docs se autoinyecta al primer turno** independientemente de si el modelo los necesita o no.

El objetivo de este diseño: cualquier doc que entre o salga del nodo LLM puede ser reenviado vía `$attachment:<id>` por cualquier nodo consumidor (`http_request` multipart en primera instancia, más adelante otros), con un único namespace, costo de tokens explícitamente controlado por el modelo, y arquitectura limpia detrás de un puerto en `domain`.

## Constraints

- **Costo de tokens es la primera prioridad.** Ninguna política nueva debe aumentar el costo de input tokens respecto al status quo. Idealmente lo reduce.
- **Hexagonal architecture.** Cualquier nueva pieza de lógica de resolución vive en `domain`/`application`; los detalles de storage o DB en `infrastructure`.
- **Backward compat con ADP.** ADP worker consume colmena develop directamente. Cualquier cambio rompiente de `EngineConfig`/`ColmenaEngine`/tool output schema debe sweepearse contra `apps/service/ia/platform/{worker,api}/src/` antes de pushear.
- **Migración SQL manual.** ADP usa `prisma migrate deploy` exclusivamente. Cualquier cambio a `conversation_attachments` se hace con SQL manual.
- **Storage cost bounded.** Persistir bytes para los tres orígenes implica más storage que hoy. Necesita TTL automático.

## Decisions

### D1 — Persistencia uniforme de bytes en `OutputStorageRepository`

Todo documento, sin importar su origen, persiste sus bytes en `OutputStorageRepository` en el momento del registro:

| Origen | Cuándo se persiste |
|---|---|
| Inline (`data:` o base64) | Al parsear el input del LLM node. Los bytes se streamean al storage antes de descartar el `retained_inline_bytes`. |
| URL firmada | Al descargarla por primera vez (en el primer turno del LLM node). Los bytes se streamean tanto al storage como al provider Files API (si aplica). |
| Generado por tool (`image_generation`/`tts`/`image_edit`) | Como hoy: el tool node llama `OutputStorageRepository::store()` antes de devolver el resultado. |

Una vez persistido, el resto del sistema lo trata uniformemente: bytes recuperables desde `OutputStorageRepository::read_stream(storage_key)`.

**Rejected alternatives:**
- Persistir sólo inline (mantener URL firmada como referencia lazy): genera dos caminos en el resolver que diferencian por `source_kind`. Más complejidad sin ganar costo apreciable (las URLs firmadas suelen expirar en horas, así que dependerás de re-fetch que puede fallar).
- Lazy persist on first downstream use: aumenta latencia del primer `$attachment:` y deja una ventana donde el storage puede haber colapsado.

### D2 — `document_id` es el único namespace público

El LLM y los nodos consumidores ven un único string identificador por doc: `document_id`. El `storage_key` interno del `OutputStorageRepository` queda como detalle de infraestructura, no expuesto en tool results, catálogos, ni placeholders.

Reglas:
- Para inputs del user, `document_id` viene de `files[].id` si el caller lo proveyó; sino se genera (`att_<8-char-uuid>`).
- Para artifacts generados, el tool node genera un `document_id` legible derivado del filename del provider (ej. `img_revenue_chart_001` para `revenue_chart_001.png`) o un UUID si no hay filename útil.
- El mapeo `document_id → storage_key` vive en `conversation_attachments`.

**Rejected alternatives:**
- Storage key opaco (UUID) como ID público (Opción A del brainstorm): pierde el `files[].id` humano-friendly de ADP, rompe compat.
- Dos IDs visibles en el catálogo (Opción B): superficie de error doble.
- Canónico + alias (Opción C): complejidad sin ganancia clara sobre D.

### D3 — Placeholder uniforme `$attachment:<document_id>`

El LLM construye literalmente el string `"$attachment:<document_id>"` en sus tool call args (ej. en el body del `http_request`). El nodo consumidor delega la resolución al `AttachmentStreamResolver` (D4) antes de procesar el body.

El placeholder funciona en **cualquier campo string** del body, recursivamente en objetos y arrays.

### D4 — Trait `AttachmentStreamResolver` en `domain`

Nuevo puerto:

```rust
// src/libs/colmena/src/llm/domain/attachments/resolver.rs

#[async_trait]
pub trait AttachmentStreamResolver: Send + Sync {
    async fn resolve(
        &self,
        agent_session_id: &str,
        document_id: &str,
    ) -> Result<StoredStream, AttachmentResolveError>;
}

pub enum AttachmentResolveError {
    NotFound { document_id: String },
    StorageError(StorageError),
    Expired { document_id: String },
}
```

Implementación en `infrastructure/attachments/stream_resolver_impl.rs`: compone `ConversationAttachmentRegistry` (lookup `document_id → storage_key`) + `OutputStorageRepository` (lectura streaming). Actualiza `last_used_at` en cada resolución.

Inyectado vía `ServiceContainer` en cualquier nodo que lo necesite. Primer consumidor: `http_request`. Disponible sin cambios para futuros nodos (SQL blob upload, future S3 upload, email attach, etc.).

### D5 — Catálogo en el system message (no en `load_attachment` description)

El catálogo se mueve de la descripción del tool `load_attachment` a un bloque dedicado **prepended al system message del nodo LLM**, renderizado dinámicamente cada turno desde `conversation_attachments` filtrado por `agent_session_id`.

Cuando no hay docs en la sesión, el bloque no se inyecta (0 tokens extra).

**Formato del catálogo:**

```
Documents available in this session:

[Q3_report]
  filename: Q3_Financial.pdf · application/pdf · 1.2 MB
  description: Q3 2026 financial results — revenue, EBITDA, OpEx by region.
  origin: uploaded by user
  created: 2026-05-25 14:32 UTC
  usage: load_attachment("Q3_report") to read · "$attachment:Q3_report" to forward

[img_revenue_chart_001]
  filename: revenue_chart.png · image/png · 348 KB
  description: Bar chart Q1-Q3 2026 revenue
  origin: generated by image_generation
  created: 2026-05-25 14:48 UTC
  usage: load_attachment("img_revenue_chart_001") to read · "$attachment:img_revenue_chart_001" to forward
```

**Campos en el catálogo:**
- `document_id` (cabecera entre `[...]`)
- `filename`, `mime_type`, `size` (human-readable)
- `description` (auto-summary o explícita)
- `origin`: `uploaded by user` | `generated by <tool_name>`
- `created`: timestamp UTC del registro
- `usage`: dos líneas explícitas con las acciones permitidas y la sintaxis correcta

**Rejected alternative:** mantenerlo en la descripción del tool `load_attachment` (status quo). Razón: la descripción del tool sólo se ve si el tool está enabled, y queremos el catálogo visible aun cuando el modelo no tenga `load_attachment` enabled (caso en que solo necesita reenviar).

### D6 — Nunca autoinyectar contenido del doc en el primer turno

El nodo LLM **NO** incluye los archivos en el mensaje user del primer turno. El modelo recibe sólo el catálogo (D5) y decide qué hacer:

- Leer → `load_attachment("<document_id>")` (siguiente turno tiene el contenido).
- Reenviar → tool call con `"$attachment:<document_id>"` en los args.
- Ignorar → 0 tokens consumidos por contenido del doc.

**Trade-off explícito:** casos de uso "te paso un PDF, contestáme X" requieren ahora un round-trip extra (turn 1 dispara `load_attachment`, turn 2 contesta). Aceptable dado el objetivo de costo. Si un grafo en específico quisiera el comportamiento viejo, se puede agregar una flag de config futura (`auto_load_attachments: true`) sin romper este default. **Fuera de scope para esta entrega.**

**Rejected alternatives:**
- Heurística por tamaño (autoinyectar si < N KB): comportamiento mágico no determinístico desde la perspectiva del LLM.
- Flag por archivo (`files[i].auto_load`): superficie de API innecesaria.

### D7 — `load_attachment` es ephemeral por turno (marker en history, no contenido)

**Durante el turno (intra-turn):**
- Al ejecutar `load_attachment(document_id)`, el resolver inyecta un synthetic `user_with_files` con el doc adjunto. El modelo lo lee normalmente en los pasos siguientes del mismo turno (incluyendo loops de tool calling).

**Al cerrar el turno (post-process antes de persistir a `llm_node_history`):**
- El motor recorre los mensajes nuevos del turno y reemplaza cada synthetic `user_with_files` por un marker:
  ```
  user: [load_attachment("<document_id>") was invoked. Document content is no longer in context. Call load_attachment again if you need to re-read.]
  ```
- Las respuestas del assistant (que pueden contener análisis derivado del doc) se preservan **intactas** — el modelo "recuerda lo que dijo" sobre el doc.
- Los demás tool results (no relacionados con `load_attachment`) se preservan intactos.

**Provider Files API:**
- `provider_file_id` se cachea en `conversation_attachments` por `(agent_session_id, document_id, provider_kind)`.
- Si el modelo re-invoca `load_attachment` del mismo doc en un turno futuro y el `provider_file_id` no expiró (24h), se reusa.
- Si expiró, se re-sube desde `OutputStorageRepository` (bytes siempre disponibles por D1).

**System prompt addition (al final del bloque catálogo de D5):**

```
load_attachment results are ephemeral: the document content stays in context only
for the turn in which you invoked the tool. Call load_attachment again if you need
to re-read it in a future turn.
```

**Costo neto comparado con status quo:**
- Lectura inicial: 1× los tokens del doc (en el turno de la llamada). Igual que hoy.
- Turnos siguientes sin re-llamar: ~20 tokens (marker). Hoy: el doc completo cada turno.
- Re-llamada en turno futuro: 1× otra vez. Acotado por la decisión del modelo, no por la longitud de la sesión.

**Rejected alternatives:**
- Persistir el doc en history (status quo): viola D6's spirit y el objetivo de costo.
- Flag ephemeral por llamada (Opción C del brainstorm): puede agregarse después si B no alcanza; añade superficie de tool description sin ganancia clara hoy.
- Compactación automática por threshold (Opción D): más complejidad sin necesidad demostrada.

### D8 — Auto-registro de artifacts generados en `conversation_attachments`

Los nodos `image_generation`, `image_edit`, y `tts` cambian su comportamiento:

1. Storean los bytes vía `OutputStorageRepository::store()` (igual que hoy) → `storage_key`.
2. **Registran** una fila en `conversation_attachments` con:
   - `document_id`: derivado del filename del provider o `att_<8-char-uuid>` si no hay nombre útil
   - `storage_key`: el devuelto por el store
   - `agent_session_id`: del contexto del run
   - `origin`: `generated_by:<tool_name>`
   - `mime_type`, `size_bytes`, `filename`, `created_at`
3. Tool result devuelve únicamente:
   ```json
   { "document_id": "img_revenue_chart_001", "mime_type": "image/png", "size_bytes": 348000 }
   ```

**Breaking change** respecto al schema actual del tool result:
- Campo viejo `attachment_id` → `document_id` (mismo significado, nuevo nombre).
- Campo `url` desaparece del tool result.

Para reducir el blast radius en ADP, durante una versión de transición el tool result puede incluir **ambos**:
```json
{
  "document_id": "img_revenue_chart_001",
  "attachment_id": "img_revenue_chart_001",  // deprecated alias
  "mime_type": "image/png",
  "size_bytes": 348000
}
```
Con un warning en los logs del nodo. Se elimina el alias en una release posterior, una vez ADP se haya migrado.

`url` no se preserva en el alias porque su valor cambia de naturaleza con D1 (ahora vive detrás de `OutputStorageRepository` con `storage_key` interno, no necesariamente accesible como HTTP). El frontend de ADP que hoy renderiza la imagen tiene que migrar a pedirla por `document_id` a un endpoint propio del backend de ADP.

### D9 — Schema de `conversation_attachments`

Tabla existente. Cambios necesarios (migración SQL manual):

```sql
ALTER TABLE conversation_attachments
  ADD COLUMN storage_key TEXT,
  ADD COLUMN origin TEXT NOT NULL DEFAULT 'unknown',
  ADD COLUMN last_used_at TIMESTAMPTZ;

-- Backfill para filas existentes (best effort: dejar storage_key nulo,
-- las filas inline-source viejas no son recuperables y eventualmente expiran):
UPDATE conversation_attachments
SET origin = CASE
  WHEN provider = 'generated' THEN 'generated_by:unknown'
  WHEN source_kind = 'inline' THEN 'user_upload'
  WHEN source_kind = 'signed_url' THEN 'user_upload'
  WHEN source_kind = 'path' THEN 'user_upload'
  ELSE 'unknown'
END
WHERE origin = 'unknown';

CREATE INDEX idx_conv_attachments_session_used
  ON conversation_attachments (agent_session_id, last_used_at);
```

Columnas existentes que se preservan: `document_id`, `agent_session_id`, `filename`, `mime_type`, `size_bytes`, `provider`, `provider_file_id`, `source_kind`, `source_value`, `description`, `label`, `refreshed_at`, `created_at`.

### D10 — TTL por `agent_session_id` con `last_used_at`

- `last_used_at` se actualiza en cada `AttachmentStreamResolver::resolve()` y en cada `load_attachment` invocation.
- Un background job (cron job o GCS lifecycle policy) borra blobs de `OutputStorageRepository` cuyo `last_used_at` (vía join con `conversation_attachments`) es más viejo que N días.
- Default N = 7 días, configurable por env var `COLMENA_ATTACHMENT_TTL_DAYS`.
- Cascade: cuando se borra la fila de `conversation_attachments`, se borra el blob asociado en `OutputStorageRepository`.

El job vive como un binario nuevo (`cargo run --bin attachment_gc`) que puede correr como cron en Cloud Run, o triggear vía Cloud Scheduler. Detalle de implementación en plan, no en spec.

## Architecture

```
Input phase
─────────────────────────────────────────────────────────────
  llm_call node receives input
    │
    ├─ files[i].data (inline)   ─┐
    ├─ files[i].url (signed_url)─┤
    │                            ▼
    │                     Persist bytes to
    │                     OutputStorageRepository.store()
    │                                │
    │                                ▼
    │                     Register row in conversation_attachments
    │                       (document_id, storage_key, origin="user_upload", ...)
    │
    └─ Tool node emits artifact ──┐
                                  ▼
                          OutputStorageRepository.store()
                                  │
                                  ▼
                          Register row in conversation_attachments
                          (document_id, storage_key, origin="generated_by:<tool>", ...)

LLM phase (every turn)
─────────────────────────────────────────────────────────────
  llm_call node executes
    │
    ├─ Load conversation_attachments by agent_session_id
    │
    ├─ Render catalog block → prepend to system message
    │
    ├─ Load llm_node_history (with markers, no doc content in past turns)
    │
    └─ Send to provider
          │
          ▼
       Model decides:
         ├─ load_attachment(id)  → resolver injects synthetic user_with_files
         │                        (this turn only; marker on persist)
         │
         ├─ tool_call(..., "$attachment:<id>", ...)
         │   │
         │   ▼
         │   Node receives args. Pre-execution hook expands $attachment:.
         │   AttachmentStreamResolver.resolve(agent_session_id, document_id)
         │     ├─ lookup conversation_attachments → storage_key
         │     ├─ OutputStorageRepository.read_stream(storage_key) → StoredStream
         │     └─ update last_used_at
         │   Node (http_request) streams part to downstream endpoint.
         │
         └─ assistant final message
                  │
                  ▼
              Post-process turn:
                replace user_with_files synthetic msgs with markers
                persist to llm_node_history
```

## Migration & Backward Compat

### Colmena → ADP worker

ADP worker (`apps/service/ia/platform/{worker,api}/src/`) consume colmena develop. El sweep obligatorio antes de pushear:

1. **Tool result schema** — buscar consumers de `attachment_id` o `url` en el output de `image_generation`/`image_edit`/`tts`. Cambiar a `document_id` o usar el alias temporal (D8).
2. **Engine config** — confirmar que `ColmenaEngine` y `EngineConfig` no expongan tipos rotos. Esta entrega no toca esas signatures; sólo agrega un nuevo trait `AttachmentStreamResolver` opcional vía `ServiceContainer`.

### ADP frontend / API

- El frontend hoy renderiza imágenes generadas usando el campo `url` del tool result. Tiene que migrar a:
  - Recibir `document_id` del tool result.
  - Pedir la URL renderizable al backend de ADP (un endpoint nuevo `GET /attachments/:document_id/url` que firme una URL apuntando al storage del worker).
- ADP API necesita un nuevo endpoint que dado un `document_id` y un `agent_session_id` (autorización), devuelva una URL firmada o el blob proxy.

**Decision para esta entrega:** durante el período de transición, el tool result devuelve `document_id` + el alias `attachment_id` (D8). El `url` no se preserva. ADP frontend tiene N semanas para migrar. Coordinar con el equipo de ADP el cronograma.

### Auto-inject removal

Grafos que hoy dependen de "doc en contexto turn 1" sin llamar `load_attachment` explícitamente van a romperse silenciosamente (el modelo recibe sólo el catálogo y puede contestar con "no veo el archivo"). Mitigación:

- Revisar todos los grafos de ADP que reciben `files[]`. Si un grafo asume autoinyección, agregar instrucción explícita al system prompt para que el modelo llame `load_attachment` cuando corresponda.
- Considerar agregar una flag de config `auto_load_attachments: true` futura para reactivar el comportamiento viejo en grafos específicos (fuera de scope acá).

## Testing

### Unit tests

- `AttachmentStreamResolverImpl::resolve`:
  - Document encontrado → `StoredStream` con bytes correctos.
  - Document no encontrado → `NotFound`.
  - `agent_session_id` mismatch → `NotFound` (aislamiento de sesiones).
  - `last_used_at` se actualiza tras resolución exitosa.
- `ConversationAttachmentRegistry`:
  - Auto-registro de artifact generado (origin=`generated_by:image_generation`).
  - Registro de inline (origin=`user_upload`, storage_key set tras persist).
  - Registro de signed URL (idem).

### Integration tests

- Test graph `tests/graphs/agents/upload_inline_to_endpoint.json`:
  - Input con doc inline.
  - LLM call con instrucción: "subí el archivo al endpoint X".
  - Mock HTTP server recibe multipart con los bytes correctos.
- Test graph `tests/graphs/agents/upload_signed_url_to_endpoint.json`: igual pero con URL firmada.
- Test graph `tests/graphs/agents/forward_generated_artifact.json`:
  - LLM call → image_generation tool → http_request tool con `$attachment:<id>` → endpoint recibe la imagen.
- Test graph `tests/graphs/agents/load_attachment_ephemeral.json`:
  - Turn 1: load_attachment, contesta.
  - Turn 2: history persistido tiene marker, no contenido.
  - Turn 2: vuelve a llamar load_attachment, funciona (re-sube si expired).

### Edge cases a cubrir

- Doc con `document_id` colisionante entre dos sesiones distintas → resolver respeta `agent_session_id`.
- Doc expirado por TTL → resolver devuelve `Expired`, LLM recibe error claro como tool result.
- Re-resolución repetida del mismo doc en un turno → no causa N reads (caching opcional intra-turn, no obligatorio en v1).
- Multipart body con N placeholders mezclados con N URLs literales → todos resuelven en paralelo, fail-fast si alguno falla.

## Risks

1. **Storage cost growth.** Con TTL=7d default y volúmenes desconocidos, hay que medir uso real de GCS bytes después del deploy. Mitigación: tunear TTL más agresivo si crece más de lo esperado, o agregar lifecycle policy en GCS lado-bucket independiente del cleanup interno.
2. **ADP frontend regression.** Si el frontend no migra antes de que removamos el alias `attachment_id`, las imágenes generadas dejan de renderizar. Mitigación: deprecation warning visible en logs + coordinar el sunset del alias con un release explícito.
3. **Round-trips extra por D6.** Si los grafos de Q&A sobre docs no migran sus system prompts para instruir al modelo a llamar `load_attachment`, el modelo puede contestar "no veo el archivo" sin pedirlo. Mitigación: documentar el patrón en `docs/developer_guide/31_load_attachment.md`, revisar grafos críticos de ADP.
4. **Provider Files API quota.** Re-subir el mismo doc cada vez que el modelo llama `load_attachment` puede pegar quotas si la sesión es muy larga. Mitigación: cache de `provider_file_id` en `conversation_attachments` con TTL de 24h ya existe; sólo se re-sube cuando expira.
5. **Migración SQL en ADP.** La migración de `conversation_attachments` tiene que correr en ADP via `prisma migrate deploy` con SQL manual. Mitigación: incluir el SQL exacto en el plan de implementación.

## Out of Scope

- Flag `auto_load_attachments: true` por nodo para preservar el comportamiento viejo.
- Flag `ephemeral: false` en `load_attachment` para persistir contenido específico en history.
- Compactación automática del history por threshold de tokens.
- Endpoint ADP `GET /attachments/:document_id/url` (responsabilidad del equipo ADP, no de colmena).
- Cleanup binario `attachment_gc` (detalle de implementación, va en el plan).
- Soporte para `$attachment:` en headers, query params, o URL path de `http_request` (sólo body por ahora).
