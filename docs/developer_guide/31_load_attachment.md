# Load Attachment — Documentos on-demand dentro del loop LLM

> **Estado:** Disponible desde 0.4.0
> **Spec:** [docs/superpowers/specs/2026-05-13-load-attachment-design.md](../superpowers/specs/2026-05-13-load-attachment-design.md)

## Por qué existe

`LlmMessage.files` no se persiste en `llm_node_history`. Cuando una conversación con un documento adjunto retoma en un turno posterior, el archivo deja de estar en el contexto del modelo. Re-adjuntarlo en cada turno es caro en tokens.

`load_attachment` resuelve esto: el LLM ve un catálogo de documentos disponibles en la descripción del tool, y pide el que necesita cuando lo necesita.

## Cómo funciona

1. Adjuntás un archivo al primer `llm_call` mediante `files[]` como siempre.
2. El motor lo sube al provider y registra metadata (no bytes) en `conversation_attachments`, scoped por `agent_session_id`.
3. En cualquier turno siguiente — incluyendo `llm_call`s dentro de subgrafos — el LLM ve la tool sintética `load_attachment` con el catálogo de la sesión en su descripción.
4. Cuando el LLM llama `load_attachment(document_id)`, el motor inyecta un mensaje `user` sintético con el archivo y persiste ese mensaje en la historia. Próximo turno: el archivo ya está en contexto.

## Flag por nodo

`attachments_enabled` (default `true`) en `llm_call.config`:

- `true` — el nodo expone la tool y aporta sus `files[]` al catálogo.
- `false` — el nodo NO expone la tool (no la ve) pero igualmente registra cualquier `files[]` que reciba, para que otros nodos los lean.

Usá `false` cuando un agente especialista NO debería tener acceso a documentos cargados por otras partes de la sesión.

## Prompt auto-inyectado

Cuando hay al menos un attachment en el catálogo de la sesión Y `attachments_enabled` es `true`, el motor inyecta automáticamente `ATTACHMENTS_SYSTEM_PRELUDE` al final del `system_message`. El prelude le explica al modelo:

- Que hay documentos disponibles (listados en la descripción del tool).
- Que debe llamar `load_attachment` antes de responder cuando el usuario referencia un documento.
- Que no debe listar/parafrasear los attachments salvo que se le pida.
- Que la tool acepta un solo `document_id` por llamada.
- Que NO debe llamar `load_attachment` preemptivamente cuando la pregunta no depende de un attachment.

**El graph author NO necesita repetir estas instrucciones en su propio `system_message`.** El `system_message` del nodo se reserva para la persona/rol del agente.

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
| El `AttachmentSource` resultante NO es `Inline` | v1 limitation — ver [Limitaciones conocidas](#limitaciones-conocidas-v1) |
| `agent_session_id` está disponible y hay registry conectado | El summary necesita persistirse |

Cuando cualquiera falla, el campo `description` queda `null` y el catálogo cae a `filename` como label.

### Pipeline

```
files[i] sin description
   ↓
acquire_bytes(source, fetcher)
   ├─ AttachmentSource::SignedUrl → fetcher.stream(url) → Vec<u8>
   ├─ AttachmentSource::Path      → tokio::fs::read(path)
   └─ AttachmentSource::Inline    → SKIP (v1 no retiene bytes inline post-upload)
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
| `AttachmentSource::Inline` (no url/path) | Skip (v1 limitation) | `description = null` |
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

1. **Archivos `data:` (base64 inline) no se summarizan.** Cuando el integrator sube un archivo embebido en el JSON (campo `data`) sin un `url` o `path` que lo respalde, los bytes se consumen durante el upload streaming al provider y no se retienen para una segunda lectura. En esos casos `AttachmentSource::Inline` se guarda en el registry pero el path de summary salta esa fila. Los archivos con `path:` SÍ se summarizan correctamente — el summary path re-lee del disco via `AttachmentSource::Path`. **Workaround para `data:`:** pasá `description` manualmente en el `files[]` entry. **Plan v2:** tee el stream de upload para retener bytes sin doble-descarga.

2. **Office formats no soportados.** `docx`, `xlsx`, `pptx`, `odt`, etc. caen a `Ok(None)` en `extract_text` y skipean el summary. **Workaround:** pasá `description` manualmente. **Plan v2:** agregar extractores específicos (`docx-rs`, `calamine`).

3. **PDFs encriptados o solo-imagen.** `pdf-extract` retorna error o texto vacío. **Workaround:** pre-procesar con OCR fuera del engine y pasar `description` manualmente.

4. **No hay retry automático.** Si el LLM falla en la primera generación, la fila queda con `description = null` para siempre (en la sesión). **Plan v2:** flag `force_resummary` o background worker.

5. **No hay cross-session deduplication.** Si el mismo archivo se sube a dos `agent_session_id` distintos, se summariza dos veces. **Plan v2:** cache por content hash.

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
