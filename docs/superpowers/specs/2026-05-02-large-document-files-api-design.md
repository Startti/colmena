# Diseño: Soporte para documentos grandes vía Files API de proveedores LLM

**Fecha**: 2026-05-02
**Estado**: Draft (pendiente de aprobación del usuario)
**Autor**: Daniel García (con asistencia de Claude)

## Contexto y problema

Hoy Colmena maneja archivos en `llm_call` exclusivamente vía base64 inline:

- El nodo recibe `FileEntry { mime_type, filename, data }` (donde `data` puede ser base64 o `path` local).
- El use case decodifica los bytes y los pasa a los adapters de LLM.
- Cada adapter (`gemini_adapter.rs`, `anthropic_adapter.rs`, `openai_adapter.rs`) embebe los bytes como base64 inline en el request a la API del proveedor.

Esto rompe para archivos por encima de los límites inline de cada proveedor:

| Proveedor  | Límite inline (payload total) |
|------------|-------------------------------|
| Anthropic  | 32 MB                         |
| Gemini     | 50 MB para PDFs / 100 MB otros |
| OpenAI     | 50 MB                         |

Para documentos como PDFs de 200 MB, los tres proveedores ofrecen **Files APIs** dedicadas: el archivo se sube primero, devuelven un identificador (`file_id` o `file_uri`), y ese identificador se referencia en la llamada de generación.

## Alcance

### Dentro de scope

1. Aceptar `FileEntry` con campo `url` (signed URL de GCS, ≥ 30 MB) además del `data` actual.
2. Para entries con `url`: descargar desde GCS y subir al Files API del proveedor activo del nodo.
3. Persistir en PostgreSQL la referencia `(document_id, provider) → provider_file_id` para reutilizar entre ejecuciones de DAG.
4. Validar disponibilidad del archivo antes de usarlo (heurística por `expires_at` con recovery on 404).
5. Streaming pipe end-to-end (descarga → upload sin bufferizar a RAM ni disco) para soportar alta concurrencia con archivos de cientos de MB.
6. Implementación en los tres proveedores: Gemini, Anthropic, OpenAI.
7. Mantener arquitectura hexagonal: nuevo puerto en `domain`, adapters en `infrastructure`.

### Fuera de scope (YAGNI)

- Decisión auto-threshold inline-vs-FilesAPI en el egreso. El emisor ya decidió a 30 MB. Si llega `data`, va inline; si llega `url`, va Files API. Sin override.
- Flag de configuración `upload_mode`.
- Caché por sha256 de contenido. Lo descartamos porque el `id` del emisor es suficiente.
- Janitor de archivos huérfanos en proveedores. Se posterga a follow-up issue cuando se vea volumen real.
- Soporte para AWS S3 / Azure Blob signed URLs. Solo GCS por ahora.
- Cleanup explícito de archivos en proveedores al terminar sesión.
- Validación pre-call autoritativa (HEAD/GET al Files API del proveedor en cada uso). La heurística por `expires_at` + retry on 404 es suficiente.

## Contrato del emisor

El emisor (sistema upstream que genera el JSON del DAG) decide cómo transportar el archivo a Colmena con un threshold de 30 MB. Cada `FileEntry` en el array `files` de un nodo `llm_call` tiene esta forma:

```json
{
  "id":         "po2pq7nx4mqzvxcjbn8q",
  "mime_type":  "application/pdf",
  "filename":   "report.pdf",
  "data":       null,
  "url":        "https://storage.googleapis.com/<bucket>/<path>?X-Goog-Algorithm=...&X-Goog-Expires=21600&X-Goog-Signature=...",
  "size_bytes": 47185920
}
```

Reglas:

- `id` siempre presente, **único por llamada** (el emisor garantiza que no hay concurrencia con el mismo `id`).
- `id` es la clave del cache `(document_id, provider)` para verificar si ya se subió o expiró. **No reemplaza ni se relaciona con `filename`**: `filename` es el nombre humano del archivo y se usa como tal en el upload al provider; `id` es solo una llave técnica de cache.
- `data` y `url` son mutuamente excluyentes en payloads válidos (preferir `data` si por bug llegan ambos).
- `data` es base64 puro (sin prefijo `data:mime;base64,`). Si arriba con `size_bytes > 30 MB` → error de violación de contrato.
- `url` es una signed URL HTTPS a `storage.googleapis.com`. TTL típico 6 h.
- `size_bytes` es hint, no verdad. Validar contra los bytes reales tras download/decode.
- `path` (legacy, solo testing local): tratado igual que `data` con un threshold del lado del receptor — si el archivo en disco supera 30 MB, error.

## Diseño

### Capas (hexagonal)

```
src/libs/colmena/src/llm/
├── domain/
│   ├── llm_message.rs              ← FileData se vuelve enum-driven
│   ├── file_provider_repository.rs ← NUEVO: puerto FilesAPI
│   └── file_cache_repository.rs    ← NUEVO: puerto cache PostgreSQL
├── application/
│   └── llm_call_use_case.rs        ← orquesta materialización + cache + upload
└── infrastructure/
    ├── gemini_adapter.rs           ← extender: aceptar ProviderFileRef
    ├── anthropic_adapter.rs        ← extender: aceptar ProviderFileRef
    ├── openai_adapter.rs           ← extender: aceptar ProviderFileRef
    └── files/                      ← NUEVO subdir
        ├── mod.rs
        ├── signed_url_downloader.rs
        ├── gemini_files_api.rs
        ├── anthropic_files_api.rs
        ├── openai_files_api.rs
        └── postgres_file_cache.rs
```

### Cambios en domain

#### `FileData` se vuelve estructura con enum `FileSource`

```rust
// llm/domain/llm_message.rs

pub struct FileData {
    pub document_id: Option<String>,    // del campo `id` del emisor
    pub mime_type: String,
    pub filename: String,
    pub size_hint: Option<u64>,         // del `size_bytes` del emisor
    pub source: FileSource,
}

pub enum FileSource {
    /// Bytes ya en RAM (vino como `data` base64 < 30 MB, o `path` < 30 MB).
    InlineBytes(Vec<u8>),

    /// Signed URL pendiente de descarga + upload al provider.
    /// document_id requerido para cache hit.
    SignedUrl(String),

    /// Ya subido al provider en una iteración previa de la ejecución
    /// o reutilizado de la cache persistida.
    Uploaded(ProviderFileRef),
}

pub struct ProviderFileRef {
    pub provider: LlmProvider,
    pub provider_file_id: String,        // file_uri (Gemini) o file_id (Anthropic/OpenAI)
    pub mime_type: String,
    pub filename: String,
    pub expires_at: Option<DateTime<Utc>>,
}
```

#### Nuevo puerto: `FileProviderRepository`

```rust
// llm/domain/file_provider_repository.rs

#[async_trait]
pub trait FileProviderRepository: Send + Sync {
    /// Sube un archivo desde un stream al Files API del proveedor.
    /// El stream es consumido (puede provenir de un download HTTP).
    async fn upload_streaming(
        &self,
        stream: BoxedByteStream,
        mime_type: &str,
        filename: &str,
    ) -> Result<ProviderFileRef, LlmError>;

    /// Devuelve cuándo expira un archivo en este proveedor.
    /// None = no expira (Anthropic, OpenAI).
    /// Some(duration) = expira en X tiempo desde el upload (Gemini = 48h).
    fn ttl(&self) -> Option<Duration>;

    /// Identificador del proveedor para keying en cache.
    fn provider(&self) -> LlmProvider;
}

pub type BoxedByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;
```

#### Nuevo puerto: `FileCacheRepository`

```rust
// llm/domain/file_cache_repository.rs

#[async_trait]
pub trait FileCacheRepository: Send + Sync {
    async fn lookup(
        &self,
        document_id: &str,
        provider: LlmProvider,
    ) -> Result<Option<CachedFileEntry>, LlmError>;

    async fn upsert(&self, entry: &CachedFileEntry) -> Result<(), LlmError>;

    async fn invalidate(
        &self,
        document_id: &str,
        provider: LlmProvider,
    ) -> Result<(), LlmError>;
}

pub struct CachedFileEntry {
    pub document_id: String,
    pub provider: LlmProvider,
    pub provider_file_id: String,
    pub mime_type: String,
    pub filename: String,
    pub size_bytes: Option<i64>,
    pub uploaded_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: DateTime<Utc>,
}

impl CachedFileEntry {
    /// Heurística: si tenemos expires_at y ya pasó (con margen de 5 min),
    /// asumimos expirado sin llamar al provider.
    pub fn is_likely_alive(&self, now: DateTime<Utc>) -> bool {
        match self.expires_at {
            None => true,                                       // no expira
            Some(exp) => now < exp - Duration::minutes(5),      // margen safety
        }
    }
}
```

### Cambios en application

#### `LlmCallUseCase` — flujo extendido

Pseudocódigo del nuevo flujo de resolución de archivos, ejecutado **antes** de pasar al `LlmRepository::call`:

```
para cada FileData en messages.files:
    match source:
        InlineBytes(bytes):
            // Sin cambios. Pasa al adapter como hoy.
            continue

        SignedUrl(url):
            si document_id es None:
                error "id requerido cuando se usa url"

            // 1. Lookup en cache
            cached = file_cache_repo.lookup(document_id, provider)?
            si cached y cached.is_likely_alive(now()):
                source = Uploaded(cached.into_ref())
                continue

            si cached:
                // expirado: invalidar y caer al re-upload
                file_cache_repo.invalidate(document_id, provider)?

            // 2. Pipe download → upload
            stream = signed_url_downloader.stream(url).await?
            file_ref = file_provider_repo.upload_streaming(
                stream, mime_type, filename
            ).await?

            // 3. Persistir
            file_cache_repo.upsert(&CachedFileEntry::from_ref(
                document_id, file_ref, now()
            ))?

            source = Uploaded(file_ref)

        Uploaded(_):
            continue  // ya resuelto en una iteración previa
```

#### Retry en LLM call con error "file not found"

```
loop {
    match llm_repo.call(request).await {
        Err(FileNotFound(provider_file_id)) si retries == 0 => {
            // Invalidar cache para todos los archivos uploaded del request
            // que puedan tener este id, re-resolver SignedUrl, retry.
            invalidate_uploaded_with_id(provider_file_id)?
            re_resolve_signed_urls(request)?
            retries += 1
            continue
        }
        result => return result
    }
}
```

Solo 1 retry duro, sin backoff. Si el segundo intento falla, propagamos el error original.

#### Dedup intra-ejecución

Durante la materialización de un mismo request LLM, si dos `FileData` distintas comparten `document_id`, hacer un único upload y reutilizar el `ProviderFileRef`. Map en memoria del use case: `HashMap<String, ProviderFileRef>` con scope `&mut self` del use case por invocación.

Razón: el hint del informe del emisor ("Mismo array en múltiples nodos") aplica también dentro de un mismo request si el grafo replica archivos entre nodos `llm_call`.

### Cambios en infrastructure

#### `SignedUrlDownloader`

```rust
// llm/infrastructure/files/signed_url_downloader.rs

pub struct SignedUrlDownloader {
    client: reqwest::Client,
}

impl SignedUrlDownloader {
    pub async fn stream(&self, url: &str)
        -> Result<BoxedByteStream, LlmError>
    {
        let response = self.client.get(url).send().await?;

        // Validar 2xx. No mandar Authorization header — la firma va en query params.
        if !response.status().is_success() {
            return Err(LlmError::SignedUrlFetchFailed(response.status()));
        }

        Ok(Box::pin(
            response.bytes_stream().map_err(|e| std::io::Error::other(e))
        ))
    }
}
```

Sin retry interno: la responsabilidad de retry vive en el use case (ver retry on 404 más arriba).

#### `GeminiFilesApiAdapter` — resumable upload protocol

Tres fases:

1. **Iniciar sesión**:
   ```
   POST https://generativelanguage.googleapis.com/upload/v1beta/files
   Headers:
     X-Goog-Upload-Protocol: resumable
     X-Goog-Upload-Command: start
     X-Goog-Upload-Header-Content-Type: <mime_type>
     X-Goog-Upload-Header-Content-Length: <size si conocido, sino omitir>
     Authorization: Bearer <api_key>
   Body: { "file": { "display_name": "<filename>" } }
   ```
   Respuesta incluye header `X-Goog-Upload-URL` con la URL única de carga.

2. **Subir bytes** (chunks de 8 MB recomendado):
   ```
   PUT <upload_url>
   Headers:
     Content-Length: <chunk_size>
     X-Goog-Upload-Offset: <offset>
     X-Goog-Upload-Command: upload   (o "upload, finalize" en el último)
   Body: <chunk_bytes>
   ```
   Los chunks vienen del `BoxedByteStream`. Acumulamos hasta `chunk_size` antes de hacer PUT.

3. **Obtener `file_uri`**: el último PUT (con command `upload, finalize`) devuelve el objeto File con `name: "files/abc123"`. Construir `file_uri = format!("https://generativelanguage.googleapis.com/v1beta/files/{name}")`.

`ttl()` devuelve `Some(Duration::hours(48))`.

Referenciar en `generateContent`:
```json
{
  "contents": [{
    "parts": [
      { "file_data": { "mime_type": "...", "file_uri": "..." } },
      { "text": "..." }
    ]
  }]
}
```

#### `AnthropicFilesApiAdapter`

```
POST https://api.anthropic.com/v1/files
Headers:
  x-api-key: <api_key>
  anthropic-version: 2023-06-01
  anthropic-beta: files-api-2025-04-14
Body: multipart/form-data con `file` part desde el stream
```

Stream-to-stream con `reqwest::multipart::Part::stream(Body::wrap_stream(...))`.

`ttl()` devuelve `None` (no expira).

Referenciar en `messages`:
```json
{
  "role": "user",
  "content": [
    { "type": "document", "source": { "type": "file", "file_id": "<id>" } },
    { "type": "text", "text": "..." }
  ]
}
```

#### `OpenAiFilesApiAdapter`

```
POST https://api.openai.com/v1/files
Headers:
  Authorization: Bearer <api_key>
Body: multipart/form-data
  - purpose: "user_data"
  - file: <stream>
```

`ttl()` devuelve `None` (no expira).

Referenciar en `responses` / `chat.completions`:
```json
{
  "role": "user",
  "content": [
    { "type": "input_file", "file_id": "<id>" },
    { "type": "input_text", "text": "..." }
  ]
}
```

#### `PostgresFileCache`

Migración SQL (nueva en `src/libs/colmena/migrations/`):

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

CREATE INDEX idx_provider_file_cache_expires
    ON provider_file_cache (expires_at)
    WHERE expires_at IS NOT NULL;
```

Implementación con `sqlx`. Conexión: **siempre lee `DATABASE_URL` desde env** (el mismo que usa el resto del DAG runtime para metadatos de nodos). El cache de archivos es transversal y obligatorio independiente de si los nodos de `llm_call` configuran o no memoria de conversación.

**Reuso de código**: extraer la lógica de creación de pool Postgres del `RepositoryFactory` actual (`llm/infrastructure/persistence/repository_factory.rs`) a un helper compartido `pg_pool::get_or_create(url) -> PgPool` que cachee pools por URL. Tanto `memory_postgres` como `PostgresFileCache` lo consumen. Sin duplicación.

`upsert`:
```sql
INSERT INTO provider_file_cache
    (document_id, provider, provider_file_id, mime_type, filename,
     size_bytes, uploaded_at, expires_at, last_used_at)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
ON CONFLICT (document_id, provider) DO UPDATE SET
    provider_file_id = EXCLUDED.provider_file_id,
    uploaded_at      = EXCLUDED.uploaded_at,
    expires_at       = EXCLUDED.expires_at,
    last_used_at     = NOW();
```

Race condition cross-execution con mismo `document_id`: dos requests simultáneos hacen el upload duplicado al provider; el segundo gana en el UPDATE de la cache; el primer archivo subido al provider queda huérfano. Aceptable porque (a) es un caso extremadamente raro en producción según el dominio, (b) los providers tienen sus propios límites de almacenamiento.

### Cambios en el nodo `llm_call`

`src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` líneas 482-547 (parser de `files`) cambia para aceptar el nuevo schema:

```
para cada file_obj en files_arr:
    document_id = file_obj.id           (opcional pero requerido si url)
    mime_type, filename, size_bytes      (igual que hoy)

    si file_obj.data presente:
        si size_bytes > 30 MB:
            error "data field exceeds 30 MB limit, emitter must use url"
        decodificar base64 → InlineBytes

    si file_obj.url presente:
        si document_id ausente:
            error "id required when url is provided"
        construir SignedUrl(url) con document_id en FileData

    si file_obj.path presente (legacy):
        tamaño_disco = fs::metadata(path).len()
        si tamaño_disco > 30 MB:
            error "path file exceeds 30 MB limit, use url for large files"
        leer bytes → InlineBytes
```

Resiliencia por archivo: si el parsing/materialización de un archivo falla, log warning y continuar con los demás. Solo abortamos si **todos** los archivos fallan o si la lista era no-vacía y queda vacía tras los errores.

### Wiring — dos factories separadas

Mantenemos `LlmProviderFactory` actual sin cambios (sigue devolviendo `Arc<dyn LlmRepository>`). Agregamos una factory hermana, separada por mantenibilidad:

```rust
// llm/infrastructure/files/file_provider_factory.rs
pub struct FileProviderFactory;

impl FileProviderFactory {
    pub fn create(kind: ProviderKind) -> Arc<dyn FileProviderRepository> {
        match kind {
            ProviderKind::Gemini    => Arc::new(GeminiFilesApiAdapter::new(...)),
            ProviderKind::Anthropic => Arc::new(AnthropicFilesApiAdapter::new(...)),
            ProviderKind::OpenAI    => Arc::new(OpenAiFilesApiAdapter::new(...)),
        }
    }
}
```

Ventajas:

- Sin cambios breaking en callers de `LlmProviderFactory`.
- Cada factory evoluciona independientemente.
- Tests pueden mockear `FileProviderRepository` sin tocar el `LlmRepository`.

El `LlmCallUseCase` recibe ambas factories vía constructor (o las invoca una vez al inicio según el `ProviderKind` del nodo). El `FileCacheRepository` se inyecta una sola vez (transversal, no per-provider).

## Manejo de errores

Errores nuevos en `LlmError`:

```rust
pub enum LlmError {
    // ... existentes
    DataFieldTooLarge { size: u64 },
    PathFieldTooLarge { size: u64 },
    UrlWithoutDocumentId,
    SignedUrlFetchFailed(StatusCode),
    FileApiUploadFailed { provider: String, source: BoxError },
    ProviderFileNotFound(String),       // 404 del provider durante generateContent
    DocumentIdRequired,                  // url sin id
}
```

Política:

- Errores por archivo individual durante materialización → log warning + continuar con los demás.
- `FileApiUploadFailed` durante el upload → fallar ese archivo, loggear, continuar (igual que arriba).
- `ProviderFileNotFound` durante el LLM call → trigger del retry duro (ver Application).
- Si tras todos los errores el array de archivos resueltos queda vacío y la lista original era no-vacía → fallar la llamada con `LlmError::AllFilesFailedToResolve`.

## Configuración

Sin nuevos flags en el JSON del nodo (consistente con la decisión de "no override en egreso"). Solo el comportamiento del cache es transparente.

`DATABASE_URL` ya existe en `.env`. Se reutiliza directamente.

## Testing

### Unit tests

- `FileData::is_likely_alive` con varios escenarios de `expires_at`.
- Parser del nodo: aceptar/rechazar payloads válidos/inválidos.
- `PostgresFileCache::lookup/upsert/invalidate` con DB de test (transaction rollback pattern existente).
- `SignedUrlDownloader::stream` con un mock HTTP server (wiremock o equivalente).
- Cada `*FilesApiAdapter` con mock HTTP que simule respuestas de cada provider.

### Integration tests (Rust)

- `tests/files_api_integration.rs` (gated por env var con keys reales): subir archivo de prueba pequeño a cada provider, verificar `provider_file_id` retornado, ejecutar generación referenciando el id, validar que la respuesta contiene info del archivo.

### Test graphs (JSON)

Nuevos en `tests/graphs/media/`:

- `pdf_url_anthropic.json` — usa `url` (signed URL pre-generada) + Anthropic.
- `pdf_url_gemini.json` — idem Gemini.
- `pdf_url_openai.json` — idem OpenAI.

Fixture: subir un PDF de prueba a un bucket GCS conocido, generar URL firmada con `gsutil signurl`, embeber en el JSON. **Regeneración manual** cuando expire la URL (6 h TTL). No automatizamos firma on-the-fly en esta iteración — los tests son ad-hoc, ejecutados manualmente cuando se valida el flujo completo. Documentar en un README dentro de `tests/graphs/media/` los pasos exactos para regenerar.

### Tests de cache

- Lookup hit alive → no descarga, no upload.
- Lookup hit expired → invalidate + re-upload.
- Lookup miss → download + upload + insert.
- Retry on 404 → invalidate + re-upload + retry succeeds.

## Migración y backward compatibility

- Schema actual `{mime_type, filename, data | path}` sigue funcionando idéntico (sin `id`, sin `url`).
- Adición de `id` y `url` es aditiva. Nodos que solo usan `data` no se ven afectados.
- Los fixtures actuales en `tests/graphs/media/` siguen pasando.
- Adición del campo `document_id` en `FileData` es `Option<String>` para no romper construcciones existentes.

## Riesgos conocidos

1. **Pipe stream y retries**: si el upload al provider falla a mitad de stream, no podemos rebobinar; hay que re-descargar de GCS. Las signed URLs duran 6 h, así que en la práctica tenemos margen. El retry on 404 lo asume.

2. **Race condition cross-execution**: dos requests simultáneos con mismo `document_id` y cache miss → upload duplicado → un archivo huérfano. Aceptable según contexto (raro en prod).

3. **Gemini 48 h**: si una sesión tiene gaps mayores a 48 h, el cache hit triggereará un retry on 404. Adicional 1 round trip cada vez, pero auto-corrige.

4. **Storage limits del provider**: Anthropic 500 GB org, OpenAI varía por tier. Sin janitor, esto puede acumularse. Aceptable para primera iteración; si se vuelve problema → follow-up issue para janitor.

5. **Backpressure mid-pipe**: si el provider acepta bytes más lento que GCS los manda, TCP flow control sincroniza automáticamente. No debería ser problema, pero hay que verificar empíricamente con archivos de 200 MB.

## Plan de implementación (de alto nivel)

Detallado se desarrollará en writing-plans. Resumen:

1. Schema y `FileData` enum + nuevos errores en domain.
2. Parser del nodo `llm_call` extendido (acepta `id`/`url`, valida `data`/`path` tamaño).
3. `FileCacheRepository` puerto + `PostgresFileCache` adapter + migración.
4. `FileProviderRepository` puerto + `SignedUrlDownloader`.
5. Adapters Files API: Anthropic primero (más simple, multipart), luego OpenAI (similar), luego Gemini (resumable).
6. `LlmCallUseCase` extendido con resolución, dedup intra-ejecución, retry on 404.
7. Modificación de cada `*_adapter.rs` para aceptar `ProviderFileRef` además de bytes.
8. Test graphs, integration tests.
9. Documentación: `docs/developer_guide/` nueva sección sobre archivos grandes.

## Decisiones clave (resumen)

| # | Decisión | Justificación |
|---|----------|---------------|
| 1 | Tres providers en una sola pasada de diseño | Solicitado, hexagonal lo facilita |
| 2 | Sin auto-threshold en egreso | El emisor ya decidió a 30 MB |
| 3 | Cache persistido en PostgreSQL por `(document_id, provider)` | Stateless por DAG, sesiones cruzan ejecuciones |
| 4 | Cache siempre en `DATABASE_URL` (env), no per-node | Cache transversal independiente de si los nodos configuran memoria |
| 4b | Dos factories separadas (`LlmProviderFactory` y `FileProviderFactory`) | Mantenibilidad y cero breaking changes en callers |
| 5 | Streaming pipe end-to-end | RAM/disco constantes bajo alta concurrencia |
| 6 | Validación heurística por `expires_at` + retry on 404 | Cero round-trips en happy path, recovery automático |
| 7 | `data > 30 MB` y `path > 30 MB` → error de contrato | Confianza en la decisión del emisor |
| 8 | `ON CONFLICT DO UPDATE` simple para race | Caso casi inexistente en producción |
| 9 | 1 retry duro sin backoff en file-not-found | Suficiente para auto-recovery |
| 10 | Janitor de archivos huérfanos out of scope | YAGNI hasta ver volumen real |
