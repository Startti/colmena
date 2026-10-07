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
- **`storage_key`** (solo con `COLMENA_LARGE_TABULAR` activo): una entrada que trae únicamente `storage_key` (sin `data`, `url` ni `path`), de tipo `text/csv` o xlsx y con `size_bytes` **estrictamente mayor** a 50 MiB, se convierte en `FileSource::StorageRef(key)`: no se descarga, no se sube al proveedor y no se resume. Cualquier otra entrada con solo `storage_key` se omite en silencio, como siempre. Ver «Archivos tabulares grandes» más abajo.
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

## Archivos tabulares grandes (`COLMENA_LARGE_TABULAR`)

La ruta de archivos tabulares grandes (interruptor encendido) no lee el archivo entero en memoria; esta primera pieza solo fija cuál archivo cuenta como grande. «Grande» tiene una sola definición, `llm::domain::large_tabular::is_large_tabular(mime, tamaño, interruptor)`: el interruptor está encendido, el mime es `text/csv` o xlsx (sin distinguir mayúsculas ni parámetros; el `.xls` antiguo no se acepta) y `size_bytes` es conocido y **estrictamente mayor** a 52 428 800 (50 MiB). Exactamente 50 MiB es pequeño, un tamaño ausente es pequeño y, con el interruptor apagado, nada es grande a ningún tamaño. `refusal_text()` es el texto que una herramienta que no puede leer un archivo grande entero le da al modelo; hoy dice lo que es verdad («demasiado grande para esta herramienta, no se puede analizar en este turno») y el texto futuro («usa `attachment_run_python` con `tables`») queda detrás de `LARGE_FILE_TOOL_AVAILABLE`, que cambia a `true` la unidad que entregue esa herramienta. Todo rechazo lleva el código `large_tabular_file`. `validate_storage_key` es la validación barata de una llave del host (no vacía, hasta 1024 caracteres, sin caracteres de control ni segmentos `..`).

Un archivo así viaja como `FileSource::StorageRef(key)`: ya está en el almacenamiento del host bajo `key` y nada necesita moverse.

- **Resolución.** `LlmCallUseCase::resolve_files` deja intacto un `StorageRef`: sin descarga, sin subida al proveedor, sin consulta a la caché, y un turno cuyo único archivo es una referencia no termina en `AllFilesFailedToResolve`. Una referencia tampoco cuenta como archivo resuelto: si todos los archivos que sí necesitaban resolverse fallan, el turno sigue reportando `AllFilesFailedToResolve` aunque viajen referencias.
- **Registro.** El paso 3 del nodo registra un `StorageRef` en el catálogo (`conversation_attachments`) con `provider_file_id` vacío, `storage_key` igual a la llave que mandó el host, fuente `AttachmentSource::Path(key)`, tamaño, mime, nombre y etiqueta de la entrada, y origen `host_storage_ref`. No se guarda ninguna copia ni se lee el objeto, y no se encola resumen automático: resumir lee la fuente de vuelta, y una fuente `Path` se lee del disco local en modo local, así que una llave nunca debe llegar ahí (`summary_target`). El modelo lo ve en el catálogo del mensaje de sistema. Si la llamada no puede registrarlo, no se descarta en silencio: sin sesión de agente o sin registro de adjuntos, con `attachments_enabled: false` (no hay catálogo, la referencia sería invisible), o porque ese `id` ya está registrado en la otra clase para el mismo proveedor (`UpsertOutcome::OwnershipConflict`, la fila existente queda intacta), el nodo agrega un aviso «Attachments not delivered» (hasta 10) y un log WARN siempre activo. Un límite conocido: en el segundo caso, si el archivo ordinario ya trajo una copia que el motor alcanzó a guardar antes de que el registro rechazara la escritura, esa copia queda sin fila.
- **Propiedad del objeto.** Una fila de catálogo que apunta a un objeto del host (la llave es la del host, no una copia que guardó el motor) lleva `origin = host_storage_ref` (`origin::HOST_STORAGE_REF`). No hay columna nueva ni migración: `origin` ya existe y sobrevive reinicios. `origin::is_user_supplied` mantiene visibles entre proveedores tanto las subidas como las referencias.
- **La clase de propiedad de una fila es inmutable.** `upsert` hacía conflicto por `(sesión, documento, proveedor)` y mezclaba `origin` y `storage_key` por separado (`COALESCE`), así que re-registrar la misma fila en la otra clase habría dejado la llave del host bajo un origen de subida (el GC borraría el objeto del usuario) o habría perdido la copia del motor detrás de la llave del host. Ahora la escritura misma lo impide, de forma atómica y en ambos motores: la rama `DO UPDATE` lleva `WHERE (COALESCE(fila.origin,'') = 'host_storage_ref') = (COALESCE(nuevo.origin,'') = 'host_storage_ref')`. Una escritura de la otra clase no cambia nada (ni `origin`, ni `storage_key`, ni `provider_file_id`, ni `refreshed_at`) y `upsert_checked` devuelve `UpsertOutcome::OwnershipConflict`; `upsert` conserva su firma y solo ignora la escritura. Las escrituras de la misma clase se comportan igual que siempre (los `COALESCE` de `label`, `description`, `storage_key` y `origin` no cambian). Filas de otro proveedor para el mismo documento son independientes.
- **Limpieza (`attachment_gc`).** El GC es el único camino del motor que borra el blob de una fila. Para una fila `host_storage_ref` borra la fila y no llama a `storage.delete`. Juzga y borra **fila por fila** (`delete_attachment_for_provider`, clave sesión + documento + proveedor; el método sin proveedor sigue existiendo): borrar por `(sesión, documento)` arrastraba la fila de otro proveedor y dejaba huérfana su copia. `total_host_references_released` cuenta solo cuando la fila ya se borró y sale en `gc.batch.end`; `--dry-run` cuenta `would_release_host_references` y `would_delete` y para una referencia del host no imprime la llave. Despliega el GC nuevo **antes** de encender el interruptor del motor (un GC anterior borraría el objeto del usuario); ver la nota de migración. Otros caminos que tocan filas, sin borrar blobs: `upsert`, `delete_attachment`, el manejo de sesiones y documentos; la limpieza de `tabular_prepare` borra copias derivadas del motor, nunca el objeto fuente.
- **`load_attachment` nunca lee entera una fila de referencia.** `AttachmentResolverImpl` rechaza una fila `host_storage_ref` **antes** de leer, subir o inlinear su objeto, con la fila que *ya* consultó para el proveedor actual y justo donde la iba a usar (también la rama `Generated` y la subida perezosa entre proveedores): la fila juzgada es, por construcción, la fila leída. No hay una segunda consulta al registro (una versión previa consultaba por documento en cualquier proveedor y podía juzgar otra fila que la que leía el resolvedor). La decisión depende de la **naturaleza de la fila**, no del interruptor ni del tamaño o el mime: tras un rollback, o con una fila sin `size_bytes`, el rechazo es el mismo. Para cualquier otra fila no cambia nada observable: mismas consultas (una por proveedor y un `touch`), mismos errores y mismo texto (`attachment_expired_unrecoverable` con el cuerpo de siempre, incluida una falla del registro), con el interruptor encendido o apagado. Lo único nuevo es el rechazo.
- **Qué pasa con una copia del motor mayor de 50 MiB (decisión).** Hoy, un CSV/xlsx de 50-100 MiB que llegó por `url` se sube al proveedor y el motor guarda una copia; `load_attachment` devuelve el archivo subido (sin leer el objeto) y las herramientas lo leen entero (como máximo 100 MiB, el tope de subida) antes de que el tope de 50 MiB sobre los bytes reales responda con su error de siempre. Eso es acotado, no un riesgo de memoria que este diseño quiera cerrar (apunta a archivos de 400 MB-1 GB), así que **se mantiene tal cual**, con el interruptor encendido o apagado. Solo las filas de referencia al objeto del host se rechazan.
- **Texto y código del rechazo.** El rechazo de `load_attachment` es `{"error":"large_tabular_file","document_id":…,"reason":…}`; el mismo código llega a las demás herramientas en el siguiente cambio. Los demás fallos de `load_attachment` conservan su cuerpo byte a byte.
- **Las herramientas que leen un adjunto entero.** `DagToolExecutor::fetch_attachment_bytes` es el punto compartido de `sql_inspect_attachment`, `sql_bulk_insert_from_attachment`, `attachment_run_python`, `data_run_python` y los importadores de gsheets y gdocs. Rechaza una fila `host_storage_ref` **antes** de tocar el almacenamiento, con el criterio de `load_attachment` (la naturaleza de la fila; sin interruptor, tamaño ni mime), usando la fila que la misma búsqueda de llave ya devolvió: **no hay una consulta más** (una por cada fila que no está en el catálogo del turno, como antes, más el `touch`), y una falla del registro conserva su texto de siempre (`attachment registry lookup failed: …`). El texto y el código `large_tabular_file` salen igual en `sql_inspect_attachment`, `sql_bulk_insert_from_attachment`, `attachment_run_python`, `data_run_python` (error de binding), gsheets `create_from_xlsx`, gdocs `create_from_docx` e `insert_image` por adjunto (este último sin prueba propia: usa el mismo ayudante `tag_refusal`).
- **Lecturas por flujo, por URL y en memoria del adjunto de una referencia del host.** Una lectura por **flujo** (`$attachment:` en un multipart, `fetch_attachment_stream`) y la URL de lectura firmada siguen permitidas, porque no cargan el objeto entero (el límite lo pone el consumidor: el multipart envía chunk a chunk; la URL es solo un enlace con TTL). Los consumidores que **sí** lo sostienen entero (el cuerpo JSON de `http_request`, que lo incluye como `data:` base64 hasta su tope por defecto de 100 MiB, e `image_edit`) pasan por `resolve_for_buffering` y rechazan una referencia del host con el mismo texto de rechazo (`AttachmentResolveError::HostObject`). Los errores de almacenamiento llevan la llave (y un adaptador local, una ruta del disco) y llegan al modelo: para una referencia del host, tanto al abrir el flujo como a mitad de flujo (`resolve`, `resolve_url`, `fetch_attachment_stream`, los chunks de `read_session_attachment`) el error nombra el documento y el archivo, como texto inerte, nunca la llave ni una ruta. Las demás filas conservan su error.
- **Adaptadores.** Ningún adaptador de proveedor acepta la variante. Los cuatro conversores (OpenAI chat y responses, Anthropic, Gemini) devuelven `InternalError` con el nombre del archivo, nunca su llave, en vez de enviarlo o descartarlo; el nodo nunca pone un archivo así en un mensaje.
- **Lectura de `files[]`.** `parse_file_entries_with(entradas, local_mode, large_tabular)` convierte una entrada que trae solo `storage_key` y es grande en un `StorageRef` (id, mime, filename y tamaño se conservan; el id no es obligatorio). Tiene la prioridad más baja: una entrada que además trae `data`, `url` o `path` se lee como siempre. Cualquier otra entrada con solo `storage_key` (interruptor apagado, otro tipo, tamaño ausente o de 50 MiB o menos) no se registra como archivo grande y las demás conservan su índice; con el interruptor encendido el motivo concreto va al log y al modelo («Attachments not delivered» en el sufijo del mensaje de sistema, hasta 10 avisos, sin la llave). `parse_file_entries` es la llamada con el interruptor apagado.
- **Interruptor.** `ColmenaEngine::new` entrega `EngineConfig.prepare.large_tabular` al registro de nodos (`set_large_tabular`), que lo comparte con `llm_call`; el nodo lo lee una vez por llamada. Un nodo construido fuera de un motor lo tiene apagado.

Pruebas: `cargo test --bin attachment_gc` (y `-- --ignored` con `DATABASE_URL` para Postgres) cubre la limpieza; `cargo test --lib ownership_contract` cubre la regla en SQLite y, con `DATABASE_URL=… cargo test --lib ownership_contract -- --ignored`, en Postgres; `cargo test --lib large_tabular` cubre la definición en su frontera, `cargo test --lib storage_key_entries` y `cargo test --lib attachment_notices` la lectura de `files[]` y sus avisos, `cargo test --lib large_files` el paso directo en la resolución y la fuente del registro, y `cargo test --lib refuses_a_storage_ref` los rechazos de los adaptadores. Para el resto: `cargo test --lib load_attachment_redirect load_failure` (`load_attachment`, dos proveedores, conteo de consultas), `cargo test --lib large_object_guard` (guarda del ejecutor, código en cada herramienta, errores sin llave de `fetch_attachment_stream`), `cargo test --lib stream_resolver session_attachment` (resolvedor y rechazo en memoria), `cargo test --lib storage_ref_turn` (registro, clase inmutable, `attachments_enabled: false`, avisos acotados), `cargo test --lib whole_read_guard` y, con Postgres, `DATABASE_URL=… SECURE_VALUES_KEY=… cargo test --test large_tabular_turn -- --ignored`.

## Estrategia por provider

| Provider  | Imagen | PDF |
|-----------|--------|-----|
| **Anthropic** | URL passthrough (Anthropic baja la URL) | Files API + `file_id` (header beta `files-api-2025-04-14` requerido también en `/v1/messages`) |
| **OpenAI**    | URL passthrough en chat completions (`image_url.url`) | Files API + `file_id` en Responses API (omitir `filename`) |
| **Gemini**    | Files API + `fileData.fileUri` | Files API resumable upload (chunks de 8 MB exactos) |

Detalles importantes:

- **Anthropic** rechaza `{"type": "file", "file_id": "..."}` para `image.source` — solo acepta `base64` o `url`. Para PDFs sí lo acepta, pero **requiere** el header `anthropic-beta: files-api-2025-04-14` también en la llamada de generación.
- **OpenAI** Chat Completions requiere `image_url.url`; `file_id` para imágenes solo funciona vía Responses API. Y en Responses, `file_id` y `filename` son **mutuamente excluyentes**.
- **Imagen por URL con Anthropic u OpenAI:** queda como URL, sin `file_id`. El Step 3 del
  `llm_call` baja la URL, guarda los bytes y la registra en el catálogo con `provider_file_id`
  vacío: `load_attachment` la sirve desde storage (base64) y `$attachment:<document_id>` reenvía
  los bytes. Hasta CHANGELOG 2026-09 §141, con `DATABASE_URL` no se registraba.
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
- `pdf_url_anthropic.json` / `pdf_url_openai.json` / `pdf_url_gemini.json` — PDFs de ~12 MB (`size_bytes: 12582912`) vía signed URL (path Files API en los 3; el `url` se usa por elección del test, no porque el archivo supere el threshold de 30 MB).

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
