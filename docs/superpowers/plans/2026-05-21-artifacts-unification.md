# Implementation Plan — Artifacts unification (Cycle: media outputs as registry-managed attachments)

## Summary

Reusar el sistema existente de `AttachmentRegistry` + `load_attachment` para los outputs generados por `image_generation`, `image_edit`, `tts`. Resultado: el agente puede ABRIR sus propias generaciones para verlas (load_attachment), ENCADENAR ediciones por attachment_id, y ENVIAR los bytes a endpoints externos (http_request con `$attachment:<id>` placeholder) — todo sin meter base64 grande en el contexto del LLM.

## Decisiones convergidas (de brainstorm)

| # | Decisión |
|---|---|
| 1 | Modelo unificado: outputs generados van al MISMO `AttachmentRegistry` que los uploaded |
| 2 | "Ver propia generación" = opt-in via `load_attachment` (no auto-attach) |
| 3 | "Enviar a endpoint" = `http_request` con placeholder `$attachment:<id>` resuelto por engine |
| 4 | Cross-provider: una fila con `provider: Generated`. Lazy upload al primer `load_attachment` desde otro provider, refresh `provider_file_id`. |
| 5 | `storage_key` se guarda en el campo `provider_file_id` existente (semántica derivada de `source`) |
| 6 | DB siempre disponible (Colmena requiere `DATABASE_URL`); no se necesita InMemory adapter |
| 7 | `OutputStorageRepository` gana método `read(storage_key) -> bytes` |

## Phase A — Foundation (no breaking changes)

### A.1 — Extender `OutputStorageRepository`

```rust
// storage/domain/output_storage_repository.rs
#[async_trait]
pub trait OutputStorageRepository: Send + Sync {
    async fn store(&self, req: StoreRequest) -> Result<StoredOutput, StorageError>;

    /// Retrieve bytes for a previously-stored output by its `storage_key`.
    /// Used by cross-provider upload (load_attachment) and by the
    /// `$attachment:<id>` placeholder in http_request.
    async fn read(&self, storage_key: &str) -> Result<StoredBytes, StorageError>;
}

pub struct StoredBytes {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub filename: String,
}
```

### A.2 — Implementación por adapter

**`LocalCacheStorageAdapter`**: extiende con `Arc<DashMap<String, StoredBytes>>` internal. `store` guarda; `read` busca. Bytes viven en RAM por la duración del process.

**`HttpCallbackStorageAdapter`**: extiende el callback contract con un segundo verb:
- `POST /internal/gcs/sign-put` (ya existía en plan Phase 7) → para `store`
- Nuevo `GET /internal/gcs/sign-get?storage_key=...` → devuelve fresh signed read URL
- `read(key)` hace GET al sign-get, baja los bytes, devuelve

Si el sign-get endpoint no existe, fallback: `read_url` cacheado en attachment registry → GET directo (puede fallar si caducó >1h).

### A.3 — Nuevas variantes

```rust
// llm/domain/llm_provider.rs
pub enum ProviderKind {
    OpenAi,
    Google,
    Anthropic,
    Mock,
    Generated,   // NEW
}

// llm/domain/attachments/conversation_attachment.rs (o similar)
pub enum AttachmentSource {
    User,
    Generated,   // si no existe ya, agregar
}
```

`ProviderKind::Generated` significa: "no atado a ningún Files API; resolvelo al cargar". Sus métodos `default_model`/`env_var_name` panican o devuelven sentinels (este provider nunca se usa para llamar LLMs).

## Phase B — Wire generation nodes

Después de `storage.store(...)`, cada uno de los 3 nodos llama:

```rust
if let Some(registry) = &self.attachment_registry {
    let _ = registry.upsert(UpsertAttachmentInput {
        agent_session_id: agent_session_id.clone().unwrap_or_default(),
        document_id: stored.storage_key.clone(),
        provider: ProviderKind::Generated,
        provider_file_id: stored.storage_key.clone(),  // reuse semántica
        mime_type: stored.mime_type.clone(),
        filename: stored.filename.clone(),
        size_bytes: Some(stored.size_bytes),
        label: None,
        description: Some(format!("{} generated: {}", node_kind, prompt_preview)),
        source: AttachmentSource::Generated,
    }).await;
}
```

Cambios:
- 3 nodos (`ImageGenerationNode`, `ImageEditNode`, `TtsNode`) reciben `Option<Arc<dyn AttachmentRegistry>>` en constructor (igual patrón que `secure_values`)
- `HashMapNodeRegistry::new_with_secure_values` recibe `attachment_registry` (mismo patrón)
- `ColmenaEngine::new` instancia `PostgresAttachmentRegistry` (ya existe) y lo pasa

Fail-soft: si el registry no está disponible o falla el upsert, el nodo sigue (la generación ya pasó, lo que se pierde es la capacidad de "ver después").

## Phase C — Cross-provider lazy upload en load_attachment

`load_attachment_tool` dispatch + `AgentService` interceptor ya existen. El cambio chico:

**`dispatch_load_attachment`** (líneas ~90+ de load_attachment_tool.rs):

```rust
// Resolver — antes solo buscaba ConversationAttachment por (session, doc_id, provider)
let entry = registry.lookup(session, doc_id, current_provider).await?;

let entry = match entry {
    Some(e) => e,
    None => {
        // Fallback: ¿existe la fila con provider=Generated?
        match registry.lookup(session, doc_id, ProviderKind::Generated).await? {
            Some(gen_entry) => {
                // Cross-provider lazy upload
                let bytes = storage.read(&gen_entry.provider_file_id).await?;
                let provider_file_id = upload_to_current_provider(&bytes, current_provider).await?;
                registry.upsert(UpsertAttachmentInput {
                    /* mismo entry pero con provider=current_provider, provider_file_id=nuevo */
                }).await?;
                ConversationAttachment { /* con el nuevo provider_file_id */ }
            }
            None => return Err(LlmError::unknown_document_id(doc_id))
        }
    }
};
```

Donde `upload_to_current_provider` ya existe parcialmente como infra (ver `llm/infrastructure/files/`). Reusar.

## Phase D — `$attachment:<id>` placeholder en http_request

Extender `HttpNode` para escanear su body/multipart en busca de strings con prefijo `$attachment:`. Por cada match:
1. Extraer el `<id>` (= storage_key)
2. `storage.read(id)` → bytes
3. Reemplazar según contexto:
   - String en body JSON → `"data:<mime>;base64,..."` o solo base64 según config
   - Field en multipart → `Part::bytes(...)`

Detalle:
```rust
// Pseudo
fn resolve_attachment_placeholders(value: &mut Value, storage: &dyn OutputStorageRepository) {
    match value {
        Value::String(s) if s.starts_with("$attachment:") => {
            let id = &s["$attachment:".len()..];
            let bytes = storage.read(id).await?;
            *s = format!("data:{};base64,{}", bytes.mime_type, base64::encode(bytes.bytes));
        }
        Value::Object(map) => for v in map.values_mut() { resolve(v, storage) },
        Value::Array(arr) => for v in arr { resolve(v, storage) },
        _ => {}
    }
}
```

Para multipart, agregar campo `multipart` al config de http_request que acepta `{ field_name: "$attachment:id" }` y construye `Part::bytes` correspondientemente.

## Architectural Impact

- **Layers afectadas**: domain (trait extension, value object additions), infrastructure (adapters, registry wiring), application (load_attachment dispatch logic)
- **New traits/ports**: ninguno (extender existentes)
- **New adapters**: ninguno (extender existentes)
- **Modified files**:
  - `storage/domain/output_storage_repository.rs` (read method)
  - `storage/infrastructure/{local_cache,http_callback}_adapter.rs` (read impl)
  - `llm/domain/llm_provider.rs` (ProviderKind::Generated)
  - `llm/domain/attachments/conversation_attachment.rs` (AttachmentSource::Generated si no existe)
  - 3 media nodes (constructor, register-after-store)
  - `registry.rs` (pass attachment_registry to media nodes)
  - `engine.rs` (instanciate registry)
  - `load_attachment_tool.rs` (cross-provider lazy upload)
  - `http.rs` (placeholder resolution)
- **Binding impact**: ninguno

## Testing Strategy

### Unit (mockall + httpmock)
- `LocalCacheStorageAdapter`: store → read round-trip
- `HttpCallbackStorageAdapter`: read calls sign-get endpoint + PUT to returned URL, bytes parsed correctly
- Media nodes: registry.upsert called with correct shape after successful generation
- load_attachment cross-provider: row lookup hits Generated → storage.read → upload mock → refresh_provider_file_id
- http_request placeholder: $attachment:abc in body replaced with base64

### Integration (ignored, requires API keys + DB)
- E2E: gen → load_attachment (same provider) → edit → load_attachment (different provider — cross-prov upload)
- E2E: gen → http_request with $attachment in multipart → mock server receives correct bytes

### Sample graph
`tests/graphs/agents/multimedia_agent_with_load.json` — agent generates an image, then is prompted to "describe what you just created" → must call load_attachment → vision turn.

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| HttpCallbackStorageAdapter.read needs new ADP endpoint that doesn't exist yet | Phase A.2 fallback: GET cached read_url. Document the production-grade endpoint as future work in Phase 7. |
| `provider: Generated` row without provider_file_id confuses existing code | Source-aware reads: anywhere that reads provider_file_id and expects an OpenAI Files API id must check `source` first |
| Lazy cross-provider upload races (two turns load simultaneously) | Idempotent upsert via the existing `(session, doc_id, provider)` unique key — last write wins is fine for provider_file_id |
| http_request placeholder leaks into URLs / headers (not just body) | Restrict resolution to body + multipart only; reject in URL/headers with clear error |
| Auto-summary for generated outputs runs unnecessary cheap-LLM call | Description set explicitly from prompt prefix — skip auto-summary when source=Generated |

## Estimated Scope

| Phase | Lines | Why |
|-------|-------|-----|
| A — storage.read + new variants | ~150 | Trait method, 2 adapter impls, 2 enum variants, ~6 tests |
| B — media nodes register | ~120 | 3 nodes × constructor + after-store logic + tests |
| C — load_attachment cross-prov | ~180 | Dispatch logic + upload helper + tests |
| D — http_request $attachment | ~200 | Recursive placeholder scan + multipart support + tests |
| **Total** | **~650 LOC** | Plus docs updates |

## Open Questions (non-blocking)

- **Lifetime of LocalCache `read` mem**: bytes persist for process duration. CLI runs that generate many images grow RAM. Acceptable for dev (process is short-lived).
- **Should description auto-summary run on generated images?** No — prompt prefix is better description. Skip when source=Generated.
- **Should the `Generated` provider variant have a special display name in catalog_line?** e.g. `"(generated)"` vs implied. Minor.

## Decision needed before coding

¿Vamos por todo el scope (A+B+C+D) en un cycle, o lo dividimos?

- **All-in (~650 LOC)**: 1-2 sessions. Tests + smoke completo.
- **A+B primero (~270 LOC)**: load_attachment para generated funciona (mismo provider). Cross-prov + http_request quedan para iteración 2. Más incremental.
- **Solo A (~150 LOC)**: storage.read existe pero nada lo usa todavía. Demasiado granular.

Mi recomendación: **A+B** este cycle, **C+D** próximo. Razón: A+B ya desbloquea "el modelo abre su propia generación" (la mayor capacidad), y nos da feedback antes de invertir en C (cross-provider, complejo) y D (http_request enhancement, scope grande).
