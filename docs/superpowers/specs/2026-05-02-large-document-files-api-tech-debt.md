# Deuda técnica: Large Document Files API

**Fecha**: 2026-05-02
**Feature ref**: `docs/superpowers/specs/2026-05-02-large-document-files-api-design.md`
**Estado**: feature mergeado como alpha/beta; los items de aquí son follow-ups conocidos.

Esta tabla agrupa el trabajo pendiente y la deuda registrada durante la implementación y el testing real del feature de archivos grandes.

---

## Deuda priorizada

| # | Item | Severidad | Bloqueante? | Estimación |
|---|------|-----------|-------------|-------------|
| 1 | Retry on `ProviderFileNotFound` no recupera (no-op) | Alta | No (best-effort documentado) | 1-2 días |
| 2 | `last_used_at` no se actualiza en cache hit | Baja | No | 30 min |
| 3 | Layer leak: `LlmCallUseCase` importa de `infrastructure` | Media | No (compila y funciona) | 1 día |
| 4 | Filas huérfanas en cache cuando se cambian estrategias | Baja | No | 1 día (con limpieza retroactiva) |
| 5 | Janitor de archivos huérfanos en proveedores | Media | No | 2-3 días |
| 6 | `ProviderKind::Mock` fallback silencioso en `PostgresFileCache::lookup` | Baja | No | 30 min |
| 7 | Error de mime malformado clasificado como `FileApiUploadFailed` | Baja | No | 1 hora |
| 8 | Tests de integración E2E reproducibles (sin signed URLs) | Media | No | 2 días |
| 9 | Métricas/observabilidad (Prometheus) para cache hit-rate | Baja | No | 1 día |
| 10 | Cache hit cross-session por `sha256` (sin coordinación con emisor) | Muy baja | No (YAGNI hasta ver volumen) | 3-5 días |

---

## 1. Retry on `ProviderFileNotFound` no recupera

**Ubicación**: `src/libs/colmena/src/llm/application/llm_call_use_case.rs::reset_uploaded_files_with_id`.

### Síntoma

Cuando el LLM falla con `ProviderFileNotFound { provider_file_id }` (porque el archivo se borró del provider, ya sea por TTL Gemini agotado o cleanup manual en Anthropic/OpenAI), el wrapper `call_with_file_retry`:

1. ✅ Invalida la fila correspondiente del cache.
2. ❌ **NO** convierte `FileSource::Uploaded(ref)` de vuelta a `FileSource::SignedUrl(url)` en los mensajes.
3. Re-ejecuta `resolve_files_in_messages`, pero ve `Uploaded` y short-circuit (ya está resuelto), no re-sube.
4. Re-llama al LLM con el mismo `file_id` muerto → mismo error.

El comentario actual en `reset_uploaded_files_with_id` lo admite explícitamente:

> "We rely on cache invalidation. The retry's resolve_files call will see the invalidated cache row and re-upload from the SignedUrl ONLY IF the original FileSource was SignedUrl. If the caller already sent FileSource::Uploaded directly, we cannot re-upload because there's no source. The retry will fail, and the original error propagates."

### Solución propuesta

Antes de la primera llamada al LLM, snapshot las URLs originales:

```rust
// In execute(), after resolve_files_in_messages:
let signed_url_snapshot: HashMap<String /* provider_file_id */, String /* original_url */> = ...;
```

En `reset_uploaded_files_with_id`:

```rust
fn reset_uploaded_files_with_id(
    messages: &mut [LlmMessage],
    provider_file_id: &str,
    snapshot: &HashMap<String, String>,
) {
    if let Some(original_url) = snapshot.get(provider_file_id) {
        for msg in messages.iter_mut() {
            if let Some(files) = msg.files_mut() {
                for file in files.iter_mut() {
                    if let FileSource::Uploaded(r) = &file.source {
                        if r.provider_file_id == provider_file_id {
                            file.source = FileSource::SignedUrl(original_url.clone());
                        }
                    }
                }
            }
        }
    }
}
```

### Test que falta

`retries_once_on_provider_file_not_found_recovers_with_resolved_signed_url`: dispatch de mock que devuelve 404 la primera vez, success la segunda; verificar que el `provider.upload_count` aumentó (re-upload real ocurrió, no solo recall).

---

## 2. `last_used_at` no se actualiza en cache hit

**Ubicación**: `src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs::lookup`.

### Síntoma

`last_used_at` solo se escribe en `INSERT` y `ON CONFLICT DO UPDATE`. En cache hit nunca se hace `UPDATE`. Las filas conservan `last_used_at = uploaded_at` aunque sean accedidas miles de veces.

Esto rompe métricas de "qué archivos están activos" y cualquier futuro janitor que use `last_used_at` para LRU.

### Solución

Hacer `lookup` ejecutar un `UPDATE ... SET last_used_at = NOW() RETURNING *` en lugar del `SELECT`. Una sola query.

```sql
UPDATE provider_file_cache
SET last_used_at = NOW()
WHERE document_id = $1 AND provider = $2
RETURNING document_id, provider, provider_file_id, mime_type, filename,
          size_bytes, uploaded_at, expires_at, last_used_at;
```

Riesgo: el `UPDATE` puede ser un poco más lento que `SELECT` por write-lock, pero en esta tabla la concurrencia es baja (cada `(document_id, provider)` se actualiza desde un solo request).

### Test que falta

`lookup_advances_last_used_at`: insertar fila, dormir 50ms, lookup, verificar `last_used_at > uploaded_at`.

---

## 3. Layer leak: `LlmCallUseCase` importa de `infrastructure`

**Ubicación**: `src/libs/colmena/src/llm/application/llm_call_use_case.rs:6`.

### Síntoma

```rust
use crate::llm::infrastructure::files::{FileProviderFactory, SignedUrlDownloader};
```

`LlmCallUseCase` está en la capa de aplicación. Por las reglas de la arquitectura hexagonal del proyecto (`CLAUDE.md`: "Domain layer has ZERO infrastructure dependencies"), las capas superiores tampoco deberían depender directamente de infraestructura — deben recibir las dependencias inyectadas vía puertos.

### Solución

1. Definir un nuevo puerto en `domain/`:
   ```rust
   #[async_trait]
   pub trait SignedUrlFetcher: Send + Sync {
       async fn stream(&self, url: &str) -> Result<BoxedByteStream, LlmError>;
   }
   ```
2. `SignedUrlDownloader` implementa este trait.
3. `LlmCallUseCase` recibe `Arc<dyn SignedUrlFetcher>` (vía `with_downloader` que ya existe pero importa el tipo concreto).
4. Eliminar `FileProviderFactory` del use case: el caller (el nodo) construye el `Arc<dyn FileProviderRepository>` y lo inyecta vía un nuevo método del use case (`execute_with_file_provider(...)`) o vía `with_file_provider(provider)` builder.

### Beneficios

- Tests de `LlmCallUseCase` no dependen de `reqwest`.
- Bindings (PyO3, napi-rs) pueden inyectar implementaciones custom (e.g., para tests).
- Cumple con la regla hexagonal del proyecto.

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

## 6. `ProviderKind::Mock` fallback silencioso

**Ubicación**: `src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs::lookup`.

### Síntoma

```rust
provider: ProviderKind::from_str(&r.provider).unwrap_or_else(|_| {
    eprintln!("WARN: ...");
    ProviderKind::Mock
}),
```

Si una fila tiene un provider string corrupto o desconocido, se devuelve como `ProviderKind::Mock`. El `eprintln!` es invisible en producción con structured logging.

Riesgos:
- Lookup retorna fila con provider mismatched, se usa el `provider_file_id` con un provider equivocado, falla en el LLM call.

### Solución

Cambiar a `Err(LlmError::RequestFailed { message: "corrupted provider in cache row" })`. Hacer fail-fast en lugar de silent corruption.

Adicional: usar `tracing::warn!` en lugar de `eprintln!` en TODO el código del feature (varios sitios).

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
