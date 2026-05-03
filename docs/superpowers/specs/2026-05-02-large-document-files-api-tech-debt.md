# Deuda técnica: Large Document Files API

**Fecha**: 2026-05-02
**Feature ref**: `docs/superpowers/specs/2026-05-02-large-document-files-api-design.md`
**Estado**: feature mergeado como alpha/beta; los items de aquí son follow-ups conocidos.

Esta tabla agrupa el trabajo pendiente y la deuda registrada durante la implementación y el testing real del feature de archivos grandes.

---

## Deuda priorizada

| # | Item | Severidad | Bloqueante? | Estimación |
|---|------|-----------|-------------|-------------|
| 1 | ~~Retry on `ProviderFileNotFound` no recupera (no-op)~~ ✅ Resuelto | Alta | No (best-effort documentado) | 1-2 días |
| 2 | ~~`last_used_at` no se actualiza en cache hit~~ ✅ Resuelto | Baja | No | 30 min |
| 3 | ~~Layer leak: `LlmCallUseCase` importa de `infrastructure`~~ ✅ Resuelto | Media | No (compila y funciona) | 1 día |
| 4+5 | Huérfanos (cache + provider) — análisis profundo hecho, implementación pendiente | Media | No | 3-5 días |
| 6 | ~~`ProviderKind::Mock` fallback silencioso en `PostgresFileCache::lookup`~~ ✅ Resuelto | Baja | No | 30 min |
| 7 | ~~Error de mime malformado clasificado como `FileApiUploadFailed`~~ ✅ Resuelto | Baja | No | 1 hora |
| 8 | Tests de integración E2E reproducibles (sin signed URLs) | Media | No | 2 días |
| 9 | Métricas/observabilidad (Prometheus) para cache hit-rate | Baja | No | 1 día |
| 10 | Cache hit cross-session por `sha256` (sin coordinación con emisor) | Muy baja | No (YAGNI hasta ver volumen) | 3-5 días |

---

## 1. Retry on `ProviderFileNotFound` no recupera ✅ RESUELTO

**Ubicación**: `src/libs/colmena/src/llm/application/llm_call_use_case.rs::{snapshot_signed_urls, reset_uploaded_files_with_id}`.

### Síntoma original

Cuando el LLM fallaba con `ProviderFileNotFound { provider_file_id }` (archivo borrado por TTL Gemini agotado o cleanup manual), el flujo de retry:

1. ✅ Invalidaba la fila correspondiente del cache.
2. ❌ **NO** convertía `FileSource::Uploaded(ref)` de vuelta a `FileSource::SignedUrl(url)`.
3. Re-ejecutaba `resolve_files_in_messages`, pero veía `Uploaded` y hacía short-circuit (ya estaba resuelto), no re-subía.
4. Re-llamaba al LLM con el mismo `file_id` muerto → mismo error.

### Solución implementada

Snapshot de las URLs originales antes de la primera resolución, indexado por `document_id`:

```rust
// En execute(), antes de resolve_files_in_messages:
let url_snapshot = Self::snapshot_signed_urls(&messages);
```

En el manejo del 404 se invoca el reset con el snapshot:

```rust
Self::reset_uploaded_files_with_id(&mut messages, &provider_file_id, &url_snapshot);
```

`reset_uploaded_files_with_id` recorre los mensajes y, para cada `Uploaded` con `provider_file_id` igual al perdido, busca el `document_id` en el snapshot y lo reescribe como `SignedUrl(url_original)`. Si el archivo llegó originalmente como `Uploaded` directo (sin URL en el snapshot), se mantiene tal cual — best-effort para ese caso.

### Tests

- Unit tests en `snapshot_and_reset_tests` (módulo nuevo) cubren las funciones puras: snapshot toma SignedUrl pero ignora Uploaded/Inline; reset reescribe match exacto; reset es no-op cuando no hay match en snapshot, cuando el id no coincide o cuando falta document_id.
- `retry_tests::retries_redownload_after_provider_file_not_found` levanta un `wiremock::MockServer`, fuerza un cache HIT con el `file_id` "lost", y verifica que tras el 404 se hace exactamente 1 GET a la URL durante el retry. Antes del fix esto era 0 GETs (el reset era no-op).

---

## 2. `last_used_at` no se actualiza en cache hit ✅ RESUELTO

**Ubicación**: `src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs::lookup`.

### Síntoma original

`last_used_at` solo se escribía en `INSERT` y `ON CONFLICT DO UPDATE`. En cache hit nunca se hacía `UPDATE`. Las filas conservaban `last_used_at = uploaded_at` aunque fueran accedidas miles de veces — rompía métricas de "qué archivos están activos" y cualquier futuro janitor LRU.

### Solución implementada

`lookup` ahora ejecuta un `UPDATE ... RETURNING *` en lugar del `SELECT`. Una sola query, mismo resultado lógico, ahora la columna refleja la realidad.

```sql
UPDATE provider_file_cache
SET last_used_at = NOW()
WHERE document_id = $1 AND provider = $2
RETURNING document_id, provider, provider_file_id, mime_type, filename,
          size_bytes, uploaded_at, expires_at, last_used_at;
```

Si la fila no existe, `UPDATE` devuelve 0 rows → mismo resultado que un SELECT MISS.

Trade-off: el `UPDATE` toma row-lock en lugar de share-lock, pero la concurrencia por `(document_id, provider)` es baja (un mismo doc lo procesa un solo request a la vez), no causa contención observable.

### Test

`lookup_advances_last_used_at_on_cache_hit`: insertar fila, dormir 50 ms, hacer lookup, verificar `got.last_used_at > original_last_used` y `got.last_used_at > got.uploaded_at` (esto último confirma que el UPDATE no toca `uploaded_at` por error).

---

## 3. Layer leak: `LlmCallUseCase` importa de `infrastructure` ✅ RESUELTO

**Ubicación**: `src/libs/colmena/src/llm/application/llm_call_use_case.rs`.

### Síntoma original

```rust
use crate::llm::infrastructure::files::{FileProviderFactory, SignedUrlDownloader};
```

El use case en la capa de aplicación importaba dos tipos concretos de `infrastructure/`. Violaba la regla hexagonal del proyecto (`CLAUDE.md`: "Domain layer has ZERO infrastructure dependencies"; capas superiores tampoco deben depender de adapters concretos).

### Solución implementada

1. **Dos puertos nuevos en `domain/`**:
   - `SignedUrlFetcher` — `async fn stream(&self, url: &str) -> Result<BoxedByteStream, LlmError>`.
   - `FileProviderFactoryPort` — `fn build(&self, kind, api_key) -> Result<Arc<dyn FileProviderRepository>, LlmError>`.
2. **Adapters existentes implementan los puertos**:
   - `SignedUrlDownloader` impls `SignedUrlFetcher` (delega al método inherente).
   - `FileProviderFactory` impls `FileProviderFactoryPort` (delega al associated `create`).
3. **`LlmCallUseCase` refactorizado**:
   - Cero `use crate::llm::infrastructure` en el bloque de imports de producción.
   - Campos: `Option<Arc<dyn FileCacheRepository>>`, `Option<Arc<dyn FileProviderFactoryPort>>`, `Option<Arc<dyn SignedUrlFetcher>>`.
   - Builders: `with_file_cache`, `with_file_provider_factory`, `with_signed_url_fetcher`.
   - `execute()` solo arma el provider si los 3 puertos están inyectados; sin ellos, la resolución se omite silenciosamente (mismo comportamiento que antes con `file_cache=None`).
   - El método estático `resolve_files(...)` ahora recibe `fetcher: &dyn SignedUrlFetcher` (antes `&SignedUrlDownloader`).
4. **Sites de composición** (legítimos en `infrastructure/`): el nodo `dag_engine/infrastructure/nodes/llm.rs` sigue construyendo los concretos y pasándolos al use case — eso es exactamente la responsabilidad de la capa de infraestructura.

### Beneficios obtenidos

- Tests de `LlmCallUseCase` ya no dependen de `reqwest` para los path sin HTTP.
- Bindings (PyO3, napi-rs) pueden inyectar implementaciones custom.
- Cumple con la regla hexagonal del proyecto.

Los `use crate::llm::infrastructure::files::*` que quedan en el archivo del use case (líneas 549, 999) están dentro de bloques `#[cfg(test)]` y son fixtures, no código de producción — esa excepción está documentada inline.

---

## 4 + 5. Huérfanos (cache rows + provider files)

**Estado**: análisis profundo hecho en sesión 2026-05-03, **implementación pospuesta** hasta tener input del owner sobre las decisiones abiertas (ver "Decisiones requeridas" abajo).

### Reframing: ¿qué es realmente un huérfano?

Un huérfano es un archivo del provider que ya nadie consulta. El ciclo de vida tiene **dos vectores** donde se generan:

#### Vector A — UPSERT sobreescribe

Re-upload tras expiry o tras retry de 404 sin recovery exitoso:

```
provider_file_cache row apunta a file_OLD
    cache.upsert(new_entry)
    ON CONFLICT DO UPDATE
provider_file_cache row ahora apunta a file_NEW
→ file_OLD vive en el provider, ya no en ninguna fila de Colmena.
```

Mismo cuando `invalidate(doc, provider)` se llama — el `provider_file_id` queda colgado en el provider.

#### Vector B — Strategy change sin invalidate

Ejemplo real: el short-circuit de imágenes OpenAI introducido en commit `3f5e574`. Antes del commit, las imágenes JPEG con OpenAI subían a la Files API y la fila quedaba en cache. Después del commit, ese path va por URL passthrough — el código nuevo nunca hace `cache.lookup` para ese caso. La fila Y el archivo del provider quedan vivos pero inalcanzables.

Ejemplo observable hoy:
```
test-jpeg-2026-05-02 | openai    | file-M7YUj2Dd68bje3u636vqaA | NULL
```

### Insight clave

El almacenamiento de la fila en Postgres es despreciable (~200 bytes). El **costo real** está en el provider (cuota org). Limpiar la fila SIN limpiar el archivo del provider no resuelve nada → **#4 y #5 son el mismo problema visto desde dos lados**, conviene tratarlos juntos.

### Espacio de soluciones

Cuatro enfoques considerados:

#### Opción A — List-and-compare

```pseudo
files_provider := GET /v1/files
ids_cache := SELECT provider_file_id FROM provider_file_cache
huérfanos := files_provider - ids_cache
for f in huérfanos: DELETE /v1/files/{f}
```

| Pro | Contra |
|-----|--------|
| Captura Vector A automáticamente. | **Riesgo de nuke cross-system**: si otro team usa la misma API key (CI, otro feature), borramos sus archivos. |
| Sin cambios en `upsert`/`invalidate`. | Mitigación obligatoria: filtrar por `display_name` o `purpose` (los adapters no setean esos fields hoy). |
| | No captura Vector B: el `id` existe en provider y existe en cache → no es huérfano según este algoritmo, aunque nadie lo consulte. |

#### Opción B — Cola de cleanup determinística

Tabla nueva:
```sql
CREATE TABLE provider_file_cleanup_queue (
  id BIGSERIAL PRIMARY KEY,
  provider TEXT NOT NULL,
  provider_file_id TEXT NOT NULL,
  queued_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  attempts INT NOT NULL DEFAULT 0,
  last_attempt_at TIMESTAMPTZ,
  last_error TEXT
);
```

`upsert`/`invalidate` encolan el `provider_file_id` viejo dentro de la misma transacción del cambio en cache. Janitor procesa la cola: `DELETE` en provider → saca de la cola.

| Pro | Contra |
|-----|--------|
| **Surgical**: solo tocamos archivos que NOSOTROS subimos. Cero riesgo cross-system. | Tabla extra + cambios transaccionales en `upsert`/`invalidate`. |
| Captura Vector A perfectamente. | No captura Vector B (filas que nunca pasan por upsert/invalidate). |
| Trail auditable: quién encoló, cuántos intentos, último error. | Más código. |

#### Opción C — LRU eviction por `last_used_at`

Ahora que #2 está resuelto y `last_used_at` se actualiza en cada hit:

```sql
WITH stale AS (
  SELECT * FROM provider_file_cache WHERE last_used_at < NOW() - INTERVAL '30 days'
)
-- Encolar para borrado en provider, luego borrar fila.
```

| Pro | Contra |
|-----|--------|
| Captura **Vector B** perfectamente: si nadie consulta la fila por X días, muere. | TTL es heurístico — un archivo legítimo poco usado podría morir y forzar un re-upload. |
| Una sola query SQL, simple. | No captura Vector A solo (las filas reescritas por upsert son nuevas, no stale). |
| Auto-cleaning sin código en el flujo normal. | |

#### Opción D — Híbrida (B + C) ← recomendada

- **Cola** (Opción B) para Vector A — determinística, capta todo lo que pasa por upsert/invalidate.
- **LRU sweep** (Opción C) para Vector B — heurística, captura strategy-change orphans.
- **Migración one-shot** para huérfanos visibles HOY (filas tipo `test-jpeg-...`).

Janitor único corre los 3 algoritmos.

### Cambios requeridos en el código

1. **Trait `FileProviderRepository`**: agregar `async fn delete_file(&self, provider_file_id: &str) -> Result<(), LlmError>`. Implementar en los 3 adapters (~2h).
2. **Tabla `provider_file_cleanup_queue`**: nueva migración SQL.
3. **`PostgresFileCache::upsert`**: leer fila existente, encolar el viejo `provider_file_id`, luego UPSERT — todo en transacción.
4. **`PostgresFileCache::invalidate`**: idem.
5. **Janitor**: nueva función o binario que procesa la cola + ejecuta el LRU sweep.
6. **Migración retroactiva** (script standalone, NO automático): encolar las filas huérfanas conocidas (image rows en OpenAI/Anthropic).

### Decisiones requeridas (input del owner pendiente)

**P1. ¿Dónde corre el janitor?**

| Opción | Pro | Contra |
|--------|-----|--------|
| Como nodo DAG `cleanup_provider_files` invocable por cron externo | Usa infra existente | Depende de armar el graph |
| Tarea background dentro del binario `dag_engine serve` (tokio interval) | Simple, siempre activo | Race con múltiples instancias del server |
| Binario separado `colmena_janitor` ejecutado por cron OS / k8s | Idempotente, controlable | Nuevo bin, nuevo deploy |

**Default sugerido**: binario separado por simplicidad operacional.

**P2. TTL para LRU eviction**

- Si los PDFs se reusan típicamente 1-2 semanas → 30 días es seguro.
- Si los docs son one-shot (sube, lee, nunca más) → 7 días sobra.

Define el upper bound de re-upload aceptable. Default sugerido: **30 días**.

**P3. Frecuencia del janitor**

Default sugerido: cada **15 minutos** para la cola (low-latency cleanup), **diaria** para el LRU sweep.

**P4. ¿Gemini en el janitor?**

Gemini auto-expira a 48h. Encolar Gemini probablemente lleva a `DELETE` que devuelve 404. **Default sugerido: skipear Gemini en la cola**, tratar 404 como éxito si igual lo procesa.

**P5. Migración retroactiva**

Ejecutar como **script standalone manual**, NO como migración automática. Una migración que borra archivos del provider en cada deploy es peligrosa.

### Tests requeridos

- Cola: encolado correcto en `upsert` (sobreescritura) y `invalidate` (DELETE). Idempotencia (re-encolar mismo id).
- Janitor: procesa entries, marca attempts en fallas, reintenta hasta N veces.
- Adapter `delete_file`: 200 ok, 404 (tratar como ok), 5xx (error).
- LRU sweep: respeta `last_used_at`, no borra activos.
- Integration: simular Vector A y B, verificar que el janitor los limpia.

---

## 6. `ProviderKind::Mock` fallback silencioso ✅ RESUELTO

**Ubicación**: `src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs::parse_provider_from_row`.

### Síntoma original

```rust
provider: ProviderKind::from_str(&r.provider).unwrap_or_else(|_| {
    eprintln!("WARN: ...");
    ProviderKind::Mock
}),
```

Filas con provider string corrupto se devolvían como `ProviderKind::Mock`. El `eprintln!` era invisible bajo structured logging. El caller usaba el `provider_file_id` con un kind equivocado y obtenía errores opacos en el LLM call.

### Solución implementada

1. **Parsing extraído a función pura `parse_provider_from_row(provider_db, document_id)`** — fácil de unit-testear sin Postgres.
2. **Fail-fast** con `LlmError::RequestFailed { message: "provider_file_cache: corrupted provider '<value>' for document_id=<id>: <error>" }`. El operador puede invalidar la fila a mano.
3. **`tracing::error!`** estructurado en lugar de `eprintln!` (campos `provider`, `document_id` capturados en JSON estructurado para alertas).
4. **`eprintln!("WARN: file resolution failed: {e}")` en `LlmCallUseCase::resolve_files` también pasó a `tracing::warn!`** — coherencia del feature.
5. **Bug latente de simetría arreglado**: `ProviderKind::from_str` no aceptaba `"mock"` (solo `to_string()` lo emitía). Ahora `from_str("mock") = Ok(Mock)`. Si el factory algún día insertara una fila Mock, el round-trip funciona.

### Tests

- `parse_provider_from_row_accepts_known_kinds`: round-trip de `anthropic`, `openai`, `gemini`, `mock`.
- `parse_provider_from_row_fails_on_corrupted_string`: assert `RequestFailed` con `message` que contiene el string corrupto y el `document_id`.

---

## 7. Mime malformado clasificado como `FileApiUploadFailed` ✅ RESUELTO

**Ubicación**: adapters de Anthropic y OpenAI Files API (Gemini no aplica — no hace pre-validación local del mime, lo manda como header HTTP).

### Síntoma original

`Part::stream(body).mime_str(mime_type)` fallaba cuando el mime era inválido y se mapeaba a:

```rust
LlmError::FileApiUploadFailed { provider, message: "precondition: invalid mime '...': ..." }
```

Pero esto era violación de precondición del caller (mime mal formado), no error de upload — ni siquiera salió el HTTP. La mezcla confundía retry policies.

### Solución implementada

1. **Nueva variante** `LlmError::InvalidMimeType { mime: String, message: String }`.
2. **Anthropic adapter** ([anthropic_files_api.rs](src/libs/colmena/src/llm/infrastructure/files/anthropic_files_api.rs)) y **OpenAI adapter** ([openai_files_api.rs](src/libs/colmena/src/llm/infrastructure/files/openai_files_api.rs)) ahora mapean el error de `mime_str` a `InvalidMimeType` con el mime original y el mensaje detallado del parser.
3. **Gemini** no requiere cambio: pasa el mime como header `X-Goog-Upload-Header-Content-Type` directamente sin validación local — si es inválido, el server lo rechaza con 400 (que ya es `FileApiUploadFailed`, correcto en ese caso porque sí fue HTTP failure).

### Tests

- `anthropic_files_api::tests::upload_classifies_invalid_mime_as_invalid_mime_type`: pasa `"not a real mime"` a `upload_streaming`, verifica `LlmError::InvalidMimeType { mime: "not a real mime", .. }`. No necesita servidor: el error fires antes de cualquier HTTP.
- `openai_files_api::tests::upload_classifies_invalid_mime_as_invalid_mime_type`: idéntico para OpenAI.

### Follow-up opcional

Validar el mime también en `parse_file_entries` del nodo `llm_call` para fallar aún más temprano (en parse JSON, no en upload). No es crítico — la nueva variante ya distingue el error correctamente en cualquier path.

---

## 8. Tests de integración E2E reproducibles

**Síntoma**

Los `tests/graphs/media/*_url_*.json` requieren signed URLs reales (TTL 6h) y API keys de los 3 providers. No son reproducibles automáticamente, son tests manuales.

### Solución

Tests con stubs de los 3 providers (wiremock) que reproduzcan el end-to-end completo:

1. Mock GCS endpoint que devuelve un PDF de 1 KB.
2. Mock de cada provider's Files API endpoint.
3. Mock del messages/generateContent endpoint.
4. Verificar el flujo completo: parse → resolve_files → upload → cache upsert → LLM call → response.

Beneficios:
- CI puede correrlos sin API keys.
- Cubren el wiring completo (incluyendo los fixes post-implementación).
- Detectan regresiones antes del primer test manual.

---

## 9. Métricas / observabilidad

**Síntoma**

Solo hay logs (`COLMENA_VERBOSE=1`). Para producción, sería útil:

- `provider_file_cache_hit_total{provider}` y `_miss_total{provider}`.
- `file_upload_duration_seconds{provider, mime_class}` (histogram).
- `file_download_duration_seconds`.
- `signed_url_fetch_failed_total{status}`.
- `provider_file_not_found_total{provider}`.

### Solución

Agregar métricas Prometheus en los puntos de decisión del use case + adapters. Si el proyecto ya usa `metrics` o similar crate, integrar ahí. Si no, decision point: ¿añadimos `metrics` ahora o seguimos con logs?

---

## 10. Cache hit cross-session por `sha256`

**Síntoma**

Hoy el cache key es `(document_id, provider)`. Dos sesiones distintas con el mismo archivo (mismos bytes) pero diferente `id` del emisor van a cada uno hacer un upload independiente.

### Solución

Calcular `sha256` durante el streaming download (cero overhead extra: ya estamos consumiendo el stream). Cache lookup secundario por `(sha256, provider)`. Si hay hit, evitar el upload (pero el download ya se hizo).

**Tradeoff**: el primer download igual ocurre para calcular el hash; solo se ahorra el upload. Útil cuando el upload es caro (Gemini resumable con 8MB chunks) y el download es rápido (GCS interno).

YAGNI hasta ver volumen real (spec lo descartó explícitamente).

---

## Cómo trabajar estos items

1. Cada item ≥ media severidad debería abrirse como issue separado en GitHub con esta tabla como referencia.
2. Items de baja severidad (#2, #6, #7) pueden agruparse en un PR de "polish post-launch" cuando alguien tenga 1-2 horas libres.
3. **No hay items bloqueantes** — el feature está en estado mergeable.

## Referencias

- [Spec original con sección de hallazgos](./2026-05-02-large-document-files-api-design.md#hallazgos-de-integración-real-post-implementación)
- [Plan con commits del feature](../plans/2026-05-02-large-document-files-api.md)
- [Guía para usuarios](../../developer_guide/28_large_files_api.md)
