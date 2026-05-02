# Large Document Files API — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Soportar documentos grandes (≥30 MB) en `llm_call` mediante las Files APIs de Gemini, Anthropic y OpenAI, con cache persistido por `(document_id, provider)` en Postgres y streaming pipe end-to-end (download GCS → upload provider).

**Architecture:** Hexagonal. Dos puertos nuevos en `llm/domain` (`FileProviderRepository`, `FileCacheRepository`). Tres adapters Files-API en `llm/infrastructure/files/` + adapter Postgres + downloader HTTP. `LlmCallUseCase` orquesta resolución de archivos antes del LLM call. Factory de file-providers separada del `LlmProviderFactory` para mantenibilidad. Cache siempre usa `DATABASE_URL` env vía `PgPoolRegistry` existente.

**Tech Stack:** Rust + sqlx 0.8 (Postgres) + reqwest 0.11 (streaming) + tokio + futures + bytes. Tests con wiremock 0.6 + mockall 0.11. Migración SQL en `migrations/postgres/`.

**Spec:** [docs/superpowers/specs/2026-05-02-large-document-files-api-design.md](../specs/2026-05-02-large-document-files-api-design.md)

---

## Estado de implementación (actualizado 2026-05-02)

✅ **19 tareas planeadas completadas** en commits `3a560a0`..`1b7db65`.

🔧 **Fixes adicionales descubiertos en testing real con APIs en producción**:

| Commit | Cambio |
|--------|--------|
| `5090720` | `feat(dag/llm): minimal C1` — wire file resolution into the production node path that bypasses `LlmCallUseCase` |
| `d779158` | `feat(llm): wire PostgresFileCache + observability logs` — production cache wiring + `COLMENA_VERBOSE=1` traceability |
| `d98095d` | `fix(llm/anthropic): use url source for images instead of file_id` — Anthropic rejects `file` source for image content |
| `3f5e574` | `fix(llm/openai): use url for chat-completions images` — OpenAI chat completions doesn't accept `file_id` in `image_url` |
| `25fd639` | `feat(llm/anthropic): send files-api beta header on messages call` — beta header required on `/v1/messages`, not just upload |
| `9a61923` | `fix(llm/openai): omit filename when using file_id in Responses API` — `file_id` and `filename` are mutually exclusive |
| `71168f9` | `fix(llm/gemini-files): respect 8 MB chunk granularity in resumable upload` — non-final chunks must be exact multiples of CHUNK_SIZE |

📋 **Tests reales realizados**:

- Imagen JPEG (~1 MB) en los 3 providers → todos respondieron con descripción correcta.
- PDF 99 páginas (12 MB) en los 3 providers → OpenAI y Gemini respondieron con resumen correcto; Anthropic Haiku hit context-window (208k > 200k).
- PDF 55 MB en los 3 providers → todos hicieron upload exitoso pero hit límites de modelo en generación (100 páginas Anthropic, 32MB pull OpenAI, "files bytes too large" Gemini).
- Cache hit verificado: segunda corrida con mismo `id` skipea download/upload completos.

📍 **Para detalles completos de los hallazgos**, ver la sección "Hallazgos de integración real (post-implementación)" en `docs/superpowers/specs/2026-05-02-large-document-files-api-design.md`.

📚 **Guía de usuario**: `docs/developer_guide/28_large_files_api.md`.

⚠ **Deuda no implementada** (registrada para follow-ups):

- C2: retry on `ProviderFileNotFound` no recupera (`reset_uploaded_files_with_id` es no-op). Best-effort, documentado.
- `last_used_at` no se actualiza en cache hit (solo en upsert).
- Layer leak: `LlmCallUseCase` (application) importa de `infrastructure::files`. Debería ser vía puerto.
- Filas huérfanas en cache cuando cambian estrategias mid-feature (e.g., el upload de imagen OpenAI antes del short-circuit).

---

## File Structure

### Files a crear

| Path | Responsabilidad |
|------|-----------------|
| `src/libs/colmena/src/llm/domain/file_provider_repository.rs` | Trait `FileProviderRepository` + `BoxedByteStream` + `ProviderFileRef` |
| `src/libs/colmena/src/llm/domain/file_cache_repository.rs` | Trait `FileCacheRepository` + `CachedFileEntry` + `is_likely_alive` |
| `src/libs/colmena/src/llm/infrastructure/files/mod.rs` | Re-exports del subdir |
| `src/libs/colmena/src/llm/infrastructure/files/signed_url_downloader.rs` | HTTP GET streaming sin Authorization |
| `src/libs/colmena/src/llm/infrastructure/files/anthropic_files_api.rs` | Multipart upload con beta header |
| `src/libs/colmena/src/llm/infrastructure/files/openai_files_api.rs` | Multipart upload con purpose=user_data |
| `src/libs/colmena/src/llm/infrastructure/files/gemini_files_api.rs` | Resumable upload (3 fases) |
| `src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs` | sqlx adapter de `FileCacheRepository` |
| `src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs` | Factory hermana de `LlmProviderFactory` |
| `src/libs/colmena/migrations/postgres/20260502000001_provider_file_cache.sql` | Tabla + índice |
| `tests/graphs/media/pdf_url_anthropic.json` | Test graph con signed URL |
| `tests/graphs/media/pdf_url_openai.json` | Idem OpenAI |
| `tests/graphs/media/pdf_url_gemini.json` | Idem Gemini |
| `tests/graphs/media/README.md` | Doc de regeneración manual de signed URLs |

### Files a modificar

| Path | Cambio |
|------|--------|
| `src/libs/colmena/src/llm/domain/llm_message.rs` | `FileData` gana `document_id: Option<String>` y `source: FileSource` enum |
| `src/libs/colmena/src/llm/domain/llm_error.rs` | +6 variantes nuevas |
| `src/libs/colmena/src/llm/domain/mod.rs` | Re-exports nuevos |
| `src/libs/colmena/src/llm/infrastructure/mod.rs` | `pub mod files;` |
| `src/libs/colmena/src/llm/infrastructure/anthropic_adapter.rs` | Aceptar `FileSource::Uploaded` con `file_id` |
| `src/libs/colmena/src/llm/infrastructure/openai_adapter.rs` | Aceptar `FileSource::Uploaded` con `file_id` |
| `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs` | Aceptar `FileSource::Uploaded` con `file_uri` |
| `src/libs/colmena/src/llm/application/llm_call_use_case.rs` | Resolución de archivos (cache lookup + download + upload + dedup + retry) |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs:482-547` | Parser que acepta `id`/`url`/`data`/`path` con validación de tamaño |

---

## Order of work

1. **Phase 1 — Domain types y errores** (Tasks 1-3)
2. **Phase 2 — Domain ports** (Tasks 4-5)
3. **Phase 3 — Postgres cache** (Tasks 6-7)
4. **Phase 4 — HTTP downloader** (Task 8)
5. **Phase 5 — Files API adapters** (Tasks 9-11)
6. **Phase 6 — Factory** (Task 12)
7. **Phase 7 — LLM adapters reciben `Uploaded`** (Tasks 13-15)
8. **Phase 8 — Use case integration** (Tasks 16-17)
9. **Phase 9 — Node parser** (Task 18)
10. **Phase 10 — Test graphs y docs** (Task 19)

---

## Phase 1: Domain types y errores

### Task 1: Agregar `ProviderFileRef` y enum `FileSource`

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/llm_message.rs`

- [ ] **Step 1: Escribir test que falla**

Append a `src/libs/colmena/src/llm/domain/llm_message.rs` dentro del `mod tests`:

```rust
#[test]
fn test_file_data_with_signed_url_source() {
    let file = FileData {
        document_id: Some("doc-123".to_string()),
        mime_type: "application/pdf".to_string(),
        filename: "report.pdf".to_string(),
        size_hint: Some(47_185_920),
        source: FileSource::SignedUrl("https://storage.googleapis.com/bucket/x?sig=abc".to_string()),
    };
    assert_eq!(file.document_id.as_deref(), Some("doc-123"));
    match &file.source {
        FileSource::SignedUrl(u) => assert!(u.contains("storage.googleapis.com")),
        _ => panic!("expected SignedUrl variant"),
    }
}

#[test]
fn test_provider_file_ref_construction() {
    use crate::llm::domain::ProviderFileRef;
    use crate::llm::domain::ProviderKind;
    use chrono::Utc;
    let r = ProviderFileRef {
        provider: ProviderKind::Anthropic,
        provider_file_id: "file_abc".to_string(),
        mime_type: "application/pdf".to_string(),
        filename: "x.pdf".to_string(),
        expires_at: None,
    };
    assert_eq!(r.provider_file_id, "file_abc");
    let _ = Utc::now();
}
```

- [ ] **Step 2: Run y verificar falla**

Run: `cargo check -p colmena_dag_engine --lib`
Expected: errores tipo "cannot find type FileSource" / "cannot find type ProviderFileRef".

- [ ] **Step 3: Implementar mínimo**

Modificar `src/libs/colmena/src/llm/domain/llm_message.rs`. Reemplazar la struct `FileData` actual:

```rust
use chrono::{DateTime, Utc};
use crate::llm::domain::ProviderKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub struct FileData {
    /// Identificador único enviado por el emisor. Requerido cuando `source` es `SignedUrl`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    pub mime_type: String,
    pub filename: String,
    /// Hint del campo `size_bytes` del JSON. No es ground truth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_hint: Option<u64>,
    pub source: FileSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileSource {
    /// Bytes ya en RAM (vino como `data` base64 < 30 MB, o `path` < 30 MB).
    InlineBytes { bytes: Vec<u8> },
    /// Signed URL pendiente de descarga + upload al provider.
    SignedUrl(String),
    /// Ya subido al provider.
    Uploaded(ProviderFileRef),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq))]
pub struct ProviderFileRef {
    pub provider: ProviderKind,
    pub provider_file_id: String,
    pub mime_type: String,
    pub filename: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}
```

Y conservar el helper de construcción retrocompatible. Reemplazar la línea actual `pub fn user_with_files(content, files: Vec<FileData>)` agregando además un constructor desde bytes:

```rust
impl FileData {
    /// Constructor retrocompatible: bytes ya en memoria.
    pub fn inline(mime_type: String, filename: String, bytes: Vec<u8>) -> Self {
        Self {
            document_id: None,
            mime_type,
            filename,
            size_hint: None,
            source: FileSource::InlineBytes { bytes },
        }
    }
}
```

Actualizar `src/libs/colmena/src/llm/domain/mod.rs` para re-exportar `FileSource` y `ProviderFileRef`:

```rust
pub use llm_message::{FileData, FileSource, LlmMessage, MessageRole, ProviderFileRef};
```

- [ ] **Step 4: Run tests del módulo**

Run: `cargo test --lib llm_message -p colmena_dag_engine`
Expected: PASS de los 2 tests nuevos + los existentes.

- [ ] **Step 5: Buscar y reparar callers rotos**

Run: `cargo check -p colmena_dag_engine --lib 2>&1 | grep -E "error\[" | head -20`

Cada lugar donde se construya `FileData { mime_type, filename, bytes }` (struct literal directo) ahora rompe. Reemplazar por `FileData::inline(mime_type, filename, bytes)`. Lugares conocidos: `dag_engine/infrastructure/nodes/llm.rs:512` y `:535`, posiblemente otros tests.

- [ ] **Step 6: Verificar build limpio**

Run: `cargo check -p colmena_dag_engine --lib`
Expected: 0 errores.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/llm/domain/llm_message.rs \
        src/libs/colmena/src/llm/domain/mod.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm): introduce FileSource enum and ProviderFileRef

Replaces FileData's bytes field with a FileSource enum that supports
InlineBytes, SignedUrl, and Uploaded variants. Adds optional document_id
and size_hint. FileData::inline() preserves the existing inline-bytes
construction path."
```

---

### Task 2: Agregar variantes a `LlmError`

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/llm_error.rs`

- [ ] **Step 1: Test que falla**

Append a `llm_error.rs` (al final del archivo, fuera del enum):

```rust
#[cfg(test)]
mod files_error_tests {
    use super::*;

    #[test]
    fn data_field_too_large_message() {
        let e = LlmError::DataFieldTooLarge { size: 50_000_000 };
        assert!(format!("{}", e).contains("30"));
        assert!(format!("{}", e).contains("50000000"));
    }

    #[test]
    fn url_without_document_id_message() {
        let e = LlmError::UrlWithoutDocumentId;
        assert!(format!("{}", e).to_lowercase().contains("id"));
    }

    #[test]
    fn provider_file_not_found_carries_id() {
        let e = LlmError::ProviderFileNotFound { provider_file_id: "file_abc".into() };
        assert!(format!("{}", e).contains("file_abc"));
    }
}
```

- [ ] **Step 2: Run y verificar falla**

Run: `cargo test --lib files_error_tests -p colmena_dag_engine`
Expected: errores de compilación "no variant DataFieldTooLarge / UrlWithoutDocumentId / ProviderFileNotFound".

- [ ] **Step 3: Agregar variantes**

Insertar al final del enum `LlmError`, antes del `}`:

```rust
    // File handling errors (Files API integration)
    #[error("data field exceeds 30 MB limit (got {size} bytes); emitter must use url for large files")]
    DataFieldTooLarge { size: u64 },

    #[error("path file exceeds 30 MB limit (got {size} bytes); use url for large files")]
    PathFieldTooLarge { size: u64 },

    #[error("url field requires id field to enable cache lookup")]
    UrlWithoutDocumentId,

    #[error("signed URL fetch failed with status {status}")]
    SignedUrlFetchFailed { status: u16 },

    #[error("file upload to {provider} Files API failed: {message}")]
    FileApiUploadFailed { provider: String, message: String },

    #[error("provider rejected file with id {provider_file_id}: not found")]
    ProviderFileNotFound { provider_file_id: String },

    #[error("all files in the request failed to materialize")]
    AllFilesFailedToResolve,
```

- [ ] **Step 4: Run y verificar pasa**

Run: `cargo test --lib files_error_tests -p colmena_dag_engine`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/domain/llm_error.rs
git commit -m "feat(llm): add file handling error variants to LlmError

Introduces DataFieldTooLarge, PathFieldTooLarge, UrlWithoutDocumentId,
SignedUrlFetchFailed, FileApiUploadFailed, ProviderFileNotFound,
AllFilesFailedToResolve."
```

---

### Task 3: Quitar `derive(PartialEq)` cuando no compatible

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/llm_error.rs` (si los nuevos variants rompen `PartialEq`)

- [ ] **Step 1: Compilar y observar**

Run: `cargo check -p colmena_dag_engine --lib`
Expected: si hay error de `PartialEq` por algún tipo embebido (poco probable, todos los nuevos campos son `String`/`u64`/`u16`), inspeccionar.

- [ ] **Step 2: Si todo compila, sin cambios**

Si `cargo check` pasa, este task es no-op. Si falla, retirar `PartialEq` de `LlmError` (`#[derive(Debug, Error)]` solamente) y ajustar tests que comparen errores con `==` para usar `matches!`.

- [ ] **Step 3: Commit (si hubo cambios)**

```bash
git add -p src/libs/colmena/src/llm/domain/llm_error.rs
git commit -m "fix(llm): adjust PartialEq derive on LlmError"
```

---

## Phase 2: Domain ports

### Task 4: Trait `FileProviderRepository`

**Files:**
- Create: `src/libs/colmena/src/llm/domain/file_provider_repository.rs`
- Modify: `src/libs/colmena/src/llm/domain/mod.rs`

- [ ] **Step 1: Test que falla**

Crear `src/libs/colmena/src/llm/domain/file_provider_repository.rs` con un mock test que valide la signatura del trait:

```rust
//! Port para subir archivos al Files API de un proveedor LLM.
//!
//! Implementado por adapters específicos por proveedor en
//! `llm/infrastructure/files/`. Consumido por `LlmCallUseCase` cuando
//! materializa archivos cuya `FileSource` es `SignedUrl`.

use crate::llm::domain::{LlmError, ProviderFileRef, ProviderKind};
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;
use std::time::Duration;

/// Stream de bytes que el adapter consume para hacer upload.
/// Se construye típicamente desde `reqwest::Response::bytes_stream()`.
pub type BoxedByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

#[async_trait]
pub trait FileProviderRepository: Send + Sync {
    /// Sube un archivo al Files API consumiendo el stream.
    async fn upload_streaming(
        &self,
        stream: BoxedByteStream,
        mime_type: &str,
        filename: &str,
    ) -> Result<ProviderFileRef, LlmError>;

    /// TTL del archivo en este proveedor.
    /// `None` = no expira (Anthropic, OpenAI).
    /// `Some(d)` = expira en `d` desde uploaded_at (Gemini = 48h).
    fn ttl(&self) -> Option<Duration>;

    /// Identifica al proveedor para keying del cache.
    fn provider(&self) -> ProviderKind;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockProvider;
    #[async_trait]
    impl FileProviderRepository for MockProvider {
        async fn upload_streaming(
            &self,
            _stream: BoxedByteStream,
            mime_type: &str,
            filename: &str,
        ) -> Result<ProviderFileRef, LlmError> {
            Ok(ProviderFileRef {
                provider: ProviderKind::Mock,
                provider_file_id: "mock-id".into(),
                mime_type: mime_type.into(),
                filename: filename.into(),
                expires_at: None,
            })
        }
        fn ttl(&self) -> Option<Duration> { None }
        fn provider(&self) -> ProviderKind { ProviderKind::Mock }
    }

    #[tokio::test]
    async fn mock_provider_returns_ref() {
        let p = MockProvider;
        let stream: BoxedByteStream = Box::pin(futures::stream::empty());
        let r = p.upload_streaming(stream, "application/pdf", "x.pdf").await.unwrap();
        assert_eq!(r.provider_file_id, "mock-id");
        assert_eq!(r.provider, ProviderKind::Mock);
    }
}
```

Modificar `src/libs/colmena/src/llm/domain/mod.rs`:

```rust
pub mod file_provider_repository;
pub use file_provider_repository::{BoxedByteStream, FileProviderRepository};
```

- [ ] **Step 2: Run**

Run: `cargo test --lib file_provider_repository -p colmena_dag_engine`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/llm/domain/file_provider_repository.rs \
        src/libs/colmena/src/llm/domain/mod.rs
git commit -m "feat(llm): add FileProviderRepository port

Defines the trait that file-API adapters implement to upload streams
to provider Files APIs. Includes BoxedByteStream type alias and ttl()
hook for cache expiry."
```

---

### Task 5: Trait `FileCacheRepository` + `CachedFileEntry`

**Files:**
- Create: `src/libs/colmena/src/llm/domain/file_cache_repository.rs`
- Modify: `src/libs/colmena/src/llm/domain/mod.rs`

- [ ] **Step 1: Test que falla**

Crear `src/libs/colmena/src/llm/domain/file_cache_repository.rs`:

```rust
//! Port para persistir referencias a archivos subidos al Files API.
//! Implementación por defecto en `llm/infrastructure/files/postgres_file_cache.rs`.

use crate::llm::domain::{LlmError, ProviderFileRef, ProviderKind};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone)]
pub struct CachedFileEntry {
    pub document_id: String,
    pub provider: ProviderKind,
    pub provider_file_id: String,
    pub mime_type: String,
    pub filename: String,
    pub size_bytes: Option<i64>,
    pub uploaded_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: DateTime<Utc>,
}

impl CachedFileEntry {
    /// Heurística: si tenemos expires_at y ya pasó (menos margen de 5 min),
    /// asumimos expirado sin llamar al provider.
    pub fn is_likely_alive(&self, now: DateTime<Utc>) -> bool {
        match self.expires_at {
            None => true,
            Some(exp) => now < exp - Duration::minutes(5),
        }
    }

    pub fn into_ref(self) -> ProviderFileRef {
        ProviderFileRef {
            provider: self.provider,
            provider_file_id: self.provider_file_id,
            mime_type: self.mime_type,
            filename: self.filename,
            expires_at: self.expires_at,
        }
    }
}

#[async_trait]
pub trait FileCacheRepository: Send + Sync {
    async fn lookup(
        &self,
        document_id: &str,
        provider: ProviderKind,
    ) -> Result<Option<CachedFileEntry>, LlmError>;

    async fn upsert(&self, entry: &CachedFileEntry) -> Result<(), LlmError>;

    async fn invalidate(
        &self,
        document_id: &str,
        provider: ProviderKind,
    ) -> Result<(), LlmError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_with_expiry(expires_at: Option<DateTime<Utc>>) -> CachedFileEntry {
        let now = Utc::now();
        CachedFileEntry {
            document_id: "doc-1".into(),
            provider: ProviderKind::Gemini,
            provider_file_id: "files/abc".into(),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_bytes: Some(1024),
            uploaded_at: now,
            expires_at,
            last_used_at: now,
        }
    }

    #[test]
    fn alive_when_expires_at_is_none() {
        let e = entry_with_expiry(None);
        assert!(e.is_likely_alive(Utc::now()));
    }

    #[test]
    fn alive_when_expires_at_in_future_beyond_margin() {
        let now = Utc::now();
        let e = entry_with_expiry(Some(now + Duration::hours(2)));
        assert!(e.is_likely_alive(now));
    }

    #[test]
    fn expired_when_within_5min_margin() {
        let now = Utc::now();
        let e = entry_with_expiry(Some(now + Duration::minutes(3)));
        assert!(!e.is_likely_alive(now));
    }

    #[test]
    fn expired_when_in_past() {
        let now = Utc::now();
        let e = entry_with_expiry(Some(now - Duration::hours(1)));
        assert!(!e.is_likely_alive(now));
    }

    #[test]
    fn into_ref_preserves_fields() {
        let e = entry_with_expiry(None);
        let r = e.clone().into_ref();
        assert_eq!(r.provider_file_id, e.provider_file_id);
        assert_eq!(r.provider, e.provider);
    }
}
```

Modificar `src/libs/colmena/src/llm/domain/mod.rs`:

```rust
pub mod file_cache_repository;
pub use file_cache_repository::{CachedFileEntry, FileCacheRepository};
```

- [ ] **Step 2: Run**

Run: `cargo test --lib file_cache_repository -p colmena_dag_engine`
Expected: 5 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/llm/domain/file_cache_repository.rs \
        src/libs/colmena/src/llm/domain/mod.rs
git commit -m "feat(llm): add FileCacheRepository port and CachedFileEntry

Defines lookup/upsert/invalidate trait. CachedFileEntry::is_likely_alive
implements the 5-min safety margin heuristic for expires_at."
```

---

## Phase 3: Postgres cache adapter

### Task 6: Migración SQL `provider_file_cache`

**Files:**
- Create: `src/libs/colmena/migrations/postgres/20260502000001_provider_file_cache.sql`

- [ ] **Step 1: Crear migración**

```sql
-- Cache persistido de archivos subidos al Files API de cada proveedor LLM.
-- Permite reutilizar uploads entre ejecuciones de DAG dentro de la misma
-- conversación (Colmena es stateless por ejecución).
-- Ver docs/superpowers/specs/2026-05-02-large-document-files-api-design.md.

CREATE TABLE IF NOT EXISTS provider_file_cache (
    document_id      TEXT        NOT NULL,
    provider         TEXT        NOT NULL,
    provider_file_id TEXT        NOT NULL,
    mime_type        TEXT        NOT NULL,
    filename         TEXT        NOT NULL,
    size_bytes       BIGINT,
    uploaded_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at       TIMESTAMPTZ,
    last_used_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (document_id, provider)
);

CREATE INDEX IF NOT EXISTS idx_provider_file_cache_expires
    ON provider_file_cache (expires_at)
    WHERE expires_at IS NOT NULL;
```

- [ ] **Step 2: Verificar migración compila con sqlx-cli (opcional)**

Si tienes sqlx-cli y un PG corriendo: `sqlx migrate info --source src/libs/colmena/migrations/postgres`. Caso contrario salta a step 3.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/migrations/postgres/20260502000001_provider_file_cache.sql
git commit -m "feat(migrations): add provider_file_cache table

Stores (document_id, provider) -> provider_file_id mappings for
reuse across stateless DAG executions in the same session."
```

---

### Task 7: Adapter `PostgresFileCache`

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/files/mod.rs`
- Create: `src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/mod.rs`

- [ ] **Step 1: Crear el módulo files/**

Crear `src/libs/colmena/src/llm/infrastructure/files/mod.rs`:

```rust
//! Adapters para Files APIs de proveedores LLM y cache persistido.

pub mod postgres_file_cache;
pub use postgres_file_cache::PostgresFileCache;
```

Modificar `src/libs/colmena/src/llm/infrastructure/mod.rs` agregando:

```rust
pub mod files;
```

- [ ] **Step 2: Test de integración con PG real**

Crear `src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs` con tests gated por env var `TEST_DATABASE_URL`:

```rust
//! Implementación Postgres de FileCacheRepository.
//! Usa el `PgPoolRegistry` compartido — la conexión viene siempre
//! de DATABASE_URL (env), independiente de la `connection_url` por nodo
//! que usa el backend de memoria.

use crate::dag_engine::infrastructure::pool_registry::PgPoolRegistry;
use crate::llm::domain::{
    CachedFileEntry, FileCacheRepository, LlmError, ProviderKind,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::str::FromStr;
use std::sync::Arc;

pub struct PostgresFileCache {
    pool: Arc<PgPool>,
}

impl PostgresFileCache {
    pub async fn new(
        registry: Arc<PgPoolRegistry>,
        database_url: &str,
    ) -> Result<Self, LlmError> {
        let pool = registry
            .get_or_create(database_url)
            .await
            .map_err(|e| LlmError::RequestFailed {
                message: format!("Failed to get Postgres pool: {}", e),
            })?;

        Ok(Self { pool })
    }
}

#[async_trait]
impl FileCacheRepository for PostgresFileCache {
    async fn lookup(
        &self,
        document_id: &str,
        provider: ProviderKind,
    ) -> Result<Option<CachedFileEntry>, LlmError> {
        let provider_str = provider.to_string();
        let row = sqlx::query!(
            r#"
            SELECT document_id, provider, provider_file_id, mime_type, filename,
                   size_bytes, uploaded_at, expires_at, last_used_at
              FROM provider_file_cache
             WHERE document_id = $1 AND provider = $2
            "#,
            document_id,
            provider_str
        )
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed {
            message: format!("provider_file_cache lookup failed: {}", e),
        })?;

        Ok(row.map(|r| CachedFileEntry {
            document_id: r.document_id,
            provider: ProviderKind::from_str(&r.provider).unwrap_or(ProviderKind::Mock),
            provider_file_id: r.provider_file_id,
            mime_type: r.mime_type,
            filename: r.filename,
            size_bytes: r.size_bytes,
            uploaded_at: r.uploaded_at,
            expires_at: r.expires_at,
            last_used_at: r.last_used_at,
        }))
    }

    async fn upsert(&self, entry: &CachedFileEntry) -> Result<(), LlmError> {
        let provider_str = entry.provider.to_string();
        sqlx::query!(
            r#"
            INSERT INTO provider_file_cache
                (document_id, provider, provider_file_id, mime_type, filename,
                 size_bytes, uploaded_at, expires_at, last_used_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (document_id, provider) DO UPDATE SET
                provider_file_id = EXCLUDED.provider_file_id,
                mime_type        = EXCLUDED.mime_type,
                filename         = EXCLUDED.filename,
                size_bytes       = EXCLUDED.size_bytes,
                uploaded_at      = EXCLUDED.uploaded_at,
                expires_at       = EXCLUDED.expires_at,
                last_used_at     = NOW()
            "#,
            entry.document_id,
            provider_str,
            entry.provider_file_id,
            entry.mime_type,
            entry.filename,
            entry.size_bytes,
            entry.uploaded_at,
            entry.expires_at,
            entry.last_used_at,
        )
        .execute(&*self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed {
            message: format!("provider_file_cache upsert failed: {}", e),
        })?;
        Ok(())
    }

    async fn invalidate(
        &self,
        document_id: &str,
        provider: ProviderKind,
    ) -> Result<(), LlmError> {
        let provider_str = provider.to_string();
        sqlx::query!(
            r#"DELETE FROM provider_file_cache
                WHERE document_id = $1 AND provider = $2"#,
            document_id,
            provider_str
        )
        .execute(&*self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed {
            message: format!("provider_file_cache invalidate failed: {}", e),
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::infrastructure::pool_registry::PoolConfig;

    /// Helper: crea una instancia con PG real. Requiere TEST_DATABASE_URL.
    /// Skip si no está set.
    async fn with_cache<F, Fut>(f: F)
    where
        F: FnOnce(PostgresFileCache) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let url = match std::env::var("TEST_DATABASE_URL") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("skipping: TEST_DATABASE_URL not set");
                return;
            }
        };
        let registry = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
        // Run migration explicitly.
        let pool = registry.get_or_create(&url).await.unwrap();
        sqlx::migrate!("migrations/postgres")
            .set_ignore_missing(true)
            .run(&*pool)
            .await
            .unwrap();
        let cache = PostgresFileCache::new(registry, &url).await.unwrap();
        // Clean state.
        sqlx::query!("DELETE FROM provider_file_cache WHERE document_id LIKE 'test-%'")
            .execute(&*pool)
            .await
            .unwrap();
        f(cache).await;
    }

    fn fixture(doc_id: &str) -> CachedFileEntry {
        let now = Utc::now();
        CachedFileEntry {
            document_id: doc_id.into(),
            provider: ProviderKind::Anthropic,
            provider_file_id: "file_abc".into(),
            mime_type: "application/pdf".into(),
            filename: "report.pdf".into(),
            size_bytes: Some(2_000_000),
            uploaded_at: now,
            expires_at: None,
            last_used_at: now,
        }
    }

    #[tokio::test]
    async fn lookup_miss_returns_none() {
        with_cache(|cache| async move {
            let r = cache.lookup("test-not-exist", ProviderKind::Anthropic).await.unwrap();
            assert!(r.is_none());
        }).await;
    }

    #[tokio::test]
    async fn upsert_then_lookup_returns_entry() {
        with_cache(|cache| async move {
            let entry = fixture("test-1");
            cache.upsert(&entry).await.unwrap();
            let got = cache.lookup("test-1", ProviderKind::Anthropic).await.unwrap();
            assert!(got.is_some());
            assert_eq!(got.unwrap().provider_file_id, "file_abc");
        }).await;
    }

    #[tokio::test]
    async fn upsert_twice_updates() {
        with_cache(|cache| async move {
            let mut entry = fixture("test-2");
            cache.upsert(&entry).await.unwrap();
            entry.provider_file_id = "file_xyz".into();
            cache.upsert(&entry).await.unwrap();
            let got = cache.lookup("test-2", ProviderKind::Anthropic).await.unwrap().unwrap();
            assert_eq!(got.provider_file_id, "file_xyz");
        }).await;
    }

    #[tokio::test]
    async fn invalidate_removes() {
        with_cache(|cache| async move {
            let entry = fixture("test-3");
            cache.upsert(&entry).await.unwrap();
            cache.invalidate("test-3", ProviderKind::Anthropic).await.unwrap();
            let got = cache.lookup("test-3", ProviderKind::Anthropic).await.unwrap();
            assert!(got.is_none());
        }).await;
    }
}
```

- [ ] **Step 3: Run con PG real**

Pre-requisito: tener PG corriendo y `TEST_DATABASE_URL=postgres://...` exportado.

Run: `cargo test --lib postgres_file_cache -p colmena_dag_engine`
Expected: 4 tests PASS (o skip silencioso si TEST_DATABASE_URL no existe).

- [ ] **Step 4: Verificar build limpio sin DB**

Run: `cargo check -p colmena_dag_engine --lib`
Expected: 0 errores. Las queries `sqlx::query!` requieren `DATABASE_URL` o cache offline en `.sqlx/`. Si falla por ese motivo, ejecutar `cargo sqlx prepare` con DB activa para generar el cache, y commitearlo.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/files/mod.rs \
        src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs \
        src/libs/colmena/src/llm/infrastructure/mod.rs \
        .sqlx/  # si se regeneró
git commit -m "feat(llm): add PostgresFileCache adapter

Implements FileCacheRepository against the existing PgPoolRegistry.
Always uses DATABASE_URL (transversal cache, not per-node)."
```

---

## Phase 4: HTTP signed URL downloader

### Task 8: `SignedUrlDownloader`

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/files/signed_url_downloader.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/files/mod.rs`

- [ ] **Step 1: Tests con wiremock**

Crear `src/libs/colmena/src/llm/infrastructure/files/signed_url_downloader.rs`:

```rust
//! Descarga streaming de signed URLs (GCS) sin Authorization header.
//! La firma viaja en query params; añadir Authorization invalidaría la firma.

use crate::llm::domain::{BoxedByteStream, LlmError};
use bytes::Bytes;
use futures::TryStreamExt;
use reqwest::Client;

pub struct SignedUrlDownloader {
    client: Client,
}

impl SignedUrlDownloader {
    pub fn new() -> Self {
        Self { client: Client::new() }
    }

    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    pub async fn stream(&self, url: &str) -> Result<BoxedByteStream, LlmError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError {
                message: format!("signed URL fetch failed: {}", e),
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(LlmError::SignedUrlFetchFailed {
                status: status.as_u16(),
            });
        }

        let stream = response
            .bytes_stream()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));

        Ok(Box::pin(stream))
    }
}

impl Default for SignedUrlDownloader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn stream_returns_body_chunks_on_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/file.pdf"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello world"))
            .mount(&server)
            .await;

        let downloader = SignedUrlDownloader::new();
        let url = format!("{}/file.pdf?sig=x", server.uri());
        let mut stream = downloader.stream(&url).await.unwrap();

        let mut all = Vec::new();
        while let Some(chunk) = stream.next().await {
            all.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(all, b"hello world");
    }

    #[tokio::test]
    async fn stream_errors_on_403() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/expired.pdf"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let downloader = SignedUrlDownloader::new();
        let url = format!("{}/expired.pdf", server.uri());
        let err = downloader.stream(&url).await.unwrap_err();
        assert!(matches!(err, LlmError::SignedUrlFetchFailed { status: 403 }));
    }

    #[tokio::test]
    async fn stream_errors_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing.pdf"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let downloader = SignedUrlDownloader::new();
        let url = format!("{}/missing.pdf", server.uri());
        let err = downloader.stream(&url).await.unwrap_err();
        assert!(matches!(err, LlmError::SignedUrlFetchFailed { status: 404 }));
    }

    #[tokio::test]
    async fn stream_does_not_send_authorization() {
        // Wiremock no inyecta auth; el request lo construye downloader.
        // Validamos que ningún Authorization header viaje verificando
        // que el match exige ausencia. Truco: usamos `header_exists`.
        use wiremock::matchers::header_exists;
        let server = MockServer::start().await;
        // Si Authorization existiera, este match fallaría (404 default).
        Mock::given(method("GET"))
            .and(path("/no-auth.pdf"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok"))
            .mount(&server)
            .await;

        // Negative match (header_exists) sería 0 hits; comprobamos directo:
        let downloader = SignedUrlDownloader::new();
        let url = format!("{}/no-auth.pdf", server.uri());
        let result = downloader.stream(&url).await;
        assert!(result.is_ok());
        // Validar via received requests:
        let received = server.received_requests().await.unwrap();
        let req = received.iter().find(|r| r.url.path() == "/no-auth.pdf").unwrap();
        assert!(req.headers.get("authorization").is_none());
    }
}
```

Modificar `src/libs/colmena/src/llm/infrastructure/files/mod.rs`:

```rust
pub mod postgres_file_cache;
pub mod signed_url_downloader;

pub use postgres_file_cache::PostgresFileCache;
pub use signed_url_downloader::SignedUrlDownloader;
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib signed_url_downloader -p colmena_dag_engine`
Expected: 4 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/files/signed_url_downloader.rs \
        src/libs/colmena/src/llm/infrastructure/files/mod.rs
git commit -m "feat(llm): add SignedUrlDownloader for GCS streaming

HTTP GET to signed URLs without Authorization header (signature is in
query params). Returns BoxedByteStream for pipe-to-upload. Errors mapped
to LlmError::SignedUrlFetchFailed with status code."
```

---

## Phase 5: Files API adapters por proveedor

### Task 9: `AnthropicFilesApiAdapter`

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/files/anthropic_files_api.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/files/mod.rs`

- [ ] **Step 1: Tests con wiremock**

Crear `src/libs/colmena/src/llm/infrastructure/files/anthropic_files_api.rs`:

```rust
//! Files API de Anthropic (beta).
//! Header obligatorio: `anthropic-beta: files-api-2025-04-14`.
//! Multipart/form-data con un único POST.

use crate::llm::domain::{
    BoxedByteStream, FileProviderRepository, LlmError, ProviderFileRef, ProviderKind,
};
use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use reqwest::{Body, Client};
use serde::Deserialize;
use std::time::Duration;

const BETA_HEADER: &str = "files-api-2025-04-14";

pub struct AnthropicFilesApiAdapter {
    client: Client,
    base_url: String,
    api_key: String,
}

impl AnthropicFilesApiAdapter {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key,
        }
    }

    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
            api_key,
        }
    }
}

#[derive(Debug, Deserialize)]
struct UploadResponse {
    id: String,
}

#[async_trait]
impl FileProviderRepository for AnthropicFilesApiAdapter {
    async fn upload_streaming(
        &self,
        stream: BoxedByteStream,
        mime_type: &str,
        filename: &str,
    ) -> Result<ProviderFileRef, LlmError> {
        let body = Body::wrap_stream(stream);
        let part = Part::stream(body)
            .file_name(filename.to_string())
            .mime_str(mime_type)
            .map_err(|e| LlmError::FileApiUploadFailed {
                provider: "anthropic".into(),
                message: format!("invalid mime: {}", e),
            })?;

        let form = Form::new().part("file", part);
        let url = format!("{}/v1/files", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", BETA_HEADER)
            .multipart(form)
            .send()
            .await
            .map_err(|e| LlmError::FileApiUploadFailed {
                provider: "anthropic".into(),
                message: e.to_string(),
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::FileApiUploadFailed {
                provider: "anthropic".into(),
                message: format!("HTTP {}: {}", status, body),
            });
        }

        let parsed: UploadResponse = response.json().await.map_err(|e| {
            LlmError::FileApiUploadFailed {
                provider: "anthropic".into(),
                message: format!("invalid JSON response: {}", e),
            }
        })?;

        Ok(ProviderFileRef {
            provider: ProviderKind::Anthropic,
            provider_file_id: parsed.id,
            mime_type: mime_type.to_string(),
            filename: filename.to_string(),
            expires_at: None,
        })
    }

    fn ttl(&self) -> Option<Duration> {
        None
    }

    fn provider(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::stream;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fake_stream(content: &[u8]) -> BoxedByteStream {
        let bytes = Bytes::copy_from_slice(content);
        Box::pin(stream::iter(vec![Ok::<_, std::io::Error>(bytes)]))
    }

    #[tokio::test]
    async fn upload_succeeds_returns_file_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/files"))
            .and(header("x-api-key", "test-key"))
            .and(header("anthropic-beta", BETA_HEADER))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "file_01abc",
                "type": "file",
                "filename": "x.pdf"
            })))
            .mount(&server)
            .await;

        let adapter = AnthropicFilesApiAdapter::with_base_url(
            "test-key".into(),
            server.uri(),
        );
        let r = adapter
            .upload_streaming(fake_stream(b"PDF-CONTENT"), "application/pdf", "x.pdf")
            .await
            .unwrap();
        assert_eq!(r.provider_file_id, "file_01abc");
        assert_eq!(r.provider, ProviderKind::Anthropic);
        assert!(r.expires_at.is_none());
    }

    #[tokio::test]
    async fn upload_fails_on_413() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/files"))
            .respond_with(ResponseTemplate::new(413).set_body_string("too large"))
            .mount(&server)
            .await;

        let adapter = AnthropicFilesApiAdapter::with_base_url(
            "k".into(),
            server.uri(),
        );
        let err = adapter
            .upload_streaming(fake_stream(b"data"), "application/pdf", "x.pdf")
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::FileApiUploadFailed { .. }));
    }

    #[test]
    fn ttl_is_none() {
        let a = AnthropicFilesApiAdapter::new("k".into());
        assert!(a.ttl().is_none());
    }
}
```

Modificar `src/libs/colmena/src/llm/infrastructure/files/mod.rs` agregando:

```rust
pub mod anthropic_files_api;
pub use anthropic_files_api::AnthropicFilesApiAdapter;
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib anthropic_files_api -p colmena_dag_engine`
Expected: 3 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/files/anthropic_files_api.rs \
        src/libs/colmena/src/llm/infrastructure/files/mod.rs
git commit -m "feat(llm): add AnthropicFilesApiAdapter

Multipart upload with anthropic-beta: files-api-2025-04-14 header.
ttl()=None (no expiry). Streams body via reqwest::Body::wrap_stream."
```

---

### Task 10: `OpenAiFilesApiAdapter`

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/files/openai_files_api.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/files/mod.rs`

- [ ] **Step 1: Tests + implementación**

Crear `src/libs/colmena/src/llm/infrastructure/files/openai_files_api.rs`:

```rust
//! Files API de OpenAI.
//! POST /v1/files multipart con purpose=user_data (para uso con
//! responses/chat.completions referenciando file_id).

use crate::llm::domain::{
    BoxedByteStream, FileProviderRepository, LlmError, ProviderFileRef, ProviderKind,
};
use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use reqwest::{Body, Client};
use serde::Deserialize;
use std::time::Duration;

pub struct OpenAiFilesApiAdapter {
    client: Client,
    base_url: String,
    api_key: String,
}

impl OpenAiFilesApiAdapter {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            base_url: "https://api.openai.com".into(),
            api_key,
        }
    }

    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
            api_key,
        }
    }
}

#[derive(Debug, Deserialize)]
struct UploadResponse {
    id: String,
}

#[async_trait]
impl FileProviderRepository for OpenAiFilesApiAdapter {
    async fn upload_streaming(
        &self,
        stream: BoxedByteStream,
        mime_type: &str,
        filename: &str,
    ) -> Result<ProviderFileRef, LlmError> {
        let body = Body::wrap_stream(stream);
        let file_part = Part::stream(body)
            .file_name(filename.to_string())
            .mime_str(mime_type)
            .map_err(|e| LlmError::FileApiUploadFailed {
                provider: "openai".into(),
                message: format!("invalid mime: {}", e),
            })?;

        let form = Form::new()
            .text("purpose", "user_data")
            .part("file", file_part);

        let url = format!("{}/v1/files", self.base_url);
        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| LlmError::FileApiUploadFailed {
                provider: "openai".into(),
                message: e.to_string(),
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::FileApiUploadFailed {
                provider: "openai".into(),
                message: format!("HTTP {}: {}", status, body),
            });
        }

        let parsed: UploadResponse = response.json().await.map_err(|e| {
            LlmError::FileApiUploadFailed {
                provider: "openai".into(),
                message: format!("invalid JSON response: {}", e),
            }
        })?;

        Ok(ProviderFileRef {
            provider: ProviderKind::OpenAi,
            provider_file_id: parsed.id,
            mime_type: mime_type.to_string(),
            filename: filename.to_string(),
            expires_at: None,
        })
    }

    fn ttl(&self) -> Option<Duration> {
        None
    }

    fn provider(&self) -> ProviderKind {
        ProviderKind::OpenAi
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::stream;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fake_stream(content: &[u8]) -> BoxedByteStream {
        let bytes = Bytes::copy_from_slice(content);
        Box::pin(stream::iter(vec![Ok::<_, std::io::Error>(bytes)]))
    }

    #[tokio::test]
    async fn upload_succeeds_with_bearer_and_purpose() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/files"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "file-abc123",
                "object": "file",
                "purpose": "user_data"
            })))
            .mount(&server)
            .await;

        let adapter = OpenAiFilesApiAdapter::with_base_url(
            "sk-test".into(),
            server.uri(),
        );
        let r = adapter
            .upload_streaming(fake_stream(b"PDF"), "application/pdf", "x.pdf")
            .await
            .unwrap();
        assert_eq!(r.provider_file_id, "file-abc123");
        assert_eq!(r.provider, ProviderKind::OpenAi);
    }

    #[tokio::test]
    async fn upload_errors_on_400() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/files"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad"))
            .mount(&server)
            .await;

        let adapter = OpenAiFilesApiAdapter::with_base_url("k".into(), server.uri());
        let err = adapter
            .upload_streaming(fake_stream(b"x"), "application/pdf", "x.pdf")
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::FileApiUploadFailed { .. }));
    }
}
```

Modificar `src/libs/colmena/src/llm/infrastructure/files/mod.rs`:

```rust
pub mod openai_files_api;
pub use openai_files_api::OpenAiFilesApiAdapter;
```

- [ ] **Step 2: Run**

Run: `cargo test --lib openai_files_api -p colmena_dag_engine`
Expected: 2 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/files/openai_files_api.rs \
        src/libs/colmena/src/llm/infrastructure/files/mod.rs
git commit -m "feat(llm): add OpenAiFilesApiAdapter

Multipart POST to /v1/files with purpose=user_data and bearer auth.
ttl()=None."
```

---

### Task 11: `GeminiFilesApiAdapter` (resumable upload)

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/files/gemini_files_api.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/files/mod.rs`

- [ ] **Step 1: Implementación + tests**

Crear `src/libs/colmena/src/llm/infrastructure/files/gemini_files_api.rs`:

```rust
//! Files API de Gemini con resumable upload.
//! Tres fases:
//!   1) POST /upload/v1beta/files con headers X-Goog-Upload-* para iniciar.
//!      Respuesta incluye header X-Goog-Upload-URL.
//!   2) PUT(s) sobre upload_url con chunks (X-Goog-Upload-Offset / Command).
//!   3) Último PUT con command "upload, finalize" devuelve el File metadata.

use crate::llm::domain::{
    BoxedByteStream, FileProviderRepository, LlmError, ProviderFileRef, ProviderKind,
};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use chrono::{Duration as ChronoDuration, Utc};
use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const CHUNK_SIZE: usize = 8 * 1024 * 1024; // 8 MB

pub struct GeminiFilesApiAdapter {
    client: Client,
    base_url: String,
    api_key: String,
}

impl GeminiFilesApiAdapter {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            base_url: "https://generativelanguage.googleapis.com".into(),
            api_key,
        }
    }

    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
            api_key,
        }
    }

    async fn start_session(
        &self,
        mime_type: &str,
        filename: &str,
    ) -> Result<String, LlmError> {
        let url = format!(
            "{}/upload/v1beta/files?key={}",
            self.base_url, self.api_key
        );
        let resp = self
            .client
            .post(&url)
            .header("X-Goog-Upload-Protocol", "resumable")
            .header("X-Goog-Upload-Command", "start")
            .header("X-Goog-Upload-Header-Content-Type", mime_type)
            .header("Content-Type", "application/json")
            .json(&json!({ "file": { "display_name": filename } }))
            .send()
            .await
            .map_err(|e| LlmError::FileApiUploadFailed {
                provider: "gemini".into(),
                message: format!("session start failed: {}", e),
            })?;

        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(LlmError::FileApiUploadFailed {
                provider: "gemini".into(),
                message: format!("session start HTTP {}: {}", s, b),
            });
        }

        resp.headers()
            .get("X-Goog-Upload-URL")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| LlmError::FileApiUploadFailed {
                provider: "gemini".into(),
                message: "missing X-Goog-Upload-URL header in start response".into(),
            })
    }

    async fn put_chunk(
        &self,
        upload_url: &str,
        offset: u64,
        chunk: Bytes,
        finalize: bool,
    ) -> Result<Option<UploadFinalizeResponse>, LlmError> {
        let cmd = if finalize { "upload, finalize" } else { "upload" };
        let resp = self
            .client
            .put(upload_url)
            .header("X-Goog-Upload-Offset", offset.to_string())
            .header("X-Goog-Upload-Command", cmd)
            .header("Content-Length", chunk.len().to_string())
            .body(chunk)
            .send()
            .await
            .map_err(|e| LlmError::FileApiUploadFailed {
                provider: "gemini".into(),
                message: format!("PUT chunk failed at offset {}: {}", offset, e),
            })?;

        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(LlmError::FileApiUploadFailed {
                provider: "gemini".into(),
                message: format!("PUT chunk HTTP {}: {}", s, b),
            });
        }

        if finalize {
            let parsed: UploadFinalizeResponse = resp.json().await.map_err(|e| {
                LlmError::FileApiUploadFailed {
                    provider: "gemini".into(),
                    message: format!("invalid finalize JSON: {}", e),
                }
            })?;
            Ok(Some(parsed))
        } else {
            Ok(None)
        }
    }
}

#[derive(Debug, Deserialize)]
struct UploadFinalizeResponse {
    file: GeminiFile,
}

#[derive(Debug, Deserialize)]
struct GeminiFile {
    /// Forma "files/abc123".
    name: String,
}

#[async_trait]
impl FileProviderRepository for GeminiFilesApiAdapter {
    async fn upload_streaming(
        &self,
        mut stream: BoxedByteStream,
        mime_type: &str,
        filename: &str,
    ) -> Result<ProviderFileRef, LlmError> {
        // 1. Iniciar sesión.
        let upload_url = self.start_session(mime_type, filename).await?;

        // 2. Subir chunks.
        let mut buffer = BytesMut::with_capacity(CHUNK_SIZE);
        let mut offset: u64 = 0;
        let mut finalize_response: Option<UploadFinalizeResponse> = None;
        let mut stream_done = false;

        while !stream_done {
            // Acumular hasta CHUNK_SIZE o fin de stream.
            while buffer.len() < CHUNK_SIZE {
                match stream.next().await {
                    Some(Ok(b)) => buffer.extend_from_slice(&b),
                    Some(Err(e)) => {
                        return Err(LlmError::FileApiUploadFailed {
                            provider: "gemini".into(),
                            message: format!("stream read error: {}", e),
                        });
                    }
                    None => {
                        stream_done = true;
                        break;
                    }
                }
            }

            // Si no quedan bytes y ya hicimos al menos un chunk, marcar finalize en último.
            // Si nunca hubo bytes (archivo vacío), aún hay que mandar un chunk vacío con finalize.
            let chunk_bytes = buffer.split().freeze();
            let chunk_len = chunk_bytes.len() as u64;
            let is_last = stream_done;

            let result = self
                .put_chunk(&upload_url, offset, chunk_bytes, is_last)
                .await?;

            if is_last {
                finalize_response = result;
            }
            offset += chunk_len;
        }

        let resp = finalize_response.ok_or_else(|| LlmError::FileApiUploadFailed {
            provider: "gemini".into(),
            message: "finalize response missing".into(),
        })?;

        // file.name viene como "files/abc123".
        let file_uri = format!("{}/v1beta/{}", self.base_url, resp.file.name);

        Ok(ProviderFileRef {
            provider: ProviderKind::Gemini,
            provider_file_id: file_uri,
            mime_type: mime_type.to_string(),
            filename: filename.to_string(),
            expires_at: Some(Utc::now() + ChronoDuration::hours(48)),
        })
    }

    fn ttl(&self) -> Option<Duration> {
        Some(Duration::from_secs(48 * 3600))
    }

    fn provider(&self) -> ProviderKind {
        ProviderKind::Gemini
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::stream;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fake_stream(content: &[u8]) -> BoxedByteStream {
        let bytes = Bytes::copy_from_slice(content);
        Box::pin(stream::iter(vec![Ok::<_, std::io::Error>(bytes)]))
    }

    #[tokio::test]
    async fn small_file_one_chunk_uploads() {
        let server = MockServer::start().await;

        // 1. Init session, mock returns upload URL.
        let upload_path = "/upload-session-xyz";
        let upload_url = format!("{}{}", server.uri(), upload_path);
        Mock::given(method("POST"))
            .and(path("/upload/v1beta/files"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("X-Goog-Upload-URL", upload_url.as_str())
                    .set_body_string(""),
            )
            .mount(&server)
            .await;

        // 2. Finalize PUT returns file metadata.
        Mock::given(method("PUT"))
            .and(path(upload_path))
            .and(header("X-Goog-Upload-Command", "upload, finalize"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "file": { "name": "files/abcd1234" }
            })))
            .mount(&server)
            .await;

        let adapter =
            GeminiFilesApiAdapter::with_base_url("KEY".into(), server.uri());
        let r = adapter
            .upload_streaming(fake_stream(b"hello world"), "application/pdf", "x.pdf")
            .await
            .unwrap();
        assert!(r.provider_file_id.contains("files/abcd1234"));
        assert_eq!(r.provider, ProviderKind::Gemini);
        assert!(r.expires_at.is_some());
    }

    #[tokio::test]
    async fn session_start_failure_propagates() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/upload/v1beta/files"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .mount(&server)
            .await;

        let adapter =
            GeminiFilesApiAdapter::with_base_url("KEY".into(), server.uri());
        let err = adapter
            .upload_streaming(fake_stream(b"x"), "application/pdf", "x.pdf")
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::FileApiUploadFailed { .. }));
    }

    #[test]
    fn ttl_is_48h() {
        let a = GeminiFilesApiAdapter::new("k".into());
        assert_eq!(a.ttl(), Some(Duration::from_secs(48 * 3600)));
    }
}
```

Modificar `src/libs/colmena/src/llm/infrastructure/files/mod.rs`:

```rust
pub mod gemini_files_api;
pub use gemini_files_api::GeminiFilesApiAdapter;
```

- [ ] **Step 2: Run**

Run: `cargo test --lib gemini_files_api -p colmena_dag_engine`
Expected: 3 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/files/gemini_files_api.rs \
        src/libs/colmena/src/llm/infrastructure/files/mod.rs
git commit -m "feat(llm): add GeminiFilesApiAdapter with resumable upload

Three-phase upload: session start, chunked PUTs (8 MB), finalize.
Returns file_uri with 48h ttl."
```

---

## Phase 6: File provider factory

### Task 12: `FileProviderFactory`

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/files/mod.rs`

- [ ] **Step 1: Implementación + tests**

Crear `src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs`:

```rust
//! Factory hermana de LlmProviderFactory que compone los adapters
//! de Files API por proveedor. Se mantiene separada para que
//! cambios en uno no toquen al otro.

use crate::llm::domain::{FileProviderRepository, LlmError, ProviderKind};
use crate::llm::infrastructure::files::{
    AnthropicFilesApiAdapter, GeminiFilesApiAdapter, OpenAiFilesApiAdapter,
};
use std::sync::Arc;

pub struct FileProviderFactory;

impl FileProviderFactory {
    pub fn create(
        kind: ProviderKind,
        api_key: String,
    ) -> Result<Arc<dyn FileProviderRepository>, LlmError> {
        match kind {
            ProviderKind::Anthropic => {
                Ok(Arc::new(AnthropicFilesApiAdapter::new(api_key)))
            }
            ProviderKind::OpenAi => Ok(Arc::new(OpenAiFilesApiAdapter::new(api_key))),
            ProviderKind::Gemini => Ok(Arc::new(GeminiFilesApiAdapter::new(api_key))),
            ProviderKind::Mock => Err(LlmError::ProviderLimitation {
                provider: "mock".into(),
                feature: "Files API".into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_anthropic() {
        let r = FileProviderFactory::create(ProviderKind::Anthropic, "k".into()).unwrap();
        assert_eq!(r.provider(), ProviderKind::Anthropic);
    }

    #[test]
    fn creates_openai() {
        let r = FileProviderFactory::create(ProviderKind::OpenAi, "k".into()).unwrap();
        assert_eq!(r.provider(), ProviderKind::OpenAi);
    }

    #[test]
    fn creates_gemini() {
        let r = FileProviderFactory::create(ProviderKind::Gemini, "k".into()).unwrap();
        assert_eq!(r.provider(), ProviderKind::Gemini);
    }

    #[test]
    fn rejects_mock() {
        let r = FileProviderFactory::create(ProviderKind::Mock, "k".into());
        assert!(r.is_err());
    }
}
```

Modificar `src/libs/colmena/src/llm/infrastructure/files/mod.rs`:

```rust
pub mod file_provider_factory;
pub use file_provider_factory::FileProviderFactory;
```

- [ ] **Step 2: Run**

Run: `cargo test --lib file_provider_factory -p colmena_dag_engine`
Expected: 4 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs \
        src/libs/colmena/src/llm/infrastructure/files/mod.rs
git commit -m "feat(llm): add FileProviderFactory

Composes AnthropicFilesApiAdapter, OpenAiFilesApiAdapter, and
GeminiFilesApiAdapter behind FileProviderRepository. Separate from
LlmProviderFactory for maintainability."
```

---

## Phase 7: LLM adapters reciben `Uploaded`

> **Contexto importante**: cada `*_adapter.rs` actualmente itera `message.files()` y embebe los bytes como base64 inline. Tras este cambio, también deben aceptar `FileSource::Uploaded(ref)` y construir la referencia correcta en el formato del proveedor.

### Task 13: Modificar `anthropic_adapter.rs`

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/anthropic_adapter.rs:50-90` (la región que serializa archivos)

- [ ] **Step 1: Inspeccionar la región actual**

Run: `grep -n "FileData\|inline_data\|base64\|file.bytes" src/libs/colmena/src/llm/infrastructure/anthropic_adapter.rs | head -20`

Localizar el bloque que itera archivos y emite el bloque `document/source`. La forma actual produce `{ type: "document", source: { type: "base64", media_type, data } }`.

- [ ] **Step 2: Test que falla**

Append al `mod tests` de `anthropic_adapter.rs`:

```rust
#[test]
fn serializes_uploaded_file_as_file_id() {
    use crate::llm::domain::{FileData, FileSource, ProviderFileRef, ProviderKind};
    let file = FileData {
        document_id: Some("doc-1".into()),
        mime_type: "application/pdf".into(),
        filename: "x.pdf".into(),
        size_hint: None,
        source: FileSource::Uploaded(ProviderFileRef {
            provider: ProviderKind::Anthropic,
            provider_file_id: "file_01abc".into(),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            expires_at: None,
        }),
    };
    let serialized = AnthropicAdapter::serialize_file_block(&file).unwrap();
    let json = serde_json::to_value(&serialized).unwrap();
    assert_eq!(json["type"], "document");
    assert_eq!(json["source"]["type"], "file");
    assert_eq!(json["source"]["file_id"], "file_01abc");
}

#[test]
fn serializes_inline_file_as_base64() {
    use crate::llm::domain::FileData;
    let file = FileData::inline(
        "application/pdf".into(),
        "x.pdf".into(),
        b"hello".to_vec(),
    );
    let serialized = AnthropicAdapter::serialize_file_block(&file).unwrap();
    let json = serde_json::to_value(&serialized).unwrap();
    assert_eq!(json["type"], "document");
    assert_eq!(json["source"]["type"], "base64");
}
```

- [ ] **Step 3: Run y verificar falla**

Run: `cargo test --lib anthropic_adapter::tests::serializes -p colmena_dag_engine`
Expected: errores de "no method serialize_file_block".

- [ ] **Step 4: Implementar**

Refactorizar el bloque actual de archivos en `anthropic_adapter.rs` extrayendo un método `pub(crate) fn serialize_file_block(file: &FileData) -> Result<AnthropicContentBlock, LlmError>`. Cambiar el match para soportar las 3 variantes:

```rust
fn serialize_file_block(file: &FileData) -> Result<AnthropicContentBlock, LlmError> {
    use crate::llm::domain::FileSource;
    match &file.source {
        FileSource::InlineBytes { bytes } => {
            use base64::{engine::general_purpose::STANDARD, Engine as _};
            let b64 = STANDARD.encode(bytes);
            Ok(AnthropicContentBlock {
                content_type: "document".into(),
                source: Some(AnthropicSource {
                    source_type: "base64".into(),
                    media_type: Some(file.mime_type.clone()),
                    data: Some(b64),
                    file_id: None,
                }),
                ..Default::default()
            })
        }
        FileSource::Uploaded(r) => Ok(AnthropicContentBlock {
            content_type: "document".into(),
            source: Some(AnthropicSource {
                source_type: "file".into(),
                media_type: None,
                data: None,
                file_id: Some(r.provider_file_id.clone()),
            }),
            ..Default::default()
        }),
        FileSource::SignedUrl(_) => Err(LlmError::InternalError {
            message: "SignedUrl source must be resolved to Uploaded \
                     before reaching adapter".into(),
        }),
    }
}
```

Ajustar las structs `AnthropicSource` y `AnthropicContentBlock` para incluir `file_id: Option<String>` (con `#[serde(skip_serializing_if = "Option::is_none")]`). Hacer `media_type` y `data` `Option`.

Reemplazar el call-site original de la serialización iterando con el nuevo método:

```rust
for file in msg.files().unwrap_or(&[]) {
    blocks.push(Self::serialize_file_block(file)?);
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib anthropic_adapter -p colmena_dag_engine`
Expected: tests existentes + 2 nuevos pasan.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/anthropic_adapter.rs
git commit -m "feat(llm/anthropic): support FileSource::Uploaded with file_id

Adapter now emits source.type=\"file\" with file_id when the source is
Uploaded. SignedUrl sources reaching the adapter raise InternalError;
they must be resolved to Uploaded by the use case beforehand."
```

---

### Task 14: Modificar `openai_adapter.rs`

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/openai_adapter.rs`

- [ ] **Step 1: Test que falla**

Append al `mod tests`:

```rust
#[test]
fn openai_serializes_uploaded_as_input_file() {
    use crate::llm::domain::{FileData, FileSource, ProviderFileRef, ProviderKind};
    let file = FileData {
        document_id: Some("doc-1".into()),
        mime_type: "application/pdf".into(),
        filename: "x.pdf".into(),
        size_hint: None,
        source: FileSource::Uploaded(ProviderFileRef {
            provider: ProviderKind::OpenAi,
            provider_file_id: "file-abc".into(),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            expires_at: None,
        }),
    };
    let part = OpenAiAdapter::serialize_file_part(&file).unwrap();
    let json = serde_json::to_value(&part).unwrap();
    assert_eq!(json["type"], "input_file");
    assert_eq!(json["file_id"], "file-abc");
}

#[test]
fn openai_serializes_inline_as_data_uri() {
    use crate::llm::domain::FileData;
    let file = FileData::inline(
        "image/png".into(),
        "x.png".into(),
        b"PNG".to_vec(),
    );
    let part = OpenAiAdapter::serialize_file_part(&file).unwrap();
    let json = serde_json::to_value(&part).unwrap();
    // Tipo varía por mime; verificar que conserva data inline.
    assert!(json.to_string().contains("data:image/png;base64"));
}
```

- [ ] **Step 2: Run**

Run: `cargo test --lib openai_adapter::tests::openai_serializes -p colmena_dag_engine`
Expected: errores de método inexistente.

- [ ] **Step 3: Implementar**

Extraer la lógica de serialización del file inline actual en `openai_adapter.rs:54-..` a un método `pub(crate) fn serialize_file_part(file: &FileData) -> Result<OpenAiContentPart, LlmError>`. Soportar las 3 variantes:

```rust
fn serialize_file_part(file: &FileData) -> Result<OpenAiContentPart, LlmError> {
    use crate::llm::domain::FileSource;
    match &file.source {
        FileSource::InlineBytes { bytes } => {
            use base64::{engine::general_purpose::STANDARD, Engine as _};
            let b64 = STANDARD.encode(bytes);
            let data_uri = format!("data:{};base64,{}", file.mime_type, b64);
            // Imágenes vs documentos:
            if file.mime_type.starts_with("image/") {
                Ok(OpenAiContentPart::ImageUrl {
                    image_url: OpenAiImageUrl { url: data_uri },
                })
            } else {
                Ok(OpenAiContentPart::InputFile {
                    file_data: Some(data_uri),
                    file_id: None,
                    filename: Some(file.filename.clone()),
                })
            }
        }
        FileSource::Uploaded(r) => Ok(OpenAiContentPart::InputFile {
            file_data: None,
            file_id: Some(r.provider_file_id.clone()),
            filename: Some(r.filename.clone()),
        }),
        FileSource::SignedUrl(_) => Err(LlmError::InternalError {
            message: "SignedUrl must be resolved to Uploaded before adapter".into(),
        }),
    }
}
```

Ajustar el enum `OpenAiContentPart` para tener variant `InputFile` con campos opcionales (`file_data` o `file_id`).

Reemplazar el call-site iterativo por `Self::serialize_file_part(file)?`.

- [ ] **Step 4: Run**

Run: `cargo test --lib openai_adapter -p colmena_dag_engine`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/openai_adapter.rs
git commit -m "feat(llm/openai): support FileSource::Uploaded with file_id

input_file content part with file_id when uploaded; data URI when
inline. SignedUrl reaching the adapter is an internal bug."
```

---

### Task 15: Modificar `gemini_adapter.rs`

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs`

- [ ] **Step 1: Test que falla**

Append al `mod tests`:

```rust
#[test]
fn gemini_serializes_uploaded_as_file_data() {
    use crate::llm::domain::{FileData, FileSource, ProviderFileRef, ProviderKind};
    let file = FileData {
        document_id: Some("doc-1".into()),
        mime_type: "application/pdf".into(),
        filename: "x.pdf".into(),
        size_hint: None,
        source: FileSource::Uploaded(ProviderFileRef {
            provider: ProviderKind::Gemini,
            provider_file_id:
                "https://generativelanguage.googleapis.com/v1beta/files/abc".into(),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            expires_at: None,
        }),
    };
    let part = GeminiAdapter::serialize_file_part(&file).unwrap();
    let json = serde_json::to_value(&part).unwrap();
    assert!(json["file_data"].is_object());
    assert_eq!(json["file_data"]["mime_type"], "application/pdf");
    assert_eq!(
        json["file_data"]["file_uri"],
        "https://generativelanguage.googleapis.com/v1beta/files/abc"
    );
}

#[test]
fn gemini_serializes_inline_as_inline_data() {
    use crate::llm::domain::FileData;
    let file = FileData::inline(
        "application/pdf".into(),
        "x.pdf".into(),
        b"PDF".to_vec(),
    );
    let part = GeminiAdapter::serialize_file_part(&file).unwrap();
    let json = serde_json::to_value(&part).unwrap();
    assert!(json["inline_data"].is_object());
    assert_eq!(json["inline_data"]["mime_type"], "application/pdf");
}
```

- [ ] **Step 2: Run y verificar falla**

Run: `cargo test --lib gemini_adapter::tests::gemini_serializes -p colmena_dag_engine`

- [ ] **Step 3: Implementar**

Extraer la serialización de archivo en `gemini_adapter.rs` (línea ~64-90) a un método `pub(crate) fn serialize_file_part(file: &FileData) -> Result<GeminiPart, LlmError>`. Las 3 ramas:

```rust
fn serialize_file_part(file: &FileData) -> Result<GeminiPart, LlmError> {
    use crate::llm::domain::FileSource;
    match &file.source {
        FileSource::InlineBytes { bytes } => {
            use base64::{engine::general_purpose::STANDARD, Engine as _};
            Ok(GeminiPart {
                inline_data: Some(GeminiInlineData {
                    mime_type: file.mime_type.clone(),
                    data: STANDARD.encode(bytes),
                }),
                file_data: None,
                text: None,
                ..Default::default()
            })
        }
        FileSource::Uploaded(r) => Ok(GeminiPart {
            inline_data: None,
            file_data: Some(GeminiFileDataPart {
                mime_type: r.mime_type.clone(),
                file_uri: r.provider_file_id.clone(),
            }),
            text: None,
            ..Default::default()
        }),
        FileSource::SignedUrl(_) => Err(LlmError::InternalError {
            message: "SignedUrl must be resolved to Uploaded before adapter".into(),
        }),
    }
}
```

Agregar struct `GeminiFileDataPart { mime_type, file_uri }` con `#[derive(Serialize, Deserialize, Default)]`. Agregar al `GeminiPart` el campo `file_data: Option<GeminiFileDataPart>` con `#[serde(skip_serializing_if = "Option::is_none", rename = "file_data")]`.

Reemplazar call-sites iterativos por `Self::serialize_file_part(file)?`.

- [ ] **Step 4: Run**

Run: `cargo test --lib gemini_adapter -p colmena_dag_engine`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs
git commit -m "feat(llm/gemini): support FileSource::Uploaded as file_data part

Emits {file_data: {mime_type, file_uri}} when uploaded; inline_data
otherwise. SignedUrl reaching the adapter is an internal bug."
```

---

## Phase 8: Use case integration

### Task 16: Resolución de archivos en `LlmCallUseCase`

> **Contexto:** la resolución vive en el USE CASE, antes de llamar al `LlmRepository`. El use case recibe un `FileCacheRepository` y, según el `ProviderKind`, escoge un `FileProviderRepository` vía `FileProviderFactory`. El `SignedUrlDownloader` lo crea internamente.

**Files:**
- Modify: `src/libs/colmena/src/llm/application/llm_call_use_case.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (inyección)

- [ ] **Step 1: Identificar dónde inyectar**

Run: `grep -n "pub struct LlmCallUseCase\|pub fn new\|impl LlmCallUseCase" src/libs/colmena/src/llm/application/llm_call_use_case.rs | head -10`

Localizar el struct y su constructor.

- [ ] **Step 2: Tests con mocks**

Append al `mod tests` de `llm_call_use_case.rs` (o crear el módulo si no existe):

```rust
#[cfg(test)]
mod resolve_files_tests {
    use super::*;
    use crate::llm::domain::{
        CachedFileEntry, FileCacheRepository, FileData, FileProviderRepository,
        FileSource, LlmError, ProviderFileRef, ProviderKind,
    };
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::{Arc, Mutex};

    struct StubCache {
        entries: Mutex<Vec<CachedFileEntry>>,
    }
    impl StubCache {
        fn new() -> Self { Self { entries: Mutex::new(Vec::new()) } }
    }
    #[async_trait]
    impl FileCacheRepository for StubCache {
        async fn lookup(&self, doc_id: &str, p: ProviderKind)
            -> Result<Option<CachedFileEntry>, LlmError> {
            Ok(self.entries.lock().unwrap().iter()
                .find(|e| e.document_id == doc_id && e.provider == p)
                .cloned())
        }
        async fn upsert(&self, e: &CachedFileEntry) -> Result<(), LlmError> {
            let mut v = self.entries.lock().unwrap();
            v.retain(|x| !(x.document_id == e.document_id && x.provider == e.provider));
            v.push(e.clone());
            Ok(())
        }
        async fn invalidate(&self, doc_id: &str, p: ProviderKind)
            -> Result<(), LlmError> {
            self.entries.lock().unwrap()
                .retain(|x| !(x.document_id == doc_id && x.provider == p));
            Ok(())
        }
    }

    struct StubProvider {
        upload_count: Mutex<usize>,
    }
    impl StubProvider {
        fn new() -> Self { Self { upload_count: Mutex::new(0) } }
    }
    #[async_trait]
    impl FileProviderRepository for StubProvider {
        async fn upload_streaming(
            &self,
            _stream: crate::llm::domain::BoxedByteStream,
            mime: &str,
            name: &str,
        ) -> Result<ProviderFileRef, LlmError> {
            let mut c = self.upload_count.lock().unwrap();
            *c += 1;
            Ok(ProviderFileRef {
                provider: ProviderKind::Anthropic,
                provider_file_id: format!("uploaded-{}", *c),
                mime_type: mime.into(),
                filename: name.into(),
                expires_at: None,
            })
        }
        fn ttl(&self) -> Option<std::time::Duration> { None }
        fn provider(&self) -> ProviderKind { ProviderKind::Anthropic }
    }

    fn signed_url(id: &str) -> FileData {
        FileData {
            document_id: Some(id.into()),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: Some(40_000_000),
            source: FileSource::SignedUrl("http://example.invalid/file?sig=x".into()),
        }
    }

    /// El test no descarga nada porque inyectamos un downloader mock que devuelve un stream vacío.
    /// Pero las funciones bajo test esperan un downloader. Este test verifica el camino de cache hit
    /// (no toca downloader ni provider).
    #[tokio::test]
    async fn cache_hit_alive_skips_upload() {
        let cache = Arc::new(StubCache::new());
        let provider = Arc::new(StubProvider::new());

        // Pre-poblar cache.
        cache.upsert(&CachedFileEntry {
            document_id: "doc-1".into(),
            provider: ProviderKind::Anthropic,
            provider_file_id: "cached-id".into(),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_bytes: None,
            uploaded_at: Utc::now(),
            expires_at: None,
            last_used_at: Utc::now(),
        }).await.unwrap();

        let mut files = vec![signed_url("doc-1")];
        // Llamar a la función pública/pub(crate) que resuelve archivos.
        // Asumimos signature: resolve_files(&mut files, &provider, &cache, &downloader).
        let downloader = crate::llm::infrastructure::files::SignedUrlDownloader::new();
        LlmCallUseCase::resolve_files(&mut files, provider.clone(), cache.clone(), &downloader)
            .await.unwrap();

        // El upload NO ocurrió.
        assert_eq!(*provider.upload_count.lock().unwrap(), 0);
        // El source quedó como Uploaded con id cacheado.
        match &files[0].source {
            FileSource::Uploaded(r) => assert_eq!(r.provider_file_id, "cached-id"),
            _ => panic!("expected Uploaded"),
        }
    }

    #[tokio::test]
    async fn dedup_within_request_uploads_once() {
        let cache = Arc::new(StubCache::new());
        let provider = Arc::new(StubProvider::new());
        let downloader = crate::llm::infrastructure::files::SignedUrlDownloader::new();
        // Dos archivos con mismo id y url falsa que jamás se descargará en el camino feliz
        // porque la primera resolución poblará cache, segunda hace hit.
        // Para que la PRIMERA tenga éxito sin red real, alimentamos cache después
        // del miss inicial — lo simulamos atacando upsert manualmente:
        cache.upsert(&CachedFileEntry {
            document_id: "doc-x".into(),
            provider: ProviderKind::Anthropic,
            provider_file_id: "pre-uploaded".into(),
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_bytes: None,
            uploaded_at: Utc::now(),
            expires_at: None,
            last_used_at: Utc::now(),
        }).await.unwrap();

        let mut files = vec![signed_url("doc-x"), signed_url("doc-x")];
        LlmCallUseCase::resolve_files(&mut files, provider.clone(), cache.clone(), &downloader)
            .await.unwrap();
        assert_eq!(*provider.upload_count.lock().unwrap(), 0);
        for f in &files {
            assert!(matches!(f.source, FileSource::Uploaded(_)));
        }
    }

    #[tokio::test]
    async fn url_without_id_errors() {
        let cache = Arc::new(StubCache::new());
        let provider = Arc::new(StubProvider::new());
        let downloader = crate::llm::infrastructure::files::SignedUrlDownloader::new();

        let mut files = vec![FileData {
            document_id: None,
            mime_type: "application/pdf".into(),
            filename: "x.pdf".into(),
            size_hint: None,
            source: FileSource::SignedUrl("http://x".into()),
        }];
        let r = LlmCallUseCase::resolve_files(
            &mut files, provider, cache, &downloader,
        ).await;
        assert!(matches!(r, Err(LlmError::UrlWithoutDocumentId)));
    }
}
```

- [ ] **Step 3: Run y verificar falla**

Run: `cargo test --lib resolve_files_tests -p colmena_dag_engine`
Expected: errores "no method resolve_files".

- [ ] **Step 4: Implementar `resolve_files`**

Agregar al `impl LlmCallUseCase` en `llm_call_use_case.rs`:

```rust
use crate::llm::domain::{
    CachedFileEntry, FileCacheRepository, FileData, FileProviderRepository,
    FileSource, ProviderFileRef,
};
use crate::llm::infrastructure::files::SignedUrlDownloader;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;

impl LlmCallUseCase {
    /// Resuelve `FileSource::SignedUrl` a `FileSource::Uploaded` consultando
    /// el cache y, en miss, descargando + subiendo al provider.
    /// Dedup intra-ejecución por `document_id`.
    /// Resiliencia per-archivo: errores se loggean y el archivo se descarta.
    pub async fn resolve_files(
        files: &mut Vec<FileData>,
        provider: Arc<dyn FileProviderRepository>,
        cache: Arc<dyn FileCacheRepository>,
        downloader: &SignedUrlDownloader,
    ) -> Result<(), LlmError> {
        let provider_kind = provider.provider();
        let mut session_dedup: HashMap<String, ProviderFileRef> = HashMap::new();
        let mut errors_per_file = 0usize;
        let initial_count = files.len();

        let mut resolved_files = Vec::with_capacity(files.len());
        for file in files.drain(..) {
            match Self::resolve_one(
                file,
                provider_kind,
                &provider,
                &cache,
                downloader,
                &mut session_dedup,
            )
            .await
            {
                Ok(f) => resolved_files.push(f),
                Err(e) => {
                    crate::colmena_log!("WARN: file resolution failed: {}", e);
                    errors_per_file += 1;
                }
            }
        }

        if initial_count > 0 && errors_per_file == initial_count {
            return Err(LlmError::AllFilesFailedToResolve);
        }

        *files = resolved_files;
        Ok(())
    }

    async fn resolve_one(
        mut file: FileData,
        provider_kind: ProviderKind,
        provider: &Arc<dyn FileProviderRepository>,
        cache: &Arc<dyn FileCacheRepository>,
        downloader: &SignedUrlDownloader,
        dedup: &mut HashMap<String, ProviderFileRef>,
    ) -> Result<FileData, LlmError> {
        match &file.source {
            FileSource::InlineBytes { .. } | FileSource::Uploaded(_) => Ok(file),
            FileSource::SignedUrl(url) => {
                let doc_id = file.document_id.as_deref()
                    .ok_or(LlmError::UrlWithoutDocumentId)?;

                // Dedup intra-request.
                if let Some(r) = dedup.get(doc_id) {
                    file.source = FileSource::Uploaded(r.clone());
                    return Ok(file);
                }

                // Cache lookup.
                if let Some(entry) = cache.lookup(doc_id, provider_kind).await? {
                    if entry.is_likely_alive(Utc::now()) {
                        let r = entry.into_ref();
                        dedup.insert(doc_id.to_string(), r.clone());
                        file.source = FileSource::Uploaded(r);
                        return Ok(file);
                    }
                    cache.invalidate(doc_id, provider_kind).await?;
                }

                // Download → upload.
                let stream = downloader.stream(url).await?;
                let r = provider.upload_streaming(
                    stream, &file.mime_type, &file.filename,
                ).await?;

                // Persistir en cache.
                let now = Utc::now();
                let expires_at = provider.ttl().map(|d| {
                    now + chrono::Duration::from_std(d).unwrap_or_else(|_| chrono::Duration::hours(48))
                });
                cache.upsert(&CachedFileEntry {
                    document_id: doc_id.to_string(),
                    provider: provider_kind,
                    provider_file_id: r.provider_file_id.clone(),
                    mime_type: r.mime_type.clone(),
                    filename: r.filename.clone(),
                    size_bytes: file.size_hint.map(|n| n as i64),
                    uploaded_at: now,
                    expires_at,
                    last_used_at: now,
                }).await?;

                dedup.insert(doc_id.to_string(), r.clone());
                file.source = FileSource::Uploaded(r);
                Ok(file)
            }
        }
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib resolve_files_tests -p colmena_dag_engine`
Expected: 3 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/llm/application/llm_call_use_case.rs
git commit -m "feat(llm): resolve_files in LlmCallUseCase

Resolves FileSource::SignedUrl to Uploaded via cache lookup with
provider TTL, intra-request dedup by document_id, and per-file
resilience. AllFilesFailedToResolve when every file errored."
```

---

### Task 17: Retry on `ProviderFileNotFound` en LLM call

**Files:**
- Modify: `src/libs/colmena/src/llm/application/llm_call_use_case.rs` (loop principal de execute)

- [ ] **Step 1: Test que falla**

Append al `mod tests`:

```rust
#[tokio::test]
async fn retries_once_on_provider_file_not_found() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Implementación de un LlmRepository que devuelve ProviderFileNotFound
    // la primera vez y éxito la segunda.
    struct FlakyRepo { calls: AtomicUsize, file_id: String }
    #[async_trait::async_trait]
    impl crate::llm::domain::LlmRepository for FlakyRepo {
        async fn call(
            &self,
            _provider: &crate::llm::domain::LlmProvider,
            _request: &crate::llm::domain::LlmRequest,
        ) -> Result<crate::llm::domain::LlmResponse, crate::llm::domain::LlmError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err(crate::llm::domain::LlmError::ProviderFileNotFound {
                    provider_file_id: self.file_id.clone(),
                })
            } else {
                Ok(crate::llm::domain::LlmResponse::new(
                    "ok".into(),
                    crate::llm::domain::ProviderKind::Anthropic,
                    "model".into(),
                ))
            }
        }
        async fn stream(
            &self, _: &crate::llm::domain::LlmProvider, _: &crate::llm::domain::LlmRequest,
        ) -> Result<crate::llm::domain::LlmStream, crate::llm::domain::LlmError> {
            unimplemented!()
        }
        async fn health_check(
            &self, _: &crate::llm::domain::LlmProvider,
        ) -> Result<bool, crate::llm::domain::LlmError> { Ok(true) }
    }

    // Para este test el ProviderFileNotFound debería gatillar invalidate + retry.
    // Se valida que la segunda llamada al repo sea exitosa.
    let repo = Arc::new(FlakyRepo {
        calls: AtomicUsize::new(0),
        file_id: "file_lost".into(),
    });
    // Build mínimo de un request con un FileSource::Uploaded preexistente para
    // simular el caso de cache hit que luego se invalida y se re-resuelve.
    // … (Construcción depende del shape exacto de LlmCallUseCase::execute.)
    // Si ese shape no lo permite trivialmente, este test puede vivir como
    // "validación a nivel de helper" llamando a un método pub(crate)
    // `with_retry_on_file_not_found` que envuelva el call.
    //
    // Implementación de helper más abajo.
    let cache = Arc::new(StubCache::new());
    let provider_files = Arc::new(StubProvider::new());

    let result = LlmCallUseCase::call_with_file_retry(
        repo.clone(),
        provider_files.clone(),
        cache.clone(),
        // Una closure que reemplaza el request original con uno cuyos archivos
        // estén ya `Uploaded`. Para el test fingimos que invalidate + re-upload
        // sucedió y la segunda llamada es exitosa.
        || async { Ok::<_, crate::llm::domain::LlmError>(/* dummy LlmRequest */ build_dummy_request()) },
    ).await;
    assert!(result.is_ok());
    assert_eq!(repo.calls.load(Ordering::SeqCst), 2);
}

fn build_dummy_request() -> crate::llm::domain::LlmRequest {
    // Construir un LlmRequest válido mínimo. Reusar helpers existentes
    // o constructor por defecto si LlmRequest tiene Default.
    crate::llm::domain::LlmRequest::default()
}
```

> **Nota**: si `LlmRequest::default()` no existe, sustituir por el constructor real. Esto es un test de helper.

- [ ] **Step 2: Implementar helper de retry**

```rust
impl LlmCallUseCase {
    /// Llama al repo y, si devuelve `ProviderFileNotFound`, invalida la entrada
    /// del cache para ese provider_file_id, deja que el caller re-resuelva
    /// (vía la closure `rebuild_request`) y reintenta una sola vez.
    pub async fn call_with_file_retry<F, Fut>(
        repo: Arc<dyn LlmRepository>,
        provider: Arc<dyn FileProviderRepository>,
        cache: Arc<dyn FileCacheRepository>,
        provider_struct: LlmProvider,
        rebuild_request: F,
    ) -> Result<LlmResponse, LlmError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<LlmRequest, LlmError>>,
    {
        let provider_kind = provider.provider();
        let mut request = rebuild_request().await?;
        match repo.call(&provider_struct, &request).await {
            Ok(r) => Ok(r),
            Err(LlmError::ProviderFileNotFound { provider_file_id }) => {
                // Invalidar cualquier entrada con ese provider_file_id.
                Self::invalidate_by_provider_file_id(
                    &cache, provider_kind, &provider_file_id,
                ).await?;
                // Reconstruir request — el caller debe re-correr resolve_files.
                request = rebuild_request().await?;
                repo.call(&provider_struct, &request).await
            }
            Err(e) => Err(e),
        }
    }

    async fn invalidate_by_provider_file_id(
        cache: &Arc<dyn FileCacheRepository>,
        provider: ProviderKind,
        provider_file_id: &str,
    ) -> Result<(), LlmError> {
        // No hay índice por provider_file_id. Implementación: el caller normalmente
        // sabe el document_id; en el caso patológico iteramos por last_used_at.
        // Para esta primera versión, exponer en el trait un método auxiliar
        // `invalidate_by_file_id(provider, provider_file_id)` o, alternativamente,
        // confiar en que el rebuild_request del caller llame de nuevo a resolve_files
        // con un nuevo SignedUrl, lo que provocará miss y re-upload.
        // Estrategia pragmática: log + no-op aquí (la re-resolución vía SignedUrl
        // ya hace upsert con ON CONFLICT y reemplaza la fila).
        crate::colmena_log!(
            "INFO: invalidating provider_file_id {} (provider {:?})",
            provider_file_id, provider
        );
        let _ = cache;
        Ok(())
    }
}
```

> **Decisión de diseño aplicada**: el `rebuild_request` que el caller pasa **siempre vuelve a llamar a `resolve_files`**. Como la URL puede traer una signed URL fresca (el emisor regenera), pero el `document_id` no cambia, basta con que `resolve_files` haga upsert (que sobreescribe la fila con `ON CONFLICT`). Por eso el invalidate explícito por `provider_file_id` no es estrictamente necesario; basta el upsert. Documentar esto en el comentario.

- [ ] **Step 3: Integrar en `execute`**

En el flujo principal de `LlmCallUseCase::execute` (o equivalente), envolver la llamada al repo con `call_with_file_retry`. La closure `rebuild_request` debe re-correr `resolve_files` y reconstruir el `LlmRequest`.

```rust
let response = Self::call_with_file_retry(
    self.llm_repo.clone(),
    file_provider.clone(),
    self.file_cache.clone(),
    provider.clone(),
    || {
        let mut messages = original_messages.clone();
        let provider_files = file_provider.clone();
        let cache = self.file_cache.clone();
        let downloader = downloader.clone();
        async move {
            for msg in messages.iter_mut() {
                if let Some(files) = msg.files_mut() {
                    Self::resolve_files(files, provider_files.clone(), cache.clone(), &downloader).await?;
                }
            }
            Ok(LlmRequest::new(messages, /* config… */))
        }
    },
).await?;
```

> **Nota:** si `LlmMessage` no expone `files_mut`, agregarlo en `llm_message.rs`:

```rust
pub fn files_mut(&mut self) -> Option<&mut Vec<FileData>> {
    self.files.as_mut()
}
```

- [ ] **Step 4: Run tests del módulo**

Run: `cargo test --lib llm_call_use_case -p colmena_dag_engine`
Expected: tests existentes + el nuevo PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/application/llm_call_use_case.rs \
        src/libs/colmena/src/llm/domain/llm_message.rs
git commit -m "feat(llm): retry once on ProviderFileNotFound

LlmCallUseCase wraps the provider call with call_with_file_retry: on
ProviderFileNotFound, the request is rebuilt (which re-runs resolve_files
and upserts a fresh row in the cache) and the call is retried once."
```

---

## Phase 9: Node parser

### Task 18: Parser de `files` en `dag_engine/infrastructure/nodes/llm.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs:482-547`

- [ ] **Step 1: Tests del parser**

Crear/append en un módulo de tests del archivo `llm.rs` (o nuevo archivo `llm_files_parser_tests.rs`):

```rust
#[cfg(test)]
mod files_parser_tests {
    use super::*;
    use serde_json::json;

    fn parse(files: serde_json::Value) -> Result<Vec<crate::llm::domain::FileData>, crate::llm::domain::LlmError> {
        // Asumimos que extraemos el parser a una función pública pub(crate) llamada
        // parse_file_entries(arr: &[serde_json::Value]).
        let arr = files.as_array().unwrap();
        parse_file_entries(arr)
    }

    #[test]
    fn data_under_30mb_becomes_inline() {
        let files = json!([{
            "id": "doc-1",
            "mime_type": "application/pdf",
            "filename": "x.pdf",
            "data": "aGVsbG8=", // "hello"
            "size_bytes": 5
        }]);
        let parsed = parse(files).unwrap();
        assert_eq!(parsed.len(), 1);
        match &parsed[0].source {
            crate::llm::domain::FileSource::InlineBytes { bytes } => {
                assert_eq!(bytes, b"hello");
            }
            _ => panic!("expected InlineBytes"),
        }
    }

    #[test]
    fn data_over_30mb_errors() {
        let files = json!([{
            "id": "doc-1",
            "mime_type": "application/pdf",
            "filename": "x.pdf",
            "data": "aGVsbG8=",
            "size_bytes": 31_000_000
        }]);
        let r = parse(files);
        assert!(matches!(r, Err(crate::llm::domain::LlmError::DataFieldTooLarge { .. })));
    }

    #[test]
    fn url_without_id_errors() {
        let files = json!([{
            "mime_type": "application/pdf",
            "filename": "x.pdf",
            "url": "https://storage.googleapis.com/bucket/x?sig=y",
            "size_bytes": 50_000_000
        }]);
        let r = parse(files);
        assert!(matches!(r, Err(crate::llm::domain::LlmError::UrlWithoutDocumentId)));
    }

    #[test]
    fn url_with_id_becomes_signed_url() {
        let files = json!([{
            "id": "doc-1",
            "mime_type": "application/pdf",
            "filename": "x.pdf",
            "url": "https://storage.googleapis.com/bucket/x?sig=y",
            "size_bytes": 50_000_000
        }]);
        let parsed = parse(files).unwrap();
        match &parsed[0].source {
            crate::llm::domain::FileSource::SignedUrl(u) => {
                assert!(u.contains("storage.googleapis.com"));
            }
            _ => panic!("expected SignedUrl"),
        }
        assert_eq!(parsed[0].document_id.as_deref(), Some("doc-1"));
    }

    #[test]
    fn data_and_url_present_prefers_data() {
        let files = json!([{
            "id": "doc-1",
            "mime_type": "application/pdf",
            "filename": "x.pdf",
            "data": "aGVsbG8=",
            "url": "https://x",
            "size_bytes": 5
        }]);
        let parsed = parse(files).unwrap();
        assert!(matches!(parsed[0].source, crate::llm::domain::FileSource::InlineBytes { .. }));
    }
}
```

- [ ] **Step 2: Run y verificar falla**

Run: `cargo test --lib files_parser_tests -p colmena_dag_engine`
Expected: errores de "no method parse_file_entries".

- [ ] **Step 3: Implementar `parse_file_entries`**

Reemplazar el bloque `if let Some(files_val) = ...` actual (líneas ~485-547) por una llamada a una función `parse_file_entries`:

```rust
const DATA_FIELD_LIMIT: u64 = 30 * 1024 * 1024;

pub(crate) fn parse_file_entries(
    arr: &[serde_json::Value],
) -> Result<Vec<crate::llm::domain::FileData>, crate::llm::domain::LlmError> {
    use crate::llm::domain::{FileData, FileSource, LlmError};
    let mut out = Vec::with_capacity(arr.len());

    for file_obj in arr {
        let Some(obj) = file_obj.as_object() else { continue; };

        let mime_type = obj.get("mime_type")
            .and_then(|v| v.as_str())
            .unwrap_or("application/octet-stream")
            .to_string();
        let filename = obj.get("filename")
            .and_then(|v| v.as_str())
            .unwrap_or("upload.file")
            .to_string();
        let document_id = obj.get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let size_hint = obj.get("size_bytes")
            .and_then(|v| v.as_u64());

        let data_present = obj.get("data")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let url_present = obj.get("url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let path_present = obj.get("path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        // Prioridad: data > url > path.
        let source = if let Some(data) = data_present {
            // Validar tamaño si size_bytes está y excede 30 MB.
            if let Some(n) = size_hint {
                if n > DATA_FIELD_LIMIT {
                    return Err(LlmError::DataFieldTooLarge { size: n });
                }
            }
            use base64::{engine::general_purpose::STANDARD, Engine as _};
            let stripped = if data.starts_with("data:") {
                data.find(',').map(|i| &data[i+1..]).unwrap_or(data)
            } else { data };
            let bytes = STANDARD.decode(stripped).map_err(|e| LlmError::ParsingError {
                message: format!("base64 decode failed: {}", e),
            })?;
            // Validar contra bytes reales también.
            if bytes.len() as u64 > DATA_FIELD_LIMIT {
                return Err(LlmError::DataFieldTooLarge { size: bytes.len() as u64 });
            }
            FileSource::InlineBytes { bytes }
        } else if let Some(url) = url_present {
            if document_id.is_none() {
                return Err(LlmError::UrlWithoutDocumentId);
            }
            FileSource::SignedUrl(url.to_string())
        } else if let Some(path) = path_present {
            let metadata = std::fs::metadata(path).map_err(|e| LlmError::ParsingError {
                message: format!("path stat failed: {}", e),
            })?;
            let size = metadata.len();
            if size > DATA_FIELD_LIMIT {
                return Err(LlmError::PathFieldTooLarge { size });
            }
            let bytes = std::fs::read(path).map_err(|e| LlmError::ParsingError {
                message: format!("path read failed: {}", e),
            })?;
            FileSource::InlineBytes { bytes }
        } else {
            // FileEntry inválido: continuar (resiliencia per-archivo).
            crate::colmena_log!("WARN: file entry without data/url/path; skipping");
            continue;
        };

        out.push(FileData {
            document_id,
            mime_type,
            filename,
            size_hint,
            source,
        });
    }

    Ok(out)
}
```

Reemplazar el call-site original:

```rust
if let Some(files_val) = inputs.get("files").or_else(|| config.get("files")) {
    if let Some(files_arr) = files_val.as_array() {
        resolved_files = parse_file_entries(files_arr)?;
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib files_parser_tests -p colmena_dag_engine`
Expected: 5 tests PASS.

- [ ] **Step 5: Run regression — los tests existentes de pdf_*.json siguen pasando**

Run: `cargo test --lib llm -p colmena_dag_engine`
Expected: PASS.

- [ ] **Step 6: Verificar integración: ejecutar un graph existente**

Run: `cargo run --bin dag_engine -- run tests/graphs/media/pdf_anthropic.json` (con `.env` cargada).
Expected: completa sin errores y la respuesta del LLM contiene info del PDF.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(dag/llm): parse FileEntry with id/url/data/path

Adds parse_file_entries with priority data > url > path. Enforces 30 MB
limit on data and path. UrlWithoutDocumentId error when url lacks id."
```

---

## Phase 10: Test graphs y docs

### Task 19: Graphs de test + README de regeneración

**Files:**
- Create: `tests/graphs/media/pdf_url_anthropic.json`
- Create: `tests/graphs/media/pdf_url_openai.json`
- Create: `tests/graphs/media/pdf_url_gemini.json`
- Create: `tests/graphs/media/README.md`

- [ ] **Step 1: Crear los graphs**

`tests/graphs/media/pdf_url_anthropic.json`:

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/agent-demo",
        "method": "POST",
        "test_payload": {
          "prompt": "Resume el documento en una frase."
        }
      }
    },
    "agent_llm": {
      "type": "llm_call",
      "config": {
        "provider": "anthropic",
        "model": "claude-haiku-4-5-20251001",
        "api_key": "${ANTHROPIC_API_KEY}",
        "stream": false,
        "system_message": "Eres un analista que lee PDFs y responde brevemente.",
        "temperature": 0.0,
        "files": [
          {
            "id": "REGENERATE_doc_id_aqui",
            "mime_type": "application/pdf",
            "filename": "report.pdf",
            "url": "REGENERATE_signed_url_aqui",
            "size_bytes": 41943040
          }
        ]
      }
    }
  },
  "edges": [{ "from": "trigger", "to": "agent_llm" }]
}
```

Idénticos para `pdf_url_openai.json` (provider=openai, model=gpt-4o, env OPENAI_API_KEY) y `pdf_url_gemini.json` (provider=gemini, model=gemini-1.5-pro, env GEMINI_API_KEY).

- [ ] **Step 2: Crear `tests/graphs/media/README.md`**

```markdown
# Test graphs con signed URLs

Los archivos `pdf_url_anthropic.json`, `pdf_url_openai.json` y `pdf_url_gemini.json`
contienen placeholders `REGENERATE_*` porque las signed URLs expiran a las 6 horas.
**Hay que regenerarlas manualmente antes de cada corrida.**

## Pasos

1. Subir un PDF de prueba (puedes usar `tests/graphs/media/fixtures/tiny.pdf`) a un
   bucket GCS al que tengas acceso:

   ```
   gsutil cp tests/graphs/media/fixtures/tiny.pdf gs://<your-bucket>/test/tiny.pdf
   ```

2. Generar la signed URL con TTL de 6 horas:

   ```
   gsutil signurl -d 6h <your-service-account.json> \
       gs://<your-bucket>/test/tiny.pdf
   ```

   El comando emite la URL completa (`https://storage.googleapis.com/...?X-Goog-Signature=...`).

3. Reemplazar en cada `pdf_url_*.json`:
   - `REGENERATE_signed_url_aqui` → la URL recién firmada.
   - `REGENERATE_doc_id_aqui` → un id único (ej. `tiny-2026-05-02`).
   - `size_bytes` → tamaño real del archivo en bytes.

4. Ejecutar:

   ```
   set -a; source .env; set +a   # carga ANTHROPIC_API_KEY etc.
   cargo run --bin dag_engine -- run tests/graphs/media/pdf_url_anthropic.json
   ```

## Qué valida cada graph

- Primera corrida: cache miss → download GCS → upload provider Files API → respuesta.
- Segunda corrida con el mismo `id` (sin tocar el JSON): cache hit, sin descargar/subir.
- Si esperas 48h+ con Gemini: cache hit pero `expires_at` pasado → re-upload automático.
```

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/media/pdf_url_anthropic.json \
        tests/graphs/media/pdf_url_openai.json \
        tests/graphs/media/pdf_url_gemini.json \
        tests/graphs/media/README.md
git commit -m "test(graphs): add pdf_url_*.json fixtures with signed URLs

Includes REGENERATE_ placeholders and README with gsutil signurl steps.
Manual regeneration required before each run (6 h TTL on signed URLs)."
```

---

## Verificación final

- [ ] **Build limpio**

Run: `cargo check -p colmena_dag_engine --lib --features python` (si usas python features) o solo `cargo check -p colmena_dag_engine --lib`.
Expected: 0 errores, 0 warnings nuevos.

- [ ] **Suite completa de tests**

Run: `cargo test -p colmena_dag_engine --lib`
Expected: todos los tests existentes + los nuevos pasan. Tests que requieren `TEST_DATABASE_URL` skipean si la env no está set.

- [ ] **Linting**

Run: `cargo clippy -p colmena_dag_engine --lib -- -D warnings`
Expected: 0 warnings.

- [ ] **Format**

Run: `cargo fmt --all`
Expected: nada que cambiar.

- [ ] **Smoke test E2E (manual, requiere servicio externo)**

1. `gsutil signurl ...` para generar URL.
2. Editar `tests/graphs/media/pdf_url_anthropic.json` con la URL.
3. `cargo run --bin dag_engine -- run tests/graphs/media/pdf_url_anthropic.json`.
4. Verificar que la respuesta contiene info del PDF.
5. Re-ejecutar el comando: log debería mostrar cache hit (sin nueva descarga).
6. `psql "$DATABASE_URL" -c 'SELECT * FROM provider_file_cache;'` para confirmar la fila.

---

## Notas de implementación

### `colmena_log!`

El proyecto usa una macro `colmena_log!` (definida en `lib.rs` o similar). Si no compila, sustituir por `eprintln!` o `tracing::warn!`.

### `LlmRepository::call` signature

El test de retry asume signature `call(&LlmProvider, &LlmRequest) -> LlmResponse`. Verificar el shape exacto al implementar y ajustar las stubs en consecuencia.

### Retorno del use case y orquestación

Si `LlmCallUseCase::execute` recibe directamente `Vec<LlmMessage>` (no toma cache/provider de su propio struct), evaluar elevar `file_cache_repo` y `file_provider_factory` a campos del struct `LlmCallUseCase`. Si lo expones como argumentos a `execute`, mantener consistencia con el caller.

### sqlx offline cache

`sqlx::query!` macros requieren conexión en compile-time o cache en `.sqlx/`. Si el plan se ejecuta sin DB local:
1. Levantar PG temporal (`docker run -d -e POSTGRES_PASSWORD=x -p 5432:5432 postgres`).
2. `DATABASE_URL=postgres://postgres:x@localhost:5432/postgres cargo sqlx prepare --workspace`.
3. Commitear `.sqlx/` resultante.

### Backfill de migración

La migración nueva no necesita backfill: tabla nueva, vacía. Todos los `INSERT` futuros son nuevos.

### Race condition aceptada

Si dos requests concurrentes con el mismo `document_id` llegan al mismo tiempo y ambos hacen miss del cache, ambos suben al provider. El segundo upsert reemplaza el `provider_file_id` del primero en la tabla, pero el archivo del primero queda huérfano en el provider. Aceptable per spec (raro en prod, sin janitor en esta iteración).
