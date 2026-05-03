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
| 4 | Filas huérfanas en cache cuando se cambian estrategias | Baja | No | 1 día (con limpieza retroactiva) |
| 5 | Janitor de archivos huérfanos en proveedores | Media | No | 2-3 días |
| 6 | ~~`ProviderKind::Mock` fallback silencioso en `PostgresFileCache::lookup`~~ ✅ Resuelto | Baja | No | 30 min |
| 7 | Error de mime malformado clasificado como `FileApiUploadFailed` | Baja | No | 1 hora |
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

## 4. Filas huérfanas en cache

**Síntoma**

Cuando una estrategia cambia mid-feature (e.g., introducimos el short-circuit de OpenAI imágenes después de un upload exitoso), las filas existentes en cache quedan apuntando a `provider_file_id` válidos pero nunca referenciados de nuevo.

Ejemplo real observado:
```
test-jpeg-2026-05-02 | openai    | file-M7YUj2Dd68bje3u636vqaA | NULL
```

Esa fila se creó antes del fix `3f5e574`. Ahora OpenAI imágenes hacen short-circuit y nunca pasan por cache lookup. La fila vive ahí inútil.

### Solución

Migración one-shot de limpieza:

```sql
DELETE FROM provider_file_cache
WHERE provider IN ('openai', 'anthropic')
  AND mime_type LIKE 'image/%';
```

Y aplicar lógica preventiva en el use case: cuando entra un short-circuit, verificar si hay fila vieja y opcionalmente borrarla con `cache.invalidate(...)` por consistencia.

---

## 5. Janitor de archivos huérfanos en proveedores

**Síntoma**

Cuando el cache se invalida o se sobrescribe (UPSERT race), el `provider_file_id` viejo sigue vivo en el provider:

- **Anthropic**: persiste indefinidamente, cuenta contra cuota org (500 GB).
- **OpenAI**: persiste indefinidamente, cuenta contra cuota usuario.
- **Gemini**: expira solo a 48h (no requiere janitor).

Para producción a alto volumen, esto puede saturar la cuota del provider.

### Solución

Background job (cron de Colmena, o agente externo) que:

1. Lista archivos en cada provider via `GET /v1/files`.
2. Compara con la tabla `provider_file_cache.provider_file_id`.
3. Borra los que no aparecen.

Posibles casos de borrado equivocado:
- Si otro sistema usa la misma API key y sus archivos no están en nuestro cache, los borraríamos.
- Mitigación: filtrar por `display_name` o `purpose` para acotar a archivos creados por Colmena.

Out of scope hasta ver volumen real (spec).

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

## 7. Mime malformado clasificado como `FileApiUploadFailed`

**Ubicación**: los 3 adapters Files API (Anthropic, OpenAI, Gemini).

### Síntoma

Si `Part::stream(body).mime_str(mime_type)` falla porque el mime es inválido, el error se mapea a:

```rust
LlmError::FileApiUploadFailed { provider, message: "precondition: invalid mime '...': ..." }
```

Pero esto es una violación de precondición del caller, no un error de upload (no se hizo HTTP call siquiera). Mezclarlos confunde retry policies.

### Solución

Agregar variante específica:

```rust
#[error("invalid mime type '{mime}': {message}")]
InvalidMimeType { mime: String, message: String },
```

Y mapear en los 3 adapters. Mantener el prefijo `precondition:` por compatibilidad de logs antiguos.

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
