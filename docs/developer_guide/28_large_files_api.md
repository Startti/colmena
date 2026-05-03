# Archivos grandes vía Files API

Esta guía describe cómo el nodo `llm_call` maneja archivos adjuntos en el JSON del DAG: desde inline base64 hasta archivos de cientos de MB referenciados por signed URL de GCS, con cache persistido y streaming pipe end-to-end a las Files APIs de Anthropic, OpenAI y Gemini.

## Contrato de entrada

El array `files` dentro de `config` o `inputs` de un nodo `llm_call` acepta entradas con esta forma:

```json
{
  "id":         "doc-abc-123",
  "mime_type":  "application/pdf",
  "filename":   "report.pdf",
  "size_bytes": 47185920,
  "data":       null,
  "url":        "https://storage.googleapis.com/<bucket>/<path>?X-Goog-Signature=..."
}
```

Reglas:

- **`id`**: requerido cuando hay `url` (es la llave de cache `(document_id, provider)`).
- **`mime_type`** y **`filename`**: siempre presentes; con defaults `application/octet-stream` y `upload.file`.
- **Mutuamente excluyentes** (preferencia `data > url > path`): solo uno de:
  - `data`: base64 puro (sin prefijo `data:mime;base64,`). Solo válido si raw < 30 MB.
  - `url`: signed URL HTTPS a GCS. TTL típico 6h.
  - `path`: legacy local (solo dev/tests). Solo válido si archivo < 30 MB.
- **Threshold del emisor**: el sistema upstream que genera el JSON decide a 30 MB. Colmena no decide threshold; confía en el emisor.
- **`size_bytes`**: hint, no ground truth. Validar contra los bytes reales tras download/decode.

Errores explícitos del parser:

- `LlmError::DataFieldTooLarge { size }` — `data` con `size_bytes > 30 MB`. Bug del emisor.
- `LlmError::PathFieldTooLarge { size }` — `path` apunta a archivo > 30 MB.
- `LlmError::UrlWithoutDocumentId` — `url` presente pero sin `id`.

## Flujo de resolución

```
[parser del nodo: parse_file_entries]
    ↓
FileSource::InlineBytes (data o path) → directo al adapter
FileSource::SignedUrl (url) → resolve_files
    ↓
[short-circuit por provider+mime]
    image + Anthropic → mantener SignedUrl, adapter emite source.type=url
    image + OpenAI    → mantener SignedUrl, adapter emite image_url.url
    otros             → continuar al cache
    ↓
[lookup PostgresFileCache (document_id, provider)]
    HIT alive    → reutilizar provider_file_id (skip download/upload)
    HIT expirado → invalidar + re-upload
    MISS         → continuar
    ↓
[pipe end-to-end]
    SignedUrlDownloader::stream(url) → BoxedByteStream
        → FileProviderRepository::upload_streaming(stream, mime, filename)
            → ProviderFileRef
    ↓
[upsert cache + reemplazar source con FileSource::Uploaded]
    ↓
[adapter emite formato correcto del provider con file_id/file_uri]
```

## Estrategia por provider

| Provider  | Imagen | PDF |
|-----------|--------|-----|
| **Anthropic** | URL passthrough (Anthropic baja la URL) | Files API + `file_id` (header beta `files-api-2025-04-14` requerido también en `/v1/messages`) |
| **OpenAI**    | URL passthrough en chat completions (`image_url.url`) | Files API + `file_id` en Responses API (omitir `filename`) |
| **Gemini**    | Files API + `fileData.fileUri` | Files API resumable upload (chunks de 8 MB exactos) |

Detalles importantes:

- **Anthropic** rechaza `{"type": "file", "file_id": "..."}` para `image.source` — solo acepta `base64` o `url`. Para PDFs sí lo acepta, pero **requiere** el header `anthropic-beta: files-api-2025-04-14` también en la llamada de generación.
- **OpenAI** Chat Completions requiere `image_url.url`; `file_id` para imágenes solo funciona vía Responses API. Y en Responses, `file_id` y `filename` son **mutuamente excluyentes**.
- **Gemini** resumable upload requiere chunks intermedios de tamaño **exactamente** múltiplo de 8 MB (`CHUNK_SIZE`). El último chunk puede ser de cualquier tamaño.

## Cache persistido en Postgres

Tabla `provider_file_cache` (migración `20260502000001_provider_file_cache.sql`):

```sql
CREATE TABLE provider_file_cache (
    document_id      TEXT NOT NULL,
    provider         TEXT NOT NULL,
    provider_file_id TEXT NOT NULL,
    mime_type        TEXT NOT NULL,
    filename         TEXT NOT NULL,
    size_bytes       BIGINT,
    uploaded_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at       TIMESTAMPTZ,
    last_used_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (document_id, provider)
);
```

Conexión: usa siempre `DATABASE_URL` env (transversal, no per-node como la memoria de conversación). Si `DATABASE_URL` no está set, el cache se desactiva y cada run sube de nuevo (se preserva el path mínimo de operación).

Heurística TTL `is_likely_alive` (margen de 5 min de seguridad por skew de reloj):

- **Gemini**: `expires_at = uploaded_at + 48h` (Gemini Files API expira a 48h).
- **Anthropic** y **OpenAI**: `expires_at = NULL` (no expiran).

UPSERT idempotente con `ON CONFLICT (document_id, provider) DO UPDATE`: si dos requests concurrentes con mismo `id` ambos hacen miss y suben, el segundo gana en cache; el archivo del primero queda huérfano en el provider (ver [deuda técnica #4+#5](../superpowers/specs/2026-05-02-large-document-files-api-tech-debt.md#4--5-huérfanos-cache-rows--provider-files)).

`lookup` usa `UPDATE ... RETURNING *` (no `SELECT`) para que `last_used_at` se actualice en cada cache hit — base para futuras métricas LRU y janitor. Si la fila no existe, devuelve 0 rows = MISS (mismo resultado que el SELECT anterior).

## Streaming pipe end-to-end

El download desde GCS y el upload al provider corren **concurrentemente**. Los bytes fluyen chunk a chunk vía `reqwest::Body::wrap_stream`, con backpressure TCP automático:

1. `SignedUrlDownloader::stream(url)` devuelve un `BoxedByteStream` lazy (no descarga nada todavía).
2. `FileProviderRepository::upload_streaming(stream, ...)` consume el stream chunk a chunk.
3. El cuerpo del POST multipart al provider se construye con `Body::wrap_stream`, que reqwest jala perezosamente.
4. Cada chunk de ~64KB del kernel TCP buffer fluye hacia el provider sin pasar por un buffer intermedio.

RAM en vuelo ~1 MB (chunk + sockets) independiente del tamaño del archivo, **excepto Gemini** que acumula chunks de 8 MB para el protocolo resumable.

## Trazabilidad

`COLMENA_VERBOSE=1` o `--verbose` activa los logs `[file-resolve]`:

```
[file-resolve] DATABASE_URL set — building PostgresFileCache for provider_file_cache table
[file-resolve] resolving 1 file(s) for provider <provider>
[file-resolve] '<filename>' (id=<id>) looking up cache for provider <provider>
[file-resolve] '<filename>' (id=<id>) cache HIT alive (file_id=..., expires_at=...) — skipping download/upload
[file-resolve] '<filename>' (id=<id>) cache MISS — will download + upload
[file-resolve] '<filename>' (id=<id>) opening signed-URL stream from GCS
[file-resolve] '<filename>' (id=<id>) piping stream → <provider> Files API upload
[file-resolve] '<filename>' (id=<id>) upload complete: provider_file_id=<id>
[file-resolve] '<filename>' (id=<id>) cache upserted (expires_at=...)
[file-resolve] '<filename>' (id=<id>) image + <provider> — passing signed URL directly to adapter (no upload)
[file-resolve] '<filename>' (id=<id>) intra-request dedup HIT — reusing file_id <id>
```

## Test graphs

`tests/graphs/media/`:

- `image_url_anthropic.json` / `image_url_openai.json` / `image_url_gemini.json` — imágenes JPEG vía signed URL (path URL passthrough en Anthropic+OpenAI, Files API en Gemini).
- `pdf_url_anthropic.json` / `pdf_url_openai.json` / `pdf_url_gemini.json` — PDFs ≥ 30 MB vía signed URL (path Files API en los 3).

Las URLs firmadas en los JSONs expiran a las 6 h. Para regenerarlas ver `tests/graphs/media/README.md`.

Ejecución:

```sh
set -a; source .env; set +a
COLMENA_VERBOSE=1 cargo run --bin dag_engine -- run tests/graphs/media/pdf_url_anthropic.json
```

Verificar la fila en Postgres:

```sh
psql "$DATABASE_URL" -c "SELECT document_id, provider, provider_file_id, expires_at, last_used_at FROM provider_file_cache;"
```

## Límites de producto a tener en cuenta

Los siguientes límites son del modelo/API de cada provider, **no del transporte**. Si los excedes, el upload se hace bien pero la generación falla:

| Provider | Límite del modelo |
|----------|-------------------|
| Anthropic | 100 páginas máx por PDF; ventana de contexto ~200k tokens en Haiku 4.5 |
| OpenAI | 32 MB de pull interno tras Files API; gpt-4o-mini procesa ~1M tokens vía `file_id` en Responses API |
| Gemini | 3000 páginas teóricas; algunos modelos rechazan files >>20 MB referenciados con "files bytes too large to be read" |

Si tu archivo excede el límite del modelo, el error es del provider, no del código nuestro. La estrategia recomendada para documentos muy grandes es RAG (extracción de chunks de texto antes del LLM call).

## Errores observables

| Error | Causa | Quién lo emite |
|-------|-------|----------------|
| `DataFieldTooLarge { size }` | `data` con `size_bytes > 30MB`. Bug del emisor. | Parser del nodo |
| `PathFieldTooLarge { size }` | `path` apunta a archivo local > 30MB. | Parser del nodo |
| `UrlWithoutDocumentId` | `url` presente sin `id`. Bug de contrato. | Parser del nodo |
| `SignedUrlFetchFailed { status }` | GCS rechazó GET (URL expirada, archivo no existe). | `SignedUrlDownloader` |
| `FileApiUploadFailed { provider, message }` | El provider rechazó upload (cuota, formato, key inválida). HTTP failure real. | Files API adapter |
| `InvalidMimeType { mime, message }` | Mime malformado — violación de precondición del caller, no falla de upload (no se hizo HTTP). | Files API adapter (Anthropic, OpenAI) |
| `ProviderFileNotFound { provider_file_id }` | Cache stale: el archivo fue borrado del provider. **Recovery automático**: snapshot de la SignedUrl original + reset + re-upload + 1 retry. | Adapter del LLM |
| `AllFilesFailedToResolve` | Todos los archivos del request fallaron en materializar. | `LlmCallUseCase::resolve_files` |
| `InternalError` | `SignedUrl` llegó al adapter sin haber sido resuelto por el use case. Bug de wiring. | Adapter del LLM |

## Arquitectura interna

```
src/libs/colmena/src/llm/
├── domain/                                ← cero dependencias de infrastructure
│   ├── llm_message.rs                    ← FileData con FileSource enum
│   ├── file_provider_repository.rs       ← puerto + BoxedByteStream
│   ├── file_cache_repository.rs          ← puerto + CachedFileEntry::is_likely_alive
│   ├── signed_url_fetcher.rs             ← puerto SignedUrlFetcher (descarga streaming)
│   └── file_provider_factory_port.rs     ← puerto FileProviderFactoryPort (build per-kind)
├── application/                           ← solo depende de puertos del domain
│   └── llm_call_use_case.rs              ← resolve_files + snapshot SignedUrl + retry recovery
└── infrastructure/files/                  ← implementaciones concretas
    ├── signed_url_downloader.rs          ← impl SignedUrlFetcher (reqwest)
    ├── anthropic_files_api.rs            ← multipart + beta header
    ├── openai_files_api.rs               ← multipart + purpose=user_data
    ├── gemini_files_api.rs               ← resumable 3-fase (start, chunks 8MB, finalize)
    ├── postgres_file_cache.rs            ← lookup/upsert/invalidate vía sqlx
    └── file_provider_factory.rs          ← impl FileProviderFactoryPort
```

**Arquitectura hexagonal estricta**: la capa `application/` no tiene `use crate::llm::infrastructure::*` en producción (las únicas referencias quedan en bloques `#[cfg(test)]` como fixtures). El use case recibe los adapters concretos vía builders:

```rust
LlmCallUseCase::new(repo)
    .with_file_cache(Arc::new(postgres_cache))
    .with_file_provider_factory(Arc::new(FileProviderFactory::new()))
    .with_signed_url_fetcher(Arc::new(SignedUrlDownloader::new()));
```

Wiring en producción: el nodo `llm_call` (`dag_engine/infrastructure/nodes/llm.rs`) — que es código de infraestructura — construye los concretos y los pasa a `LlmCallUseCase::resolve_files`.

**Recovery del 404 (`ProviderFileNotFound`)**: el use case toma un snapshot `(document_id → SignedUrl)` antes de la primera resolución. Si el LLM falla con `ProviderFileNotFound { provider_file_id }`:
1. Invalida la fila del cache.
2. `reset_uploaded_files_with_id` revierte `Uploaded(bad_id)` → `SignedUrl(orig)` consultando el snapshot.
3. `resolve_files` corre de nuevo: cache MISS → re-download → re-upload.
4. Un único retry al LLM.

Si el archivo originalmente vino como `Uploaded` directo (sin SignedUrl), no hay URL para recuperar y el retry no ayuda — best-effort en ese único caso.

## Deuda técnica y trabajo por hacer

Documento dedicado con lista priorizada, severidad y plan de solución por item:

📋 **[Deuda técnica del feature](../superpowers/specs/2026-05-02-large-document-files-api-tech-debt.md)**

Resumen ejecutivo:

| Item | Severidad | Estado |
|------|-----------|--------|
| Retry on `ProviderFileNotFound` no recupera | Alta | ✅ Resuelto (snapshot + reset) |
| `last_used_at` no se actualiza en cache hit | Baja | ✅ Resuelto (UPDATE...RETURNING) |
| Layer leak: `LlmCallUseCase` importa de `infrastructure` | Media | ✅ Resuelto (puertos `SignedUrlFetcher` + `FileProviderFactoryPort`) |
| Huérfanos en cache + provider | Media | 🔍 Análisis profundo hecho, implementación pospuesta |
| `ProviderKind::Mock` fallback silencioso en lookup | Baja | ✅ Resuelto (fail-fast + tracing estructurado) |
| Mime malformado clasificado como upload error | Baja | ✅ Resuelto (variante `InvalidMimeType`) |
| Tests de integración E2E reproducibles (sin signed URLs) | Media | Pendiente |
| Métricas Prometheus para cache hit-rate | Baja | Pendiente |
| Cache hit cross-session por `sha256` (YAGNI) | Muy baja | Pendiente (descartado por spec) |

**Ningún item pendiente es bloqueante.** Ver el doc para detalles, soluciones propuestas y decisiones requeridas.
