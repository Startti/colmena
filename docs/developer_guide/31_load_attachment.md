# Load Attachment — Documentos on-demand dentro del loop LLM

> **Estado:** Disponible desde 0.4.0
> **Spec:** [docs/superpowers/specs/2026-05-13-load-attachment-design.md](../superpowers/specs/2026-05-13-load-attachment-design.md)

## Rol — reader general-purpose para cualquier mime

`load_attachment` es **la herramienta primaria** para leer el contenido literal de un attachment dentro del loop LLM, **para cualquier mime type** (PDF, imagen, markdown, plain text, código, audio cuando el provider lo soporte, y sí, CSV/XLSX cuando el LLM necesita ver filas verbatim).

Items 13 + auto-summary + `attachment_run_python` (2026-06-09 / 2026-06-10) **agregaron paths más eficientes para el caso tabular específico** — no reemplazaron `load_attachment`. La regla:

| Pregunta del usuario | Tool primario |
|---|---|
| "Qué columnas tiene este CSV?" | Catalog auto-summary (gratis, ya en system message) |
| "Cuál producto tiene precio más alto?" | `data_run_python` (math server-side; `attachment_run_python` está DEPRECATED desde 2026-07-02, ver `text/tools/sql.yaml`) |
| "Cargá este CSV en mi DB" | `sql_bulk_insert_from_attachment` (COPY) |
| **"Leéme el PDF" / "Describime la imagen" / "Resumime este markdown"** | **`load_attachment`** |
| "Mostrame la fila 23 del CSV verbatim" | `load_attachment` |
| "Qué dice este código?" | `load_attachment` |

La matriz completa de elección está en
[`23_sql_node.md` §"Elegir la herramienta correcta para un attachment"](23_sql_node.md).

> ### ✅ Validado en dev (2026-05-28)
> Confirmado end-to-end contra el worker `colmena-worker-00047` (bucket
> `adp-reference-develop-startti-dev`, DB `adp_db_develop`):
> - **Plan A multipart:** el LLM construyó `{"body":{"file":"$attachment:<doc_id>"}}`,
>   el resolver streameó un PDF de 46 KB a httpbin.org como `multipart/form-data`
>   real, y `last_used_at` se tocó en el éxito (verificado que el touch ocurre
>   solo en runs exitosos, NO en runs que fallan).
> - **Plan B `load_attachment`:** la tool se auto-inyectó desde el catálogo y el
>   contenido se cargó de forma efímera (D6/D7 confirmados — la ausencia del
>   `tool_calls`/contenido en el resultado persistido = marcador efímero).
> - **Fix D10:** tras el redeploy, `load_attachment` ahora toca `last_used_at`
>   (verificado: la fila pasó de `NULL` → poblado).

## Plan A — Persistent bytes for all attachment sources (2026-05-25)

As of Plan A, every attachment registered in `conversation_attachments` has its
bytes persisted in `OutputStorageRepository`. This is true regardless of source:

- **Inline (base64 en `files[].data`):** los bytes se streamean al storage en el momento del registro.
- **Signed URL (`files[].url`):** los bytes se descargan y se streamean al storage.
- **Generated artifact** (`image_generation` / `image_edit` / `tts`): los bytes ya
  viven en storage; el artefacto se registra automáticamente en `conversation_attachments`
  con `origin = generated_by:<tool>` y `source = Path(storage_key)`.

Esto habilita el placeholder `$attachment:<document_id>` para nodos downstream
(inicialmente `http_request` multipart) sin importar de dónde vino el documento.

El catálogo que ve el LLM en su system message lista cada documento con su
`document_id` y una pista de uso:
- `load_attachment(document_id)` para leer el contenido dentro del loop.
- `"$attachment:<document_id>"` para reenviar los bytes (p. ej. a un endpoint multipart).

Resolución: un `AttachmentStreamResolver` (port en `domain`, impl en
`infrastructure`) hace `document_id → storage_key → StoredStream`. La impl
también soporta un fallback de backward-compat que trata al identificador
como `storage_key` directo para flujos previos a Plan A.

Background y decisiones:
- Spec: [`docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md`](../superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md)
- Plan: [`docs/superpowers/plans/2026-05-25-attachment-uniform-resolution-plan-a.md`](../superpowers/plans/2026-05-25-attachment-uniform-resolution-plan-a.md)

### `last_used_at` se toca en TODA resolución (D10, fix 2026-05-28)

La decisión D10 del spec exige que `last_used_at` se actualice en **ambos**
caminos de resolución de un attachment, para que el GC (Plan C) no borre docs
que siguen en uso activo:

1. **`AttachmentStreamResolver::resolve()`** — el camino Plan A
   (`$attachment:<document_id>` en `http_request` multipart). Toca
   `last_used_at` al resolver con éxito.
2. **`load_attachment`** — el camino dentro del loop LLM
   (`AttachmentResolverImpl::resolve()` en
   `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`).

> **Bug fixed (commit `e3322ec`, develop):** el camino `load_attachment` **no
> estaba** tocando `last_used_at` — solo lo hacía el camino Plan A. Se corrigió
> agregando una llamada best-effort a `touch_last_used` justo después de
> resolver la fila del attachment. Test agregado:
> `resolver_touches_last_used_at_on_successful_load`. El touch es best-effort:
> un fallo al actualizar `last_used_at` NO falla el `load_attachment` (el
> contenido igual se entrega). El touch ocurre solo en cargas exitosas, no en
> runs que fallan al resolver.

## Plan B — Comportamiento catalog-driven + contenido efímero (2026-05-25)

Plan B activa las dos optimizaciones de costo que Plan A dejó sentadas como
fundación.

### Sin autoinject en el primer turno

Cuando el usuario adjunta un documento vía `inputs.files[]`, el LLM **ya no**
recibe los bytes en el primer mensaje user. El bloque de catálogo que Plan A
(Task 11) prepende al system message le dice al modelo qué documentos están
disponibles, cada uno con su `document_id`. El modelo decide por turno:

- **Leer el contenido** — llama `load_attachment(document_id)`.
- **Reenviar el doc a un tool downstream** — usa `"$attachment:<document_id>"`
  en los argumentos del tool (p. ej. `http_request` multipart).
- **Ignorar el doc** — no se paga ningún token de input por él.

Trade-off: un round-trip adicional cuando el modelo necesita leer el doc, a
cambio de ahorro de costo cuando no lo necesita (no input tokens por docs no
leídos).

### Resultados de `load_attachment` efímeros

Cuando el modelo llama `load_attachment(document_id)`, el contenido del doc se
inyecta en el **stream in-memory de iteraciones ReAct** por el resto del turno
actual. El modelo razona sobre el contenido normalmente dentro del turno.

Pero el mensaje sintético que carga el contenido **NO se persiste en
`llm_node_history`**. En su lugar, se persiste un marcador corto:

```
user: [load_attachment("Q3_report") was invoked. Document content was available
       for this turn only. Call load_attachment again if you need to re-read it.]
```

Los turnos futuros ven el marcador, no el contenido. El modelo retiene los
análisis que produjo a partir del doc (los mensajes assistant quedan intactos),
pero deja de pagar tokens de input por el doc en cada turno subsiguiente.

Si el modelo necesita re-leer el doc, vuelve a llamar `load_attachment` — el
resolver re-streamea el contenido desde `OutputStorageRepository`.

## Patrón: reenviar sin leer (ahorro de tokens)

Caso de uso canónico: el usuario adjunta un documento **gigante** (p. ej. un PDF
de 80 MB) y quiere subirlo a un endpoint externo (un Knowledge Base, un bucket,
un servicio de ingesta) **sin que el LLM lea su contenido** — leerlo
consumiría todos los tokens del contexto, y no hace falta para reenviarlo.

Esto es exactamente lo que habilita la combinación Plan A + Plan B:

```
Documento gigante adjuntado (files[])
   │
   ├─ Plan B / D6: el LLM NO recibe el contenido. Solo ve el catálogo en el
   │   system message → ~30 tokens de metadata, NO los 80 MB.
   │
   └─ El LLM llama un tool http_request con body {"file": "$attachment:<document_id>"}
       │
       └─ AttachmentStreamResolver streamea los bytes DIRECTO
          desde OutputStorageRepository → endpoint destino.
          Los bytes NUNCA entran al contexto del LLM. Cero tokens de contenido.
          El multipart se streamea end-to-end (sin bufferear en RAM).
```

**Las dos vías son independientes y excluyentes por intención:**

| Vía | Tool | ¿Contenido al contexto? | Tokens de contenido |
|---|---|---|---|
| **Leer** | `load_attachment(document_id)` | Sí (efímero, este turno) | Sí (el doc se inyecta) |
| **Reenviar** | `"$attachment:<document_id>"` en args de `http_request` | No | **Cero** |

Para forzar el reenvío sin lectura, el `system_message` solo necesita la
**política** — la mecánica ya viene del prelude:

```jsonc
{
  "type": "llm_call",
  "config": {
    "provider": "google",
    "model": "gemini-2.5-flash",
    "api_key": "${GEMINI_API_KEY}",
    "system_message": "Cuando el usuario pida subir un documento adjunto a un endpoint (KB, bucket, etc.), reenvialo con la tool correspondiente usando el placeholder $attachment. NUNCA leas el contenido — los documentos pueden ser enormes y leerlos desperdicia recursos.",
    "tool_configurations": {
      "upload_to_kb": {
        "name": "upload_to_kb",
        "node_type": "http_request",
        "description": "Upload an attached document to the Knowledge Base. body MUST be { \"file\": \"$attachment:<document_id>\" } from the catalog. Streams the file directly — never read its content.",
        "node_schema": {
          "base_url": { "fixed": "https://kb.example.com" },
          "endpoint": { "fixed": "/documents" },
          "method":   { "fixed": "POST" },
          "headers":  { "fixed": { "Content-Type": "multipart/form-data" } },
          "body": {
            "type": "object",
            "required": true,
            "description": "Must be { \"file\": \"$attachment:<document_id>\" } from the catalog."
          }
        }
      }
    }
  }
}
```

> El `system_message` se inyecta **antes** del prelude, así que su instrucción
> ("nunca leas, solo reenviá") prevalece sobre la regla baseline del prelude
> ("call load_attachment if the user asks about a document"). El LLM reenvía.

**Detalles operativos:**

- El multipart de `http_request` streamea end-to-end sin bufferear el archivo en memoria — un doc de decenas de MB no infla la RAM del worker.
- El placeholder `$attachment:` se resuelve **solo en el nodo `http_request`** (no globalmente en el tool executor). Si necesitás reenviar desde otro tipo de nodo, ese nodo tiene que cablear el `AttachmentStreamResolver` igual que `http_request`.
- Validado en dev (2026-05-28): un PDF de 46 KB se reenvió a httpbin.org como `multipart/form-data` real sin que el LLM lo leyera; solo se gastaron ~50 tokens (metadata + tool call).

## Por qué existe

`LlmMessage.files` no se persiste en `llm_node_history`. Cuando una conversación con un documento adjunto retoma en un turno posterior, el archivo deja de estar en el contexto del modelo. Re-adjuntarlo en cada turno es caro en tokens.

`load_attachment` resuelve esto: el LLM ve un catálogo de documentos disponibles en la descripción del tool, y pide el que necesita cuando lo necesita.

## Cómo funciona

1. Adjuntás un archivo al primer `llm_call` mediante `files[]` como siempre.
2. El motor sube los bytes al `OutputStorageRepository` (Plan A) y registra metadata en `conversation_attachments`, scoped por `agent_session_id`. **El contenido NO se inyecta al contexto del LLM** (Plan B / D6) — el LLM solo ve el catálogo.
3. En cualquier turno — incluyendo `llm_call`s dentro de subgrafos — el LLM ve: (a) el catálogo de la sesión en el system message, y (b) la tool sintética `load_attachment` auto-registrada.
4. El LLM decide por turno qué hacer con cada documento:
   - **Leer** → `load_attachment(document_id)`: el contenido se inyecta **solo para el turno actual** (efímero, D7). No se persiste en `llm_node_history` — futuros turnos ven un marcador, no el contenido. Ver la sección [Resultados de load_attachment efímeros](#resultados-de-load_attachment-efímeros).
   - **Reenviar sin leer** → `"$attachment:<document_id>"` en los args de un tool downstream (p. ej. `http_request` multipart). Los bytes se streamean directo desde storage al endpoint, **sin pasar por el contexto del LLM** (cero tokens de contenido). Ver la sección [Patrón: reenviar sin leer](#patrón-reenviar-sin-leer-ahorro-de-tokens).
   - **Ignorar** → no se paga ningún token por el documento.

## Flag por nodo

`attachments_enabled` (default `true`) en `llm_call.config`:

- `true` — el nodo expone la tool y aporta sus `files[]` al catálogo.
- `false` — el nodo NO expone la tool (no la ve) pero igualmente registra cualquier `files[]` que reciba, para que otros nodos los lean.

Usá `false` cuando un agente especialista NO debería tener acceso a documentos cargados por otras partes de la sesión.

## Prompt auto-inyectado (la mecánica baseline)

Cuando hay al menos un attachment en el catálogo de la sesión Y `attachments_enabled` es `true`, el motor inyecta **automáticamente** tres cosas, sin que el graph author escriba nada:

1. **`ATTACHMENTS_SYSTEM_PRELUDE`** — un bloque de prosa en el system message que explica las DOS vías (leer + reenviar) y la semántica efímera.
2. **El catálogo** (`render_catalog`) — una línea por documento con `document_id`, label, mime, size, **y la pista de uso por doc**.
3. **La tool sintética `load_attachment`** — auto-registrada (NO requiere `enabled_tools`), con el catálogo repetido en su descripción y los `document_id` válidos como enum.

### Orden de ensamblado del system message

Desde el cambio "cache-safe temporal context" (commit `e8191dd1`), el bloque de
contexto temporal/geográfico **ya no va primero**: se construye con
`format_temporal_context_block` y se adjunta vía
`llm_config.with_volatile_system_suffix(context_block)`
(`llm.rs` ~línea 3054), que cada adapter de provider agrega al **final** del
system message (`openai_adapter.rs:44-58`, `gemini_adapter.rs:276`/`1305`,
`anthropic_adapter.rs:207-213`). El motor arma el resto en este orden
(`llm.rs::execute`, comentario en línea 3059: *"First stable section now that
the temporal block moved to the volatile suffix"*):

```
[TU system_message]                          ← el rol/persona/política del agente
[ATTACHMENTS_SYSTEM_PRELUDE]                 ← auto, solo si hay adjuntos
[catálogo de documentos]                     ← auto, una línea por doc
[lista de tools disponibles]                 ← auto, si hay tools
[bloque de contexto temporal/geográfico]     ← siempre último (volatile suffix, no afecta el prompt cache)
```

El prelude se inyecta **después** de tu `system_message`. El modelo lee primero su persona/política, luego la mecánica de attachments, y el timestamp fresco llega al final para no invalidar el prompt cache.

### Texto exacto del prelude

Esto es lo que el modelo ve textualmente (constante `ATTACHMENTS_SYSTEM_PRELUDE` en `llm_synthetic_tools/load_attachment_tool.rs`):

```
## Conversation Attachments
This conversation has one or more documents attached to it. They are listed in
the catalog below (and in the description of the `load_attachment` tool), each
with a `document_id`, label, mime type, and size.

You will NOT see document content automatically — the catalog only advertises
which documents exist. To read a document's content, you must call
load_attachment(document_id). To forward a document to a downstream tool (for
example `http_request` multipart) without reading it yourself, pass the string
"$attachment:<document_id>" in that tool's args.

load_attachment results are ephemeral: the document content is available only
for the turn in which you invoked the tool. Future turns will see a marker
confirming the call happened, but not the content itself. Call load_attachment
again if you need to re-read the document.

Rules:
- If the user asks about any uploaded document, call `load_attachment` with the
  matching `document_id` before answering — never guess at the contents.
- Do not list, paraphrase, or summarise the attachments unless the user asks.
- One `document_id` per call. Call the tool again if you need a second document.
- If the user's question does not depend on any attachment, answer normally —
  do NOT call `load_attachment` preemptively.
```

El catálogo que sigue al prelude renderiza, por documento:

```
- "<document_id>" — <filename> (<mime>, <size>). <description>
  usage: load_attachment("<document_id>") to read · "$attachment:<document_id>" to forward
```

### Separación: mecánica auto (baseline) vs política del graph author (extra)

```
┌──────────────────────────────────────────────────────────────────┐
│ AUTO-INYECTADO por el motor — la MECÁNICA (el "sí o sí")          │
│  · cómo leer:     load_attachment(document_id)                    │
│  · cómo reenviar: "$attachment:<document_id>" en args de tools    │
│  · semántica efímera + reglas baseline                            │
│  · el catálogo con los document_id reales de la sesión            │
├──────────────────────────────────────────────────────────────────┤
│ TU system_message — la POLÍTICA (el comportamiento "extra")       │
│  · CUÁNDO leer vs reenviar (decisión de negocio)                  │
│  · a qué endpoint reenviar, con qué tool                          │
│  · persona, tono, flujo del caso de uso                           │
└──────────────────────────────────────────────────────────────────┘
```

**El graph author NO necesita repetir la mecánica en su `system_message`.** No hace falta explicar `load_attachment`, ni `$attachment:`, ni dónde están los `document_id` — todo eso ya está en el prelude + catálogo. El `system_message` se reserva para la **política**: cuándo el agente debe leer, cuándo reenviar, a dónde, y el rol del agente.

Como el `system_message` se inyecta **antes** del prelude, una instrucción explícita del graph author (p. ej. "para subir al KB, reenviá; NUNCA leas") **prevalece** sobre la regla genérica del prelude ("call load_attachment if the user asks about a document"). Así la capa de política sobreescribe la baseline cuando hay conflicto.

## Observabilidad SSE de los tools

Todos los tools que ejecuta el nodo `llm_call` emiten eventos SSE de tool para
que el frontend pueda renderizar su ciclo de vida. Los eventos viajan en dos
momentos:

- **Input** (`tool-input-start`, `tool-input-delta`, `tool-input-available`) —
  se emiten **antes** de ejecutar, mientras el LLM streamea el tool call. Son
  uniformes para TODOS los tools.
- **Output** (`tool-output-available`) — se emite **después** de ejecutar.

| Tool | input-* | output-available | Evento extra |
|---|:---:|:---:|---|
| Node-backed (`http_request`, `sql_query`, `python_script`, `image_generation`, `image_edit`, `tts`, `socketio_request`, …) | ✅ | ✅ | — |
| Toolkit sub-tools (`api_explorer__*`) | ✅ | ✅ | — |
| `load_skill` | ✅ | ✅ | `skill-loaded` |
| `describe_tool` (lazy loading) | ✅ | ✅ | `tool-described` |
| `document_*` (create, read, apply_patch, get_head, list_versions, rollback, list_my_artifacts) | ✅ | ✅ | — |
| `load_attachment` | ✅ | ✅ (desde fix `c897bcf`) | — |
| `suspend` / `secure_suspend` | ✅ | ❌ por diseño | → `finish` con `__colmena_status: SUSPENDED` |

> **`load_attachment` (fix `c897bcf`):** su payload de `tool-output-available`
> contiene **solo metadata** (`{ "document_id": "...", "status": "loaded" | "error" }`),
> NO el contenido del documento — el contenido sigue siendo efímero en el
> contexto del LLM (D7) y no viaja por el SSE. Antes del fix, `load_attachment`
> emitía los eventos de input pero no el de output (el bloque del sentinel hacía
> `continue` antes de la emisión), dejando al frontend con un input sin output.

> **`suspend` / `secure_suspend`:** no emiten `tool-output-available` a
> propósito — no son tools "completados" sino una pausa del loop. Se surface
> vía el evento `finish` con `__colmena_status: SUSPENDED` (el banner de
> suspend en la UI).

## Campos opcionales en `files[]`

```jsonc
{
  "files": [
    {
      "id": "q3-report",                       // opcional, auto-id (att_<hex16>) si falta
      "mime_type": "application/pdf",
      "filename": "Q3_Financial.pdf",
      "label": "Reporte Financiero Q3",        // opcional, fallback = filename
      "description": "Ingresos y gastos Q3",   // opcional
      "url": "https://storage.googleapis.com/...?X-Goog-Signature=..."
    }
  ]
}
```

## Subgrafos

`agent_session_id` se propaga automáticamente al subgrafo. Eso significa que un `llm_call` dentro de un subgrafo ve el mismo catálogo que el padre, sin código adicional. Si querés aislamiento estricto, usá `attachments_enabled: false` en el `llm_call` del subgrafo.

## Recuperación por expiración

Gemini caduca los `file_id` a las 48h. Cuando una fila en `conversation_attachments` tiene `source_kind = 'signed_url' | 'path'` y `refreshed_at` ≥ 24h, el resolver re-sube el archivo silenciosamente al provider y actualiza el row. Si el archivo se subió como `InlineBytes` (bytes embebidos en el JSON), NO retenemos los bytes — el load fallará con `attachment_expired_unrecoverable` y el LLM puede pedir al usuario que re-suba.

## Errores que el LLM puede ver como resultado de la tool

```json
{ "error": "unknown_document_id", "document_id": "...", "hint": "..." }
{ "error": "attachment_expired_unrecoverable", "document_id": "...", "reason": "..." }
```

Ambos se devuelven como `ToolResult` ordinario para que el modelo pueda recuperarse (pedir al usuario, intentar otro id, etc.).

## Tabla `conversation_attachments`

```sql
agent_session_id, document_id, provider, provider_file_id,
mime_type, filename, size_bytes,
label, description,
source_kind, source_value,
registered_at, refreshed_at
PRIMARY KEY (agent_session_id, document_id, provider)
```

`source_kind` controla la estrategia de recuperación:
- `signed_url` / `path` → recuperable (re-upload silencioso al pasar 24h)
- `inline` → no recuperable (bytes no retenidos)

## Test graphs

- `tests/graphs/agents/load_attachment_basic.json` — single `llm_call` con un archivo + pregunta posterior
- `tests/graphs/agents/load_attachment_subgraph.json` — parent registra + child subgrafo lee
- `tests/graphs/agents/load_attachment_opt_out.json` — verifica que `attachments_enabled: false` oculta la tool
- `tests/graphs/agents/load_attachment_auto_summary.json` — auto-summary con Gemini Flash + Postgres (no se pasa `description` para forzar la generación)

Todos usan `google` / `gemini-2.5-flash` con `${DATABASE_URL}` para memoria Postgres. Los URLs `$REPLACE_WITH_SIGNED_URL` son placeholders — sustituí por una signed URL real (GCS) antes de correr.

---

## Auto-generated descriptions (auto-summary)

> **Estado:** Disponible desde 0.4.0
> **Spec:** [docs/superpowers/specs/2026-05-14-attachment-auto-summary-design.md](../superpowers/specs/2026-05-14-attachment-auto-summary-design.md)

Cuando una entrada de `files[]` **no** trae `description`, el nodo `llm_call` genera automáticamente un resumen de una línea, lo persiste en `conversation_attachments.description`, y lo expone en el catálogo del tool `load_attachment` a partir del siguiente turno. El graph author no necesita escribir metadata; la descripción aparece sola.

### Por qué existe

Sin auto-summary, el catálogo de `load_attachment` mostraba solo `filename + mime + size`. Para uploads con nombres genéricos (`document.pdf`, `Screenshot 2026-05-14.png`, `untitled.docx`) eso no le permitía al LLM elegir el archivo correcto entre varios candidatos. El auto-summary llena ese vacío sin que el integrador (ADP, etc.) tenga que generar metadata desde su propio backend.

### Cuándo se dispara

El path de generación corre durante el turno en que el archivo se **registra por primera vez** en `conversation_attachments`. Específicamente, todas estas condiciones tienen que cumplirse:

| Condición | Por qué |
|---|---|
| `summary_enabled: true` en `config` | Master switch (default `true`) |
| La entrada de `files[]` NO trae `description` | Si el caller pasó descripción, se respeta sin overhead |
| El archivo NO existe ya en `conversation_attachments` para `(agent_session_id, document_id, provider)` | Solo se genera en la primera registración; turnos subsiguientes leen la fila existente |
| `AttachmentSource` válido (`SignedUrl`, `Path` o `Inline` con bytes retenidos) | El summary necesita una fuente de bytes — todas funcionan post-fix 2026-05-18 |
| `agent_session_id` está disponible y hay registry conectado | El summary necesita persistirse |

Cuando cualquiera falla, el campo `description` queda `null` y el catálogo cae a `filename` como label.

### Pipeline

```
files[i] sin description
   ↓
acquire_bytes(source, fetcher, inline_bytes)
   ├─ AttachmentSource::SignedUrl → fetcher.stream(url) → Vec<u8>
   ├─ AttachmentSource::Path      → tokio::fs::read(path)
   └─ AttachmentSource::Inline    → SummaryTarget::inline_bytes (clonados en resolve_one antes del upload)
   ↓
extract_text(mime, bytes) → Option<String>
   ├─ application/pdf                                           → pdf-extract::extract_text_from_mem
   ├─ text/plain | text/markdown | text/csv | text/html | ...   → str::from_utf8
   ├─ image/*                                                   → sin extracción (se manda imagen entera)
   └─ otros                                                     → Ok(None) → SKIP
   ↓
truncate_chars(text, summary_max_chars=5000)
   ↓
LlmAttachmentSummaryGenerator::generate
   ├─ SummarySource::ExtractedText → prompt textual (1 línea, max_output_chars)
   └─ SummarySource::ImageBytes    → prompt con FileData::inline + vision
   ↓
tokio::time::timeout(summary_timeout_secs, repo.call(request))
   ↓
post-process: trim, strip surrounding quotes, collapse \n, char-truncate
   ↓
AttachmentRegistry::update_description(agent_session_id, document_id, provider, summary)
```

Todo el bloque corre **dentro de un `tokio::task::JoinSet`**, en paralelo con `agent_service.run(params)` vía `tokio::join!`. La latencia user-facing es `max(answer_call, summary_batch)`. Con Gemini Flash + 5000 chars el summary suele tardar 1–2 s — más rápido que el answer call típico — así que la penalidad efectiva es ~0 ms.

### Per-MIME strategy

| MIME | Estrategia | Notas |
|---|---|---|
| `application/pdf` | `pdf-extract::extract_text_from_mem` → texto → truncate → prompt textual | PDFs solo-imagen (sin text layer) → `Ok(None)` → fallback a filename. PDFs corruptos → `ExtractError::PdfParse` → fallback a filename. |
| `text/plain`, `text/markdown`, `text/x-markdown`, `text/csv`, `text/html` | `str::from_utf8` → texto → truncate → prompt textual | Parámetros del MIME (`charset=utf-8`, etc.) se strippean antes del match. Bytes no-UTF8 → `ExtractError::InvalidUtf8` → fallback. |
| `image/png`, `image/jpeg`, `image/webp`, `image/gif` | Bytes enteros → `FileData::inline` → prompt visión | ~258 tokens/imagen. Cualquier `image/*` cae acá. Sin extracción local. |
| Cualquier otro (`application/zip`, `application/vnd.openxmlformats-...`, etc.) | `extract_text` retorna `Ok(None)` → SKIP | Office (docx/xlsx/pptx) son out-of-scope para v1; agregar extractores específicos si la necesidad surge. |

### Configuración

Cinco campos opcionales en `llm_call.config`:

| Campo | Tipo | Default | Qué hace |
|---|---|---|---|
| `summary_enabled` | `bool` | `true` | Master switch. `false` desactiva el path completo. |
| `summary_max_chars` | `int` | `5000` | Caracteres de texto extraído enviados al LLM (~2 páginas de prosa). Ignorado para imágenes. |
| `summary_model` | `string` | `null` → cheap-tier del provider | Override del modelo: Google→`gemini-2.5-flash`, OpenAI→`gpt-4o-mini`, Anthropic→`claude-haiku-4-5-20251001`. Mismo provider que el main call siempre. |
| `summary_timeout_secs` | `int` | `15` | Per-call timeout (segundos). También se reusa como batch-level ceiling. |
| `summary_max_output_chars` | `int` | `200` | Cap del summary producido. Char-truncado post-hoc. |

Ejemplo mínimo:

```json
{
  "type": "llm_call",
  "config": {
    "provider": "google",
    "model": "gemini-2.5-flash",
    "api_key": "${GEMINI_API_KEY}",
    "connection_url": "${DATABASE_URL}",
    "files": [
      {
        "id": "demo_doc",
        "url": "https://storage.googleapis.com/...",
        "mime_type": "application/pdf",
        "filename": "Q3_Financial.pdf"
      }
    ],
    "prompt": "Resumeme el documento adjunto."
  }
}
```

Como `summary_enabled` defaultea a `true` y el `files[0]` no trae `description`, en el primer turno el motor:

1. Sube el PDF al provider (existing pipeline).
2. Registra la fila en `conversation_attachments` con `description = null`.
3. En paralelo con el answer call: descarga otra vez el PDF, extrae texto, manda primeras 5000 chars a Gemini Flash con un prompt cataloger.
4. Cuando el summary vuelve (típico < 2 s), persiste `description = "Q3 2026 financial report — revenue, expenses, EBITDA"`.

Turno 2 (mismo `agent_session_id`, sin `files[]` esta vez):

```
load_attachment tool description:
  Available attachments:
  - "demo_doc" — Q3_Financial.pdf (application/pdf, 1.2 MB). Q3 2026 financial report — revenue, expenses, EBITDA
```

### Modelo usado por defecto

El mapping `provider_cheap_tier` en `src/libs/colmena/src/llm/infrastructure/attachment_summary/cheap_tier.rs` define el modelo barato por provider:

| Provider | Cheap-tier default |
|---|---|
| Google | `gemini-2.5-flash` |
| OpenAI | `gpt-4o-mini` |
| Anthropic | `claude-haiku-4-5-20251001` |
| Mock | `mock-model` |

Si querés calidad mayor para el summary (por ejemplo cataloger más preciso para un dominio técnico), seteá `summary_model: "gemini-2.5-pro"` (u otro modelo del MISMO provider — la `api_key` se reusa).

### Concurrencia

```
execute() inicio
   ↓
parse config (incluye 5 fields de summary)
   ↓
resolve_files (upload pipeline existente)
   ↓
auto-register + collect summary_targets
   ↓
build summary_fut (no se await todavía)
   ↓
tokio::join!(
    agent_service.run(params),               ← main answer call
    tokio::time::timeout(N s, summary_fut),  ← batch summary
)
   ↓
si summary_outcome es Err → log warn "summary.batch_timeout"
si agent_run_result es Err → propaga error normal
   ↓
los summaries que sí completaron se persistieron mid-await
```

Internamente, `summary_fut` usa un `tokio::task::JoinSet`: cada attachment se procesa en su propia task concurrente con las otras. **Dropping el `JoinSet` aborta todas las tasks**, lo que garantiza que si el caller cancela el `execute` (CTRL+C, DAG abort) ningún summary task quede zombi escribiendo a la base después de que el nodo retornó.

Cada task individual aplica `tokio::time::timeout(summary_timeout_secs, repo.call(...))` adentro de `LlmAttachmentSummaryGenerator`. Si una sola attachment tarda más, se cancela esa sola, las otras siguen.

### Failure modes (matriz)

| Escenario | Comportamiento | Estado persistido |
|---|---|---|
| `summary_enabled: false` | Skip total | `description = caller-supplied or null` |
| Caller pasó `description` no vacío | Skip generación, usa el valor pasado | `description = caller value` |
| `AttachmentSource::Inline` (data: base64) | Summary corre con bytes retenidos vía `retained_inline_bytes` (clon en `resolve_one`) | `description = summary` |
| MIME no soportado (zip, docx, etc.) | Skip extracción, no LLM call | `description = null` |
| PDF solo-imagen (`pdf-extract` retorna empty) | Skip LLM call | `description = null` |
| PDF corrupto (`pdf-extract` retorna Err) | Skip LLM call, log warn | `description = null` |
| Bytes no se pueden adquirir (download/read error) | Skip LLM call, log warn | `description = null` |
| LLM call falla (network, 5xx, parse) | Log warn | `description = null` |
| LLM call retorna whitespace-only | `SummaryError::EmptyResponse`, log warn | `description = null` |
| LLM call excede `summary_timeout_secs` | Cancel via `tokio::time::timeout` | `description = null` |
| Batch entero excede timeout (raro) | Cancel todas las tasks via JoinSet drop | All `description = null` |
| Answer call falla, summary OK | Persiste summary igual (útil para turno 2) | `description = summary` |
| Provider file expirado (recovery path) | Re-upload silencioso; summary NO se regenera | `description` sin cambios |

**Regla general:** ningún failure del summary jamás propaga al user-facing result. El answer call es el contrato; el summary es nice-to-have.

### Cost (orden de magnitud)

Para un PDF típico de ~10 páginas en Gemini Flash con defaults (5000 chars extraídos):

- **Input:** ~1250 tokens (5000 chars / 4) + ~50 tokens del system prompt cataloger
- **Output:** ~50 tokens (1 línea de ~200 chars)
- **Costo:** ~$0.0003 por summary (Flash input $0.075/1M, output $0.30/1M)
- **A escala:** 10 000 archivos summarizados = ~$3

Comparado con mandar el doc entero (PDF de 200 pp ≈ 120 000 tokens visión + texto ≈ $0.009): ~**30× más barato** que summarizar sin truncar.

### Limitaciones conocidas (v1)

> **Resuelto (2026-05-18):** La limitación previa "archivos `data:` (base64 inline) no se summarizan" se cerró. `LlmCallUseCase::resolve_one` ahora clona los bytes inline a un nuevo campo `FileData::retained_inline_bytes` antes de que el upload streaming los consuma, y el auto-register loop los pasa a `acquire_bytes` vía `SummaryTarget::inline_bytes` para la fuente `Inline`. Verificado e2e contra Gemini Flash: `source_kind = inline` ahora produce filas con `description` no-null.

1. **Office formats no soportados.** `docx`, `xlsx`, `pptx`, `odt`, etc. caen a `Ok(None)` en `extract_text` y skipean el summary. **Workaround:** pasá `description` manualmente. **Plan v2:** agregar extractores específicos (`docx-rs`, `calamine`).

2. **PDFs encriptados o solo-imagen.** `pdf-extract` retorna error o texto vacío. **Workaround:** pre-procesar con OCR fuera del engine y pasar `description` manualmente.

3. **No hay retry automático.** Si el LLM falla en la primera generación, la fila queda con `description = null` para siempre (en la sesión). **Plan v2:** flag `force_resummary` o background worker.

4. **No hay cross-session deduplication.** Si el mismo archivo se sube a dos `agent_session_id` distintos, se summariza dos veces. **Plan v2:** cache por content hash.

### Test graph para auto-summary

`tests/graphs/agents/load_attachment_auto_summary.json` — un `llm_call` único con Gemini Flash, Postgres como registry, y un `files[]` SIN `description` para forzar el path. Reemplazá `$REPLACE_WITH_SIGNED_URL` por una signed URL real de GCS:

```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/agents/load_attachment_auto_summary.json \
  --agent-session-id agent_auto_summary_001
```

Inspeccionar Postgres después de la corrida:

```bash
psql "$DATABASE_URL" -c \
  "SELECT document_id, description FROM conversation_attachments \
   WHERE agent_session_id = 'agent_auto_summary_001';"
```

Esperado: `description` contiene una línea no vacía describiendo el doc.

### Cómo opt-out

Si tu integrador ya construye descripciones desde su propio backend (CRM metadata, etc.), pasalas en el JSON y el auto-summary se skipea automáticamente:

```json
"files": [{
  "id": "contract_abc",
  "url": "...",
  "mime_type": "application/pdf",
  "filename": "contract.pdf",
  "label": "MSA — Acme Corp",
  "description": "MSA Acme Corp — vigencia 2026-09-01 a 2027-08-31, ARR USD 480k"
}]
```

O desactivá el feature por nodo con `summary_enabled: false` (la metadata caller-supplied sigue respetándose, pero los archivos sin description quedarán con `null`).
