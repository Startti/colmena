# Plan COLMENA — Inyección de attachments a `image_edit` (+ catálogo cross-turn)

**Fecha:** 2026-05-29
**Estado:** ✅ Implementado en colmena (Fases 1, 1b, 2). Pendiente: endpoint ADP del plan hermano + E2E cross-process.
**Repo:** colmena (`develop`)
**Plan hermano (ADP):** `adp/docs/COLMENA_SIGN_GET_CROSS_PROCESS.md` — endpoint `POST /internal/gcs/sign-get`.
**Paralelizable:** Sí. Fases 1 y 1b son 100% independientes de ADP. La Fase 2 se desarrolla contra un **mock** del contrato sign-get (§Contrato) y solo se integra con ADP en el E2E/deploy.

---

## 1. Problema

Al agregar `image_generation` + `image_edit` como tools en un `llm_call`:

1. `image_generation("un gato")` → guarda bytes (GCS), registra fila en `conversation_attachments` (`document_id="img_image_0"` → `storage_key`), devuelve al LLM **solo** `{ document_id }`, y su descripción le dice: *"Use `$attachment:<document_id>` in downstream tools"*.
2. `image_edit(source_url="$attachment:img_image_0", ...)` → `image_edit.rs:262` → `fetch_image("$attachment:img_image_0")` → no matchea `local://`/`chat-attachments/`/`data:`/`http(s)` → `else` (`:145-149`) → **`"unsupported url scheme"`**.

El LLM hace lo que `image_generation` le instruye, pero `image_edit` no resuelve el handle. **Contrato roto dentro de colmena.**

### Evidencia

| Hecho | Ubicación |
|---|---|
| `image_generation` devuelve solo `document_id` y dice "use `$attachment:<id>`" | `nodes/image_generation.rs:384-390`, `:429-430` |
| `image_edit.source_url` se lee verbatim, sin resolver `$attachment` | `nodes/image_edit.rs:221-230` → `:262` |
| `fetch_image` solo acepta `local://`/`chat-attachments/`/`data:`/`http(s)` | `nodes/image_edit.rs:88-149` |
| `image_edit` **no** tiene `attachment_resolver` (grep=0); `http_request` **sí** | `nodes/image_edit.rs` vs `nodes/http.rs:39,242-247` |
| `HttpCallback` solo resuelve keys de su `meta_cache` in-process | `storage/infrastructure/http_callback_adapter.rs:44-48,189-200` |

---

## 2. Verificación del catálogo cross-turn (por `agent_session_id`)

**Confirmado en código.** El `llm_call` ya construye un catálogo de attachments de **toda la sesión** y ya expone los handles de inyección:

- `llm.rs:1534` → `registry.list_for_session(agent_session_id)` devuelve **todas** las filas (sin filtro temporal). Se prepende al system message como *"Documents available in this session:"*.
- `render_catalog` (`llm/application/attachment_catalog.rs:43-46`) imprime por doc: `usage: load_attachment("<id>") to read · "$attachment:<id>" to forward`.
- Uploads del usuario: registrados con `origin=user_upload` + `storage_key` (`llm.rs:1271-1298`). Inyectables.
- Imágenes generadas: registradas como `Generated` + `storage_key` (`image_generation.rs:355-369`). Inyectables.
- `AttachmentStreamResolverImpl::resolve(agent_session_id, document_id)` ya hace `registry → storage_key → read_stream` con fallback a raw key (`llm/infrastructure/attachments/stream_resolver_impl.rs:50-95`). `http_request` ya lo usa.

### Gaps (parte colmena)

| # | Gap | Fase |
|---|---|---|
| **G1** | `image_edit` no resuelve `$attachment:<id>` ni `document_id` pelado | 1 |
| **G2** | `HttpCallback.read/read_stream` solo resuelve keys de su `meta_cache` in-process → editar imágenes de turnos previos / uploads falla | 2 |
| **G3** | Catálogo filtra por `provider == provider_kind \|\| Generated` (`llm.rs:1542-1545`); uploads se registran bajo el provider del turno (`llm.rs:1290`) → si cambia el modelo del agente, uploads viejos desaparecen del catálogo | 1b |
| **G4** | `render_catalog` muestra hint `$attachment` incluso para filas legacy con `storage_key=NULL` (no inyectables) | 1b |

---

## 3. Arquitectura — "storage-blind" y localidad de proceso

- colmena **nunca** tiene credenciales GCS (`http_callback_adapter.rs:1-4`). Para leer un objeto necesita una **URL firmada** que solo ADP genera.
- `HttpCallback.read()` usa un `meta_cache: DashMap` in-process con el `read_url` recibido en `store()`. **Solo conoce keys que ese proceso subió.**
- **Same-turn** (generar + editar en la misma respuesta) → mismo proceso → `meta_cache` hit → **Fase 1 alcanza**.
- **Cross-turn / uploads** → otro proceso (o el objeto lo subió ADP) → `meta_cache` miss → **requiere Fase 2** (sign-get cross-process).

---

## 4. FASE 1 — Resolver `$attachment` en `image_edit`  *(independiente de ADP)*

**Fixea G1. Arregla el síntoma actual (same-turn). No toca storage adapter ni ADP.**

### Cambios

**`dag_engine/infrastructure/nodes/image_edit.rs`**
1. Campo `attachment_resolver: Option<Arc<dyn AttachmentStreamResolver>>` (espejo de `http.rs:39`).
2. Builder `with_attachment_resolver(mut self, resolver) -> Self` (espejo de `http.rs:242-247`).
3. `agent_session_id` ya se extrae (`:167-169`); pasarlo a la resolución del source.
4. Resolución de `source_url` (orden):
   1. `$attachment:` → strip prefix → `resolver.resolve(agent_session_id, id)` → drenar `StoredStream` → `(bytes, mime)`.
   2. branches existentes (`local://`, `chat-attachments/`, `data:`, `http(s)`) sin cambios.
   3. token no-URL (p. ej. `img_image_0`) **y** hay resolver → `resolver.resolve(...)` (el fallback a raw key lo absorbe).
   4. nada matchea → error mejorado mencionando `$attachment:<document_id>`.
5. Drenar `StoredStream.stream` a `Vec<u8>` con cap defensivo (sugerido: reusar `COLMENA_FILE_FETCH_MAX_BYTES`=100 MB).
6. Actualizar descripción del tool (`:436`, `:450-453`) y texto de `source_url`.

**`dag_engine/infrastructure/registry.rs`** (`:299-305`)
```rust
let mut edit = ImageEditNode::new(storage_arc);
if let Some(reg) = attachment_registry.clone() { edit = edit.with_attachment_registry(reg); }
if let Some(resolver) = attachment_resolver.clone() { edit = edit.with_attachment_resolver(resolver); }
```
- **Verificar** que `attachment_resolver` y `storage` de `image_generation`/`image_edit` compartan el mismo `Arc<HttpCallbackStorageAdapter>` (meta_cache compartido in-process = lo que hace funcionar el same-turn).

### Tests (unit en `image_edit.rs`)
- `$attachment:doc-1` con `SqliteAttachmentRegistry` + `MockOutputStorageRepository` → edita OK.
- `document_id` pelado → resuelve vía fallback.
- `$attachment:no-existe` → error `NotFound` claro.
- Regresión: `data:` y `http(s)` siguen OK.

### Riesgo
Bajo. Aditivo, no cambia API pública → no rompe el worker ADP.

---

## 5. FASE 1b — Visibilidad cross-provider de uploads + polish del catálogo  *(independiente de ADP)*

**Fixea G3, G4.**

**`dag_engine/infrastructure/nodes/llm.rs`** (`:1542-1545`)
```rust
.filter(|a| a.provider == provider_kind
    || a.provider == ProviderKind::Generated
    || a.origin.as_deref() == Some(origin::USER_UPLOAD))
```
- Mantener dedup por `document_id` con preferencia a la fila del provider actual (`:1549-1560`).
- Solo afecta visibilidad en el catálogo. La lectura para inyección va por `storage_key` (no por provider).

**`llm/application/attachment_catalog.rs`** (`render_catalog`, `:43-46`)
- Recibir si la fila tiene `storage_key`. Si `None`, **omitir** el hint `"$attachment:<id>" to forward` (o marcar "read-only legacy").

### Tests
- Upload bajo provider A visible cuando el turno usa provider B.
- `render_catalog`: fila con `storage_key=None` → sin hint `$attachment`.

### Riesgo
Bajo. Presentación/visibilidad; no afecta resolución de bytes ni API pública.

---

## 6. FASE 2 — Lectura cross-process en `HttpCallback` (sign-get)  *(integra con plan ADP)*

**Fixea G2.** Desarrollar contra el **mock** del contrato (§Contrato). Integración real cuando ADP despliegue el endpoint.

**`storage/infrastructure/http_callback_adapter.rs`**
- Derivar `sign_get_url` por reemplazo de path hermano, igual que `delete_url()` (`:89-90`): `.../sign-put` → `.../sign-get`.
- En `read()` (`:189`) y `read_stream()` (`:230`): en `meta_cache` **miss**, antes de fallar → `POST {sign_get_url}` con `X-Internal-Token` + body `{ "storage_key": <key> }` → `{ "read_url": <url> }` → GET de bytes (reutilizar el camino que ya hace GET sobre un `read_url`).
- Errores: 404 → `InvalidInput`/NotFound; non-2xx → `CallbackFailed`; transporte → `BackendUnavailable`.
- `meta_cache` sigue como fast-path. Decisión: cachear el `read_url` firmado pero revalidar/refirmar en 403 (TTL corto).

### Tests
- `read_stream` con `meta_cache` miss → wiremock del sign-get → `read_url` → GET bytes OK (espejo de `:510-564`).
- sign-get 404 → NotFound.

### Riesgo
Bajo para colmena: el cambio es **interno** (no toca el trait `OutputStorageRepository` ni la API pública) → no rompe el build del worker.

---

## 7. Contrato compartido `sign-get` (la única interfaz con ADP)

> Ambos planes codean contra esto. colmena lo mockea con wiremock; ADP lo implementa. Si esto no cambia, los dos avanzan en paralelo.

- **Método/ruta:** `POST /internal/gcs/sign-get` (hermano de `/internal/gcs/sign-put`).
- **Auth:** header `X-Internal-Token: <COLMENA_INTERNAL_TOKEN>`.
- **Request (JSON):** `{ "storage_key": "<string>" }`
- **Response 200 (JSON):** `{ "read_url": "<signed GET url>" }`  *(snake_case, igual que el `read_url` de sign-put)*.
- **Errores:** `400` storage_key faltante/ inválido · `403` token inválido · `404` objeto no encontrado (colmena → NotFound) · `5xx` transitorio (colmena → BackendUnavailable).
- **Derivación de URL en colmena:** tomar `COLMENA_STORAGE_CALLBACK_URL` (`.../sign-put`) y reemplazar el sufijo `/sign-put` → `/sign-get`.
- **TTL del read_url:** minutos (default actual de ADP = 1h, aceptable).

---

## 8. Orden y paralelismo

```
COLMENA (este plan)                        ADP (plan hermano)
─────────────────────                      ──────────────────
Fase 1   ─┐  (sin dep)                      Sign-get endpoint ─┐ (sin dep)
Fase 1b  ─┘  → mergeable ya                 deploy_gcp.sh      ─┘ → desplegable ya
Fase 2 (contra mock) ───────┐
                            └──── E2E ─────► requiere ADP sign-get desplegado
```

- **Despliegue Fase 2:** ADP despliega `sign-get` **primero** (aditivo). Luego colmena mergea Fase 2 a `develop`; el worker lo consume en su próximo Cloud Build con el endpoint ya disponible.
- Fases 1 y 1b pueden mergear y desplegarse sin esperar a ADP.

---

## 9. Cobertura

| Caso de uso | Fase 1 | Fase 1b | Fase 2 |
|---|---|---|---|
| Generar + editar en el **mismo turno** (síntoma actual) | ✅ | — | ✅ |
| Editar imagen **generada en turno anterior** | ⚠️ solo si mismo proceso vivo | — | ✅ |
| Editar imagen **subida por el usuario** (turno previo) | ❌ | (visible) | ✅ |
| Upload visible aunque cambie el modelo del agente | — | ✅ | — |
| No inducir inyección de docs legacy no inyectables | — | ✅ | — |
| `image_edit` con URL externa / `data:` | ✅ (ya) | — | ✅ |
| `$attachment:<id>` a `http_request` (cross-process) | — | — | ✅ |

---

## 10. Archivos tocados (solo colmena)

- `dag_engine/infrastructure/nodes/image_edit.rs` — resolver `$attachment` (F1)
- `dag_engine/infrastructure/registry.rs` — cablear resolver (F1)
- `dag_engine/infrastructure/nodes/llm.rs` — filtro de catálogo (F1b)
- `llm/application/attachment_catalog.rs` — hint condicional a `storage_key` (F1b)
- `storage/infrastructure/http_callback_adapter.rs` — sign-get cross-process (F2)
