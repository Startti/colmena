# Multimedia Generation — image_generation, image_edit, tts

> **Updated:** 2026-05-28 · **Status:** Shipped end-to-end (in-colmena) y **validado en dev**. Host-side wiring (Phase 7) tracked separately by the consuming application.

> ### ✅ Validado en dev (2026-05-28)
> Confirmado end-to-end contra el worker `colmena-worker-00047` (bucket
> `adp-reference-develop-startti-dev`, DB `adp_db_develop`):
> - **`image_generation`** con Vertex Imagen 4 (`imagen-4.0-generate-001`) →
>   blob en GCS + fila en `conversation_attachments` (origin
>   `generated_by:image_generation`, `storage_key` poblado).
> - **`tts`** con Gemini TTS (`gemini-2.5-flash-preview-tts`, voz `Kore`,
>   formato `wav`) → blob WAV + fila (origin `generated_by:tts`).
> - **Agente multimedia** (LLM Gemini con tools `generate_image` + `speak_text`)
>   encadenando 2 tool calls en un mismo turno.
>
> **NO** validado / roto: el chaining LLM-driven `image_generation` →
> `image_edit` (ver "Limitación conocida" más abajo).

Tres nodos para generar media + el sistema completo de "artifacts" (storage, registry, placeholders, scrubber) que permite encadenar generaciones, "ver" lo generado desde el LLM, y enviarlo a endpoints externos — todo **sin que el LLM vea bytes binarios nunca**.

## Donde viven los artifacts — bucket-first, dev convenience

> ⚠️ **Importante leer antes de cualquier setup.** Los artifacts (imágenes,
> audios) **viven en un bucket de Google Cloud Storage** tanto en producción
> como en dev "real". El modo `LocalHttpStorageAdapter` (`COLMENA_LOCAL=true`
> que vas a ver más abajo) **NO es el modo "normal"** — es una conveniencia
> de desarrollo local para iterar sin tener que setear credenciales GCS ni
> levantar el callback a la host application en tu laptop.

### Matriz canónica de modos

| Entorno | Adapter | Dónde viven los bytes | URL que recibe el LLM | Cuándo usarlo |
|---|---|---|---|---|
| **Producción** (Cloud Run worker) | `HttpCallbackStorageAdapter` | Bucket GCS real (compartido con uploads del usuario) | Signed GCS URL firmado por host application (~1h TTL) | Siempre en deploys reales |
| **Dev "real"** (apuntando a tu staging) | `HttpCallbackStorageAdapter` | Bucket GCS de staging | Signed GCS URL de staging | Cuando querés validar el flow end-to-end de prod sin desplegar |
| **Dev local (conveniencia)** | `LocalHttpStorageAdapter` | Disco local `/tmp/colmena-out/` + axum server en `127.0.0.1` | `http://127.0.0.1:8765/files/<key>` | Iteración rápida sin GCS ni host application levantada |
| **CI / tests unitarios** | `LocalCacheStorageAdapter` | RAM del proceso (se pierde al cerrar) | Handle opaco `local://<uuid>` | Solo tests automatizados — never user-facing |

**Mental model**: en cualquier entorno donde hay agentes corriendo para usuarios reales (staging, prod, dev contra staging), los artifacts **están en GCS**. El `LocalHttp` es una "simulación local" del bucket para que un dev pueda probar la pipeline sin que su laptop necesite credenciales GCS ni callback URL a la host application.

### El flujo prod en una imagen

```
   ┌────────────────────┐                    ┌──────────────────────┐
   │  colmena worker    │                    │  host application    │
   │  (Cloud Run, Rust) │                    │  (external service,  │
   │                    │                    │   not part of this   │
   │                    │                    │   repo)              │
   │                    │                    │                      │
   │  image_generation  │                    │  /internal/gcs/      │
   │  node generates    │                    │  sign-put            │
   │  bytes via OpenAI  │                    │  (InternalService    │
   │                    │                    │   Guard)             │
   └──────────┬─────────┘                    └──────┬───────────────┘
              │                                      │
              │ 1. POST {session_id, mime, filename} │ 2. lookup AgentSession
              │    + X-Internal-Token header         │    derive userId
              ├─────────────────────────────────────►│    build storage_key
              │                                      │    generate signed PUT URL
              │ 3. { put_url, read_url, storage_key}│
              │◄─────────────────────────────────────┤
              │                                      │
              │ 4. PUT bytes → signed put_url ─────────────────────┐
              │                                      │             ▼
              │                                      │      ┌──────────────┐
              │                                      │      │  GCS bucket  │
              │                                      │      │  (real)      │
              │                                      │      └──────────────┘
              │                                      │
              │ 5. auto-registra fila en             │
              │    conversation_attachments:         │
              │      { document_id, storage_key,     │
              │        origin: "generated_by:…",     │
              │        mime_type, size_bytes,        │
              │        source: Path(storage_key) }   │
              │                                      │
              │ 6. emits tool output {                │
              │      document_id,                    │
              │      mime_type, size_bytes,          │
              │      description?                    │
              │    }                                  │
              ├─────────────────────────────────────►│
              │                                      │
              │                                      │  7. host app (si necesita)
              │                                      │     resuelve document_id →
              │                                      │     join conversation_attachments
              │                                      │     → recupera storage_key
              │                                      │     → firma URL para frontend
```

Cero credenciales GCS en el worker. host application es la única con creds y la única que firma URLs.

### El flujo dev local (mismo modelo, sin bucket)

```
   ┌────────────────────┐
   │  colmena worker    │  (cargo run)
   │  LocalHttpAdapter  │
   │  + embedded axum   │  → write to /tmp/colmena-out/<uuid>.png
   │  on 127.0.0.1:8765 │  → return http://127.0.0.1:8765/files/<uuid>.png
   └────────────────────┘
              │
              ▼
       /tmp/colmena-out/<uuid>.png   ← inspeccionable con `open`
```

Sin red externa, sin GCS, sin host application. **Mismo shape de URL en el output** (HTTP fetchable) — por eso el flow del agente es idéntico bytes-wise: `load_attachment`, `$attachment:<key>`, vision input — todos funcionan sin código condicional.

### Setup de cada modo

```bash
# Producción (worker en Cloud Run)
COLMENA_LOCAL=false
COLMENA_STORAGE_CALLBACK_URL=https://your-host-api.example.com/internal/gcs/sign-put
COLMENA_STORAGE_CALLBACK_SECRET=<shared con COLMENA_INTERNAL_TOKEN en host application>

# Dev contra la host application real (cuando querés validar el flujo prod)
COLMENA_LOCAL=false
COLMENA_STORAGE_CALLBACK_URL=https://your-host-api-staging.example.com/internal/gcs/sign-put
COLMENA_STORAGE_CALLBACK_SECRET=<staging secret>

# Dev local (la mayor parte del tiempo durante iteración)
COLMENA_LOCAL=true
# COLMENA_LOCAL_STORAGE_DIR=/tmp/colmena-out   (default — opcional)
# COLMENA_LOCAL_STORAGE_PORT=8765              (default — opcional)

# CI / tests
# (no setear nada — cae a LocalCacheStorageAdapter)
```

Si seteás `COLMENA_LOCAL=false` y olvidás el callback URL o secret, el engine **panica al startup** con mensaje claro. Esto previene el footgun "olvidé sacar `COLMENA_LOCAL=true` de mi env y mi staging cae a in-memory silencioso".

---

## TL;DR

| Quiero | Cómo |
|---|---|
| Generar una imagen desde un prompt | Nodo `image_generation` (OpenAI gpt-image-1 o Google Vertex Imagen 4). |
| Editar una imagen ya generada | Nodo `image_edit` (OpenAI gpt-image-1 multipart). Acepta `source_url` en formato `data:`, `http(s)://` o `local://<key>`. |
| Hacer text-to-speech | Nodo `tts` (OpenAI, ElevenLabs, o Google Gemini TTS). |
| Que el LLM "vea" lo que generó | Llamar `load_attachment(document_id=<document_id>)` desde el agente — el resolver hace upload cross-provider lazy a la Files API del provider activo y lo inyecta como vision input en el siguiente turn. |
| Mandar la imagen generada a un webhook | `http_request` con body que contiene `"$attachment:<document_id>"` — el engine resuelve el placeholder a `data:` URI antes del POST. |
| Inspeccionar artifacts en dev | `COLMENA_LOCAL=true` activa el adapter de disco + server HTTP local. Files en `/tmp/colmena-out/`, URLs `http://127.0.0.1:8765/files/<key>`. |

## Architectural invariant

**El contexto del LLM nunca contiene bytes binarios.** Tres mecanismos lo enforcen, en cada dirección del flujo:

```
                    ┌──────────────────────────┐
                    │      LLM context         │  ← never raw bytes
                    │  (text + URL handles)    │
                    └──┬─────────────────────▲─┘
   (URL or short handle│                     │ scrubbed tool result
    in tool output)    │                     │ (data:base64 → marker,
                       │                     │  strings >50KB → truncated)
                       ▼                     │
          ┌─────────────────────┐    ┌───────┴─────────────┐
          │ image_generation /  │    │   DagToolExecutor   │
          │ image_edit / tts    │    │   (scrubber)        │
          │ — outputs handle    │    │                     │
          └─────────┬───────────┘    └─────────▲───────────┘
                    │ store bytes              │ external endpoint
                    ▼                          │ response (echo, etc)
          ┌─────────────────────┐    ┌─────────┴───────────┐
          │ OutputStorageRepo   │    │   http_request      │
          │ (read/store bytes)  │◄───┤   ($attachment      │
          │                     │    │    placeholder      │
          │                     │    │    resolution)      │
          └─────────────────────┘    └─────────────────────┘
                    ▲
                    │ register
                    ▼
          ┌─────────────────────┐
          │ AttachmentRegistry  │  ← load_attachment looks here
          │ (provider=Generated)│
          └─────────────────────┘
```

## Storage adapter selection

`EngineConfig::from_env` (`src/dag_engine/engine.rs:73`) elige uno de tres adapters según env vars:

| `COLMENA_LOCAL` | Adapter | URL shape | Bytes location |
|---|---|---|---|
| `true` | `LocalHttpStorageAdapter` | `http://127.0.0.1:<port>/files/<key>` | Disco bajo `COLMENA_LOCAL_STORAGE_DIR` (default `/tmp/colmena-out`) |
| `false` | `HttpCallbackStorageAdapter` | URL firmada GCS (`https://storage.googleapis.com/...?X-Goog-Signature=...`) | GCS bucket — colmena pide el signed PUT URL al callback de la host application |
| unset | Implicit fallback (back-compat): callback si vars set → local-http si dir set → else `LocalCacheStorageAdapter` (in-memory) | varía | varía |

**Default values cuando `COLMENA_LOCAL=true`**:
- `COLMENA_LOCAL_STORAGE_DIR=/tmp/colmena-out`
- `COLMENA_LOCAL_STORAGE_PORT=8765` (use `0` para que el OS asigne uno random)

**Required vars cuando `COLMENA_LOCAL=false`** (hard-fail si faltan):
- `COLMENA_STORAGE_CALLBACK_URL` (ej: `https://your-host-api.example.com/internal/gcs/sign-put`)
- `COLMENA_STORAGE_CALLBACK_SECRET` (shared con el `InternalServiceGuard` del lado API)

### Logging al startup

Cada modo emite un `tracing::info!` que permite verificar en runtime qué adapter está activo:

```
storage_mode_selected mode=local      adapter=LocalHttpStorageAdapter dir=/tmp/colmena-out port=8765
storage_mode_selected mode=prod       adapter=HttpCallbackStorageAdapter callback_url=https://...
storage_mode_selected mode=implicit-local       (hint: setear COLMENA_LOCAL=true explícito)
storage_mode_selected mode=implicit-in-memory   (warning: bytes se pierden al cerrar)
```

## URL semantics — dev/prod symmetry

Esto es lo más importante para entender por qué el flow funciona idéntico en dev y prod sin código condicional:

| Concepto | Significado |
|---|---|
| `storage_key` | Handle canónico — opaco, estable. Forma: `<uuid>.<ext>` en LocalHttp, `chat-attachments/<userId>/<sessionId>/generated/<cuid>-<name>` en HttpCallback. Es lo que va en `$attachment:<storage_key>` placeholders. |
| `read_url` | URL fetchable — `http://127.0.0.1:8765/files/<key>` en dev (axum local), signed GCS URL en prod. Misma forma HTTP en ambos. |

El agente (LLM) ve el tool output con la siguiente forma (Plan B, 2026-05-25):
```json
{
  "images": [{
    "document_id": "img_revenue_chart_a1b2c3",           ← úsalo en $attachment:<id> o load_attachment
    "mime_type": "image/png",
    "size_bytes": 1458957,
    "description": "Image generated with gpt-image-1: A puppy with a blue cap"
  }],
  "provider": "openai",
  "model": "gpt-image-1"
}
```

> Plan B (2026-05-25) eliminó los campos legacy `attachment_id` y `url` del tool
> result. Los consumidores que necesiten una URL renderizable deben hacer lookup
> por `document_id` vía un endpoint dedicado del backend (joineando contra
> `conversation_attachments`). Internamente el `storage_key` y la `read_url`
> siguen registrados — solo dejaron de exponerse al LLM.

**El LLM nunca recibe los bytes** — solo el handle estable (`document_id`) + metadata. Para renderizar al usuario o abrir en una nueva pestaña, el frontend resuelve `document_id` → signed URL vía un endpoint del backend.

## Nodos disponibles

### `image_generation`

| Config field | Required | Description |
|---|---|---|
| `provider` | sí | `openai` o `google` |
| `model` | sí | `gpt-image-1`, `dall-e-3` (OpenAI); `imagen-4.0-generate-001` (Vertex) |
| `api_key` | sí (openai) | Soporta `${OPENAI_API_KEY}` + secure-value placeholders |
| `prompt` | sí | Detalle del prompt — inputs-over-config, podés pasarlo por edge o LLM tool arg |
| `size` | opcional | Default `1024x1024` |
| `quality` | opcional (openai) | `low | medium | high | auto` |
| `n` | opcional | Default 1, max 10 (clamped) |
| `google_project_id` | opcional* (google) | **Best practice: omitir.** Si no está en config se lee de `GOOGLE_CLOUD_PROJECT` (o `GOOGLE_PROJECT_ID`) env var del worker. |
| `google_location` | opcional (google) | Default `us-central1`. Si no está en config se lee de `GOOGLE_CLOUD_LOCATION` (o `GOOGLE_LOCATION`) env var. |

**Output**: `{ "output": { "images": [...], "provider", "model" } }`.

**Best practice para `provider=google`:** dejá `google_project_id` y `google_location` FUERA del grafo JSON. El worker los inyecta desde sus env vars en deploy time (`GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION`). El mismo grafo así es portable entre dev/staging/prod — cada environment provee su propio project sin tocar el JSON. Solo hardcodeá en el grafo si necesitás targetear un project distinto al del worker (raro).

**Auth de Google Vertex** se hace internamente con `yup-oauth2` vía **Application Default Credentials**. Discovery order: (1) `GOOGLE_APPLICATION_CREDENTIALS` key file path, (2) `gcloud auth application-default login` creds en `~/.config/gcloud/`, (3) GCE/Cloud Run metadata server (runtime SA). El access token resultante se cachea ~50min en una `tokio::sync::Mutex<Option<CachedToken>>` para evitar revalidar en cada call.

**Sample graph**: `tests/graphs/media/image_generation_basic.json`.

### `image_edit`

| Config field | Required | Description |
|---|---|---|
| `provider` | sí | `openai` (único soportado hoy) |
| `model` | opcional | Default `gpt-image-1` |
| `api_key` | sí | OpenAI key |
| `source_url` | sí | `data:` URI, `http(s)://` URL, o storage handle `local://<key>` / `chat-attachments/<key>`. **NO** acepta `document_id` pelado ni `$attachment:<document_id>` (ver "Limitación conocida") |
| `mask_url` | opcional | PNG con transparencia marcando el área a editar |
| `prompt` | sí | Describe la edición |
| `size`, `quality`, `n` | opcional | Igual que image_generation |

**Output**: mismo shape que `image_generation` → resultado encadenable.

**Chaining nativo gen→edit** — ⚠️ **actualmente roto bajo Plan B.** Plan B
(2026-05-25) eliminó el campo `url` del tool result, así que el patrón legacy de
pasar `images.0.url` ya no aplica. Y cablear `images.0.document_id` →
`edit.source_url` **tampoco funciona**: `image_edit.source_url` no resuelve un
`document_id` pelado ni un placeholder `$attachment:<document_id>` (solo
`data:`, `http(s)://`, `local://<key>`, `chat-attachments/<key>`). Ver
"Limitación conocida" más abajo. Hasta que haya fix, solo encadenás pasando una
URL `http(s)://` / `data:` independientemente fetchable a `edit.source_url`.

### `tts`

| Config field | Required | Description |
|---|---|---|
| `provider` | sí | `openai`, `elevenlabs`, `google` |
| `model` | sí | `tts-1`/`tts-1-hd`/`gpt-4o-mini-tts` (openai); `eleven_multilingual_v2`/`eleven_turbo_v2_5` (elevenlabs); `gemini-2.5-flash-preview-tts` (google) |
| `api_key` | sí | Provider key |
| `text` | sí | Texto a sintetizar |
| `voice` | sí | OpenAI: `alloy|echo|fable|onyx|nova|shimmer`. ElevenLabs: voice_id alfanumérico. Google: prebuilt voice name (ej: `Kore`) |
| `format` | opcional | `mp3` (default), `wav`, `opus`, `pcm`. Google ignora — siempre L16. |
| `speed` | opcional | 0.25-4.0 (openai/google). ElevenLabs ignora. |

**Output** (Plan B, 2026-05-25): `{ "output": { "audio": { document_id, mime_type, size_bytes, duration_ms, description }, provider, model } }`. Los campos legacy `attachment_id` y `url` fueron eliminados — usá `document_id` con `$attachment:<document_id>` o `load_attachment`.

**Sample graph**: `tests/graphs/media/tts_basic.json`.

## Artifacts unification — el agente como ciudadano de primera

Los outputs generados se registran automáticamente en `AttachmentRegistry` con `provider: ProviderKind::Generated` (variant sintético, ver `src/llm/domain/llm_provider.rs`). Esto desbloquea 3 capacidades:

### 1. "Ver" la propia generación — `load_attachment`

Patrón idéntico al de attachments uploaded por el usuario (ver [`31_load_attachment.md`](31_load_attachment.md)):

```
1. agente llama generate_image → tool output con document_id
2. agente llama load_attachment(document_id=<document_id>)
3. AgentService intercepta el LOAD_ATTACHMENT sentinel
4. Resolver hace: lookup(provider=current) → None →
   lookup(provider=Generated) → row found →
   storage.read(storage_key) → bytes →
   file_provider.upload_streaming(bytes) → provider_file_id en Files API del provider →
   registry.upsert con (provider=current, provider_file_id=nuevo)
5. AgentService inyecta synthetic user message con FileData::Uploaded
6. Siguiente turn: LLM "ve" la imagen vía vision multimodal
```

Esto se llama **cross-provider lazy upload**. Si generaste con OpenAI y después cambias a Anthropic en otro turn, la primera vez que `load_attachment` se llame desde Anthropic, el bytes se uploadea a Anthropic Files API y la fila se persiste para que sucesivas calls hagan fast path.

### 2. Editar una imagen generada — `image_edit` chaining

`image_edit` acepta en `source_url`: `data:` URIs, `http(s)://` URLs, y
storage handles `local://<key>` / `chat-attachments/<key>` (resueltos vía
`storage.read` en `image_edit.rs::fetch_image`). Si tu grafo tiene una URL
fetchable independiente (no proveniente de un tool result), el chaining
funciona normalmente.

> ### ⚠️ Limitación conocida (2026-05-28) — chaining LLM-driven gen→edit roto bajo Plan B
>
> **El encadenamiento `image_generation` → `image_edit` manejado por el LLM
> está actualmente roto.** Razón:
>
> - Plan B (2026-05-25) eliminó los campos legacy `attachment_id` y `url` del
>   tool result de `image_generation`/`image_edit`/`tts`. Ahora exponen **solo
>   `document_id`** (un id opaco tipo `img_image_0_ge0png`).
> - Pero `image_edit.source_url` (en `image_edit.rs::fetch_image`) **NO**
>   resuelve un `document_id` pelado ni un placeholder `$attachment:<document_id>`.
>   El resolver `$attachment:` está cableado **solo en el nodo `http_request`**,
>   no globalmente en `dag_tool_executor`.
>
> Resultado: un LLM que pase el `document_id` del tool anterior como
> `source_url` **falla**. El chaining estático por edge que antes pasaba el
> viejo `url`/storage_key tampoco funciona, porque `url` fue removido.
>
> **Workaround hoy:** pasá a `source_url` una URL independientemente fetchable
> (un signed URL `http(s)://` o un `data:` URI que NO venga de un tool result
> previo).
>
> **Fix futuro** (no implementado): hacer que `image_edit` resuelva
> `$attachment:<document_id>` vía el attachment registry, o que el tool
> executor resuelva `$attachment:` en todos los args de tool. Tracked en
> [`docs/superpowers/specs/2026-05-25-colmena-pending-followups.md`](../superpowers/specs/2026-05-25-colmena-pending-followups.md) §2.E.

### 3. Enviar a endpoint externo — `$attachment:<key>` placeholder

`http_request` ahora escanea recursivamente el body buscando strings que empiecen con `$attachment:` y los reemplaza por `data:<mime>;base64,...` antes de mandar el HTTP request.

```json
{
  "node_type": "http_request",
  "fixed_config": {
    "method": "POST",
    "headers": { "Content-Type": "application/json" }
  },
  "node_schema": {
    "base_url": { "type": "string", "required": true },
    "body": {
      "type": "object",
      "required": true,
      "properties": {
        "image": {
          "type": "string", "required": true,
          "description": "MUST be '$attachment:<document_id>' from a previous generate_image"
        }
      }
    }
  }
}
```

El LLM pasa `body: { image: "$attachment:abc.png" }`. El engine resuelve a `body: { image: "data:image/png;base64,iVBORw0..." }` antes del POST. **El LLM ve solo el handle corto, nunca los bytes.**

## Universal binary scrubber

`DagToolExecutor.execute()` aplica un scrubber a todo tool result antes de devolverlo al agent loop. Ver `src/dag_engine/infrastructure/dag_tool_executor.rs:1048+`.

Reglas:
1. Strings con prefijo `data:` que contienen `;base64,` → reemplazo por `[binary elided: mime=<mime>, encoded_size=<N> bytes]`.
2. Strings cuyo `.len()` excede `max_tool_result_bytes` (default 50 KB, configurable via `llm_call.config.max_tool_result_bytes`) → reemplazo por `[truncated: original_size=N bytes (cap=M bytes); request via load_attachment if needed]`.
3. Otros tipos (números, bools, nulls, strings cortas) pasan sin tocar.

Esto se aplica **recursivamente** sobre todo JSON Value (objects, arrays nested). El caso que originó esto: `httpbin.org/post` echo-eaba el body con la imagen base64 en su response, y ese response volvía al LLM como tool result — 1.6MB → 1M tokens → TPM rate limit. Con el scrubber, ese mismo echo se colapsa a unos cientos de bytes.

## Setup local rápido

Agregar al `.env`:

```bash
COLMENA_LOCAL=true
COLMENA_LOCAL_STORAGE_DIR=/tmp/colmena-out
COLMENA_LOCAL_STORAGE_PORT=8765
```

Después:

```bash
mkdir -p /tmp/colmena-out
set -a && source .env && set +a
cargo run --bin dag_engine -- run tests/graphs/media/image_generation_basic.json
```

Verás:
- Log al startup: `storage_mode_selected mode=local adapter=LocalHttpStorageAdapter dir=/tmp/colmena-out port=8765`
- En el output JSON: `"url": "http://127.0.0.1:8765/files/<uuid>.png"`
- En disco: `ls /tmp/colmena-out/` muestra el archivo (1-3MB típico para 1024x1024 PNG)
- Podés `open /tmp/colmena-out/<uuid>.png` para inspeccionarlo, o pegar la URL en el navegador DURANTE el run

**Lifecycle**: el server HTTP local muere cuando termina el proceso. Los archivos en disco persisten. La URL deja de responder pero el archivo sigue siendo `open`-eable.

## Troubleshooting

| Síntoma | Causa probable | Fix |
|---|---|---|
| Tool result llega vacío al LLM y el modelo improvisa el tool call como JSON inline en texto | `tool_configurations` parsing falló silenciosamente (común con `node_schema` sin `type` en algún field LLM-visible) | Ya no aplica desde 2026-05-20 — el parser hace fail-hard con mensaje pedagógico. Si lo ves en logs viejos, mirar el error literal — apunta al field exacto. |
| Run hit OpenAI TPM rate limit en el siguiente turn después de un tool call | Tool result devolvió un body con base64 grande (echo de httpbin, o un endpoint que devuelve la imagen procesada) | El scrubber lo elide automático desde 2026-05-20. Si querés permitir bodies grandes (>50KB) para un caso específico, setear `max_tool_result_bytes: 200000` en el config del `llm_call`. |
| LLM no llama tools y solo escribe texto sobre lo que va a hacer | `tool_configurations` malformado (cae al silent-empty fallback en versiones anteriores), o modelo chico (gpt-4o-mini) con instrucciones débiles | Verificar logs del startup buscando warns sobre `tool_configurations failed to parse`. Subir a gpt-4o + system_message más estricto ("invoke the tool directly, do not describe what you are about to do"). |
| URL `http://127.0.0.1:8765/files/...` devuelve connection refused | El proceso `cargo run` ya terminó — el server local solo vive durante el run | Usar el path de disco: `open /tmp/colmena-out/<key>` |
| `COLMENA_LOCAL=false` y error al startup pidiendo callback URL | Estás en modo prod sin haber configurado el callback de la host application | Setear `COLMENA_STORAGE_CALLBACK_URL` + `COLMENA_STORAGE_CALLBACK_SECRET` o cambiar a `COLMENA_LOCAL=true` |
| Cross-provider lazy upload da error "no OutputStorageRepository wired" | El AttachmentResolver no recibió storage adapter — bug si tenés `EngineConfig::from_env` standard | Verificar que `LlmNode::with_storage()` se llama en el registry init (ver `src/dag_engine/infrastructure/registry.rs`) |

## Referencias

- **Plan canónico**: [`docs/superpowers/plans/2026-05-19-multimedia-generation-nodes.md`](../superpowers/plans/2026-05-19-multimedia-generation-nodes.md) — diseño + status banner + delta shipped/planned.
- **Schema**: [`docs/node_configurations.json`](../node_configurations.json) → entradas `image_generation`, `image_edit`, `tts` + categoría `media`.
- **Sample graphs**: `tests/graphs/media/` (4 standalone) + `tests/graphs/agents/multimedia_agent*.json` (2 con agent).
- **Load attachment base** (input direction, scoping): [`31_load_attachment.md`](31_load_attachment.md).
- **Tool execution flow** (cómo se ejecutan tool calls): [`22_tool_execution_flow.md`](22_tool_execution_flow.md).
- **Code entry points**:
  - `src/storage/` — port + 3 adapters.
  - `src/dag_engine/infrastructure/nodes/{image_generation,image_edit,tts}.rs` — nodos.
  - `src/llm/infrastructure/{openai_tts_adapter,elevenlabs_tts_adapter,google_tts_adapter}.rs` — TTS adapters.
  - `src/dag_engine/infrastructure/dag_tool_executor.rs:1048+` — scrubber.
  - `src/dag_engine/engine.rs:73+` — `COLMENA_LOCAL` selection logic.

## Host-side integration (out of scope for this repository)

Colmena defines the **client contract** for `HttpCallbackStorageAdapter` —
what the worker sends, what response it expects. The host application that
consumes colmena (e.g. a chat backend) is responsible for implementing the
server side. The contract is small:

**Request from worker → host:**
```
POST <COLMENA_STORAGE_CALLBACK_URL>
Headers:
  X-Internal-Token: <shared secret from COLMENA_STORAGE_CALLBACK_SECRET>
  Content-Type: application/json
Body:
{
  "session_id":        "<colmena session id>",
  "agent_session_id":  "<optional conversation id>",
  "mime_type":         "image/png",
  "filename":          "image_0.png",
  "purpose":           "generated_output"
}
```

**Expected response (200):**
```json
{
  "put_url":     "https://storage.googleapis.com/...?X-Goog-Signature=...",
  "read_url":    "https://storage.googleapis.com/...?X-Goog-Signature=...",
  "storage_key": "<host-derived path inside the bucket>"
}
```

The host typically derives the storage_key from the `session_id` lookup
(mapping it to a user / conversation in its own DB), reuses its existing
GCS signed-URL helpers, and persists an attachment row when the
worker emits the corresponding `tool_call_finish` event.

For an example reference implementation living in a private host
application this codebase ships against, see the host's internal docs —
the contract above is the only thing colmena needs to be compatible.
