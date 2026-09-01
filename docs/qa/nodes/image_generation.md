# QA — Nodo `image_generation`

**Fuente de código:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs`

**Fuentes de doc revisadas:**
- `docs/node_configurations.json` (entrada `image_generation`)
- `docs/node_as_tools_reference.json` (ejemplos de tool config)
- `docs/agent_context/node_ports_reference.md` (puertos y defaults)
- `docs/developer_guide/32_multimedia_generation.md` (guía completa)

---

## 1) Config documentada NO soportada por el código

**Sin discrepancias detectadas.**

Todas las configuraciones documentadas son soportadas. Verificaciones spot:
- `size` default 1024x1024 (imagen_generation.rs:239) ✓
- `n` clamped 1-10 (imagen_generation.rs:234) ✓
- `quality` solo en provider=openai (imagen_generation.rs:251-259) ✓
- `google_project_id`/`google_location` fallback a env vars (imagen_generation.rs:280-281, 295-296) ✓
- Output sin `attachment_id`/`url` legacy (Plan B, imagen_generation.rs:384-394) ✓
- Auto-registration en AttachmentRegistry (imagen_generation.rs:349-382) ✓

---

## 2) Código NO documentado

### 2.1 Inputs engine-injected (`__colmena_session_id`, `__colmena_agent_session_id`)

**Ubicación:** `image_generation.rs:175-182`

El código lee dos inputs inyectados por el engine:
```rust
let session_id = inputs.get("__colmena_session_id").and_then(|v| v.as_str()).map(String::from);
let agent_session_id = inputs.get("__colmena_agent_session_id").and_then(|v| v.as_str()).map(String::from);
```

Estos se propagan a `StoreRequest` (línea 329-330) para que el `OutputStorageRepository` derive la ruta conversation-scoped.

**Impacto:** La doc en `node_configurations.json` menciona que el nodo persiste outputs vía `OutputStorageRepository`, pero NO documenta explícitamente que estos IDs de sesión deben estar presentes en inputs para que la persistencia sea conversation-scoped (defaultan a `None` para CLI runs).

**Recomendación QA:** No es un error (el comportamiento es correcto: None → ruta genérica), pero valdría aclarar en node_configurations.json que estos campos engine-injected son opcionales y usan rutas de alcance diferente según su presencia.

### 2.2 Inyección de secure-values

**Ubicación:** `image_generation.rs:186-194`

El nodo inyecta `<value_N>` placeholders si el servicio está wired:
```rust
if let Some(svc) = &self.secure_values {
    let _ = svc.inject_secrets(&mut cfg, svc_session, agent_session_id.as_deref()).await?;
}
```

**Impacto:** La doc dice "supports... secure-value placeholders" para `api_key`, pero NO especifica que esto **solo funciona** si el engine wired `secure_values: Some(Arc<SecureValueService>)`. Un usuario que intente `api_key: "<value_1>"` en un graph sin engine con secure-values active verá error "api_key is required when provider=openai" (porque el placeholder no se resolvió).

**Recomendación QA:** Documentar en node_configurations.json que secure-value resolution requiere que el engine lo haya inicializado (standard en `EngineConfig::from_env`, pero no siempre en tests/CLI).

### 2.3 Token caching de Vertex AI (~50 min TTL)

**Ubicación:** `image_generation.rs:101-102, 587-623`

El nodo cachea el access token en una `tokio::sync::Mutex<Option<CachedToken>>` con TTL ~50 minutos. Cada call chequea si `expires_at > now + 60s` antes de revalidar.

**Impacto:** La developer_guide.md dice "Token is cached for ~50 min" pero no documenta:
- Que es un estado compartido entre calls (el token persiste en la instancia del nodo)
- Que la latencia de reauth es >100ms (yup-oauth2 hace round-trip a Google)
- Que si el key file (GOOGLE_APPLICATION_CREDENTIALS) cambia durante execution, el token sigue siendo válido hasta su TTL

**Recomendación QA:** Documentar el TTL + la implicación de que múltiples calls en rápida sucesión (< 60s antes de expiración) reutilizan el mismo token sin reauth.

### 2.4 Schema fields (`default_input`, `default_output`)

**Ubicación:** `image_generation.rs:434-443`

El nodo expone:
- `default_input() = "prompt"` (línea 435)
- `default_output() = "output"` (línea 442)

**Impacto:** 
- `node_ports_reference.md` dice "Output (Plan B, 2026-05-25): `{ images: [{document_id, mime_type, size_bytes, description}], provider, model }`" pero NO menciona que el default_output es `"output"`, así que downstream que no especifique un path JSON recibe `{ images, provider, model }` directamente.
- No documenta que `default_input = "prompt"` significa que edges sin destino explícito inyectan en inputs["prompt"].

**Recomendación QA:** Añadir a node_ports_reference.md en la fila de image_generation: `default_input = prompt`, `default_output = output`.

### 2.5 Mapping MIME type → extensión

**Ubicación:** `image_generation.rs:317-322`

```rust
let filename = match mime_type.as_str() {
    "image/png" => format!("image_{}.png", i),
    "image/jpeg" => format!("image_{}.jpg", i),
    "image/webp" => format!("image_{}.webp", i),
    _ => format!("image_{}.bin", i),
};
```

**Impacto:** Si un provider devuelve un MIME type inesperado, cae a `.bin`. OpenAI siempre devuelve `image/png`. Vertex devuelve `image/png` o `image/webp` según el modelo. No documentado cuál es el fallback.

**Recomendación QA:** No es un bug (`.bin` es un fallback seguro), pero documentar que solo `.png`, `.jpg`, `.webp` son mapeados; otros mimetypes reciben `.bin`.

### 2.6 Registry registration is fail-soft

**Ubicación:** `image_generation.rs:372-381`

```rust
if let Err(e) = reg.upsert(upsert).await {
    tracing::warn!(...);  // warn pero NO fallar la generación
}
```

**Impacto:** Si la registry está fuera de servicio, la generación sigue adelante. El output aún tiene `document_id`, pero `load_attachment` no podrá encontrarlo después. La doc dice "Auto-registers... so that `load_attachment` can later resolve" pero no advierte que falla silenciosamente si registry está offline.

**Recomendación QA:** Documentar que auto-registration es fail-soft. Si `load_attachment` falla después, revisar logs para ver si hubo warn de registry fail.

### 2.7 OpenAI URL base override (test-only)

**Ubicación:** `image_generation.rs:104-108, 135-139, 153-159`

El nodo tiene un `#[cfg(test)]` override de la OpenAI base URL para wiremock:

```rust
#[cfg(test)]
test_openai_base_url: Option<String>,
```

**Impacto:** Zero impacto en prod (compilado out). Solo relevante para tests internos.

---

## 3) Plan de pruebas QA

### TC-IG-001: Happy path OpenAI (gpt-image-1)

**Objetivo:** Verificar que image_generation genera y persiste una imagen vía OpenAI.

**Grafo mínimo (`tests/graphs/media/image_generation_openai.json`):**
```json
{
  "nodes": [
    {
      "id": "generate",
      "node_type": "image_generation",
      "config": {
        "provider": "openai",
        "model": "gpt-image-1",
        "api_key": "${OPENAI_API_KEY}",
        "prompt": "A minimalist red circle"
      }
    },
    {
      "id": "output",
      "node_type": "output",
      "inputs": { "input": "generate" }
    }
  ]
}
```

**Comando:**
```bash
set -a && source .env && set +a
cargo run --bin dag_engine -- run tests/graphs/media/image_generation_openai.json
```

**Resultado esperado:**
- Exit code 0
- Output JSON contiene `{ "output": { "images": [...], "provider": "openai", "model": "gpt-image-1" } }`
- `images[0].document_id` comienza con `img_`
- `images[0].mime_type` es `image/png`
- `images[0].size_bytes > 0`
- `images[0]` NO contiene `attachment_id` o `url` (Plan B)
- En disco `/tmp/colmena-out/image_0.png` existe (si `COLMENA_LOCAL=true`)

**Credenciales requeridas:** `OPENAI_API_KEY` en `.env`

---

### TC-IG-002: Happy path Google Vertex (imagen-4.0-generate-001)

**Objetivo:** Verificar que image_generation funciona con Google Vertex AI + ADC.

**Grafo mínimo (`tests/graphs/media/image_generation_vertex.json`):**
```json
{
  "nodes": [
    {
      "id": "generate",
      "node_type": "image_generation",
      "config": {
        "provider": "google",
        "model": "imagen-4.0-generate-001",
        "prompt": "A serene landscape with mountains"
      }
    },
    {
      "id": "output",
      "node_type": "output",
      "inputs": { "input": "generate" }
    }
  ]
}
```

**Comando:**
```bash
# Vertex requiere ADC. Si usas gcloud user creds:
gcloud auth application-default login
# O setear GOOGLE_APPLICATION_CREDENTIALS=/path/to/sa-key.json

set -a && source .env && set +a
GOOGLE_CLOUD_PROJECT=<your-gcp-project> \
  cargo run --bin dag_engine -- run tests/graphs/media/image_generation_vertex.json
```

**Resultado esperado:**
- Exit code 0
- `images[0].mime_type` es `image/png` o `image/webp`
- `images[0].size_bytes > 0`
- Log contiene `storage_mode_selected` con adapter info

**Credenciales requeridas:** GCP ADC (gcloud user creds o SA key file)

---

### TC-IG-003: Size parameter (non-default 1792x1024)

**Objetivo:** Verificar que `size` se resuelve desde config e impacta la generación.

**Grafo:**
```json
{
  "id": "generate",
  "node_type": "image_generation",
  "config": {
    "provider": "openai",
    "model": "gpt-image-1",
    "api_key": "${OPENAI_API_KEY}",
    "prompt": "A wide landscape",
    "size": "1792x1024"
  }
}
```

**Resultado esperado:**
- Imagen generada con aspect ratio 1792x1024 (visible inspeccionando el archivo)
- `size_bytes` mayor que 1024x1024 típicamente

---

### TC-IG-004: `n` parameter (multiple images)

**Objetivo:** Verificar que `n=2` genera dos imágenes + persiste ambas.

**Grafo:**
```json
{
  "id": "generate",
  "node_type": "image_generation",
  "config": {
    "provider": "openai",
    "model": "gpt-image-1",
    "api_key": "${OPENAI_API_KEY}",
    "prompt": "A cat",
    "n": 2
  }
}
```

**Resultado esperado:**
- `images.length === 2`
- `images[0].document_id` y `images[1].document_id` son distintos
- Ambas imágenes persistidas en storage (2 archivos PNG en `/tmp/colmena-out/` si COLMENA_LOCAL=true)

---

### TC-IG-005: `n` clamped a max 10

**Objetivo:** Verificar que `n > 10` se clampea a 10 (no rechaza, no genera 11+).

**Grafo:**
```json
{
  "config": {
    "n": 15
  }
}
```

**Resultado esperado:**
- `images.length === 10` (clamped, no error)
- No hay warning ni error — silenciosamente clamped

---

### TC-IG-006: Default `size` (1024x1024)

**Objetivo:** Verificar que omitir `size` usa el default.

**Grafo sin `size`:**
```json
{
  "config": {
    "provider": "openai",
    "model": "gpt-image-1",
    "api_key": "${OPENAI_API_KEY}",
    "prompt": "A circle"
  }
}
```

**Resultado esperado:**
- Imagen generada 1024x1024
- No error, comportamiento silencioso

---

### TC-IG-007: Default `n` (1)

**Objetivo:** Verificar que omitir `n` usa default 1.

**Resultado esperado:**
- `images.length === 1`

---

### TC-IG-008: Env var resolution for OPENAI_API_KEY

**Objetivo:** Verificar que `api_key: "${OPENAI_API_KEY}"` se resuelve desde env.

**Setup:**
```bash
export OPENAI_API_KEY=sk-proj-...
```

**Grafo:**
```json
{
  "config": {
    "api_key": "${OPENAI_API_KEY}"
  }
}
```

**Resultado esperado:**
- Key resuelto, request exitoso

---

### TC-IG-009: Env var resolution for GOOGLE_CLOUD_PROJECT

**Objetivo:** Verificar que omitir `google_project_id` fallback a env var.

**Setup:**
```bash
export GOOGLE_CLOUD_PROJECT=my-project
```

**Grafo (sin google_project_id):**
```json
{
  "config": {
    "provider": "google",
    "model": "imagen-4.0-generate-001",
    "prompt": "..."
  }
}
```

**Resultado esperado:**
- Project resuelto desde env, no error

---

### TC-IG-010: Env var fallback chain (GOOGLE_CLOUD_PROJECT → GOOGLE_PROJECT_ID)

**Objetivo:** Verificar que `GOOGLE_PROJECT_ID` se usa si `GOOGLE_CLOUD_PROJECT` no existe.

**Setup:**
```bash
unset GOOGLE_CLOUD_PROJECT
export GOOGLE_PROJECT_ID=my-project
```

**Resultado esperado:**
- Project resuelto desde GOOGLE_PROJECT_ID

---

### TC-IG-011: Missing provider error

**Objetivo:** Verificar que omitir `provider` falla con mensaje útil.

**Grafo sin provider:**
```json
{
  "config": {
    "model": "gpt-image-1",
    "api_key": "...",
    "prompt": "..."
  }
}
```

**Resultado esperado:**
- Error: `"image_generation: provider is required (openai|google)"`
- Exit code ≠ 0

---

### TC-IG-012: Missing model error

**Objetivo:** Verificar que omitir `model` falla.

**Resultado esperado:**
- Error: `"image_generation: model is required"`

---

### TC-IG-013: Missing prompt error

**Objetivo:** Verificar que omitir `prompt` falla.

**Resultado esperado:**
- Error: `"image_generation: prompt is required (via inputs or config)"`

---

### TC-IG-014: Missing api_key (OpenAI) error

**Objetivo:** Verificar que omitir `api_key` cuando `provider=openai` falla.

**Resultado esperado:**
- Error: `"image_generation: api_key is required when provider=openai"`

---

### TC-IG-015: Missing google_project_id (Google) error

**Objetivo:** Verificar que omitir `google_project_id` cuando `provider=google` Y env vars no set falla.

**Setup:**
```bash
unset GOOGLE_CLOUD_PROJECT GOOGLE_PROJECT_ID
```

**Resultado esperado:**
- Error mention `google_project_id is required when provider=google` + hint sobre env vars

---

### TC-IG-016: Unknown provider error

**Objetivo:** Verificar que `provider=midjourney` (inválido) falla.

**Resultado esperado:**
- Error: `"image_generation: unknown provider 'midjourney' (expected openai|google)"`

---

### TC-IG-017: Inputs override config (prompt)

**Objetivo:** Verificar que `inputs.prompt` toma precedencia sobre `config.prompt`.

**Grafo + inputs:**
```
config: { prompt: "config prompt" }
inputs: { prompt: "inputs prompt" }
```

**Resultado esperado:**
- Imagen generada usando "inputs prompt"
- En la descripción del resultado aparece "inputs prompt"

---

### TC-IG-018: Inputs override config (size, n, quality)

**Objetivo:** Verificar que `inputs.size` y `inputs.n` se aplican (si presentes).

**Grafo + inputs:**
```
config: { size: "1024x1024", n: 1, quality: "low" }
inputs: { size: "1792x1024", n: 2 }
```

**Resultado esperado:**
- 2 imágenes generadas con size 1792x1024

---

### TC-IG-019: Tool-execution path (config empty, all in inputs)

**Objetivo:** Verificar que cuando se usa como LLM tool con `node_schema.fixed`, los values llegan via inputs no config.

**Contexto:** `dag_tool_executor.rs` pasa `config={}` + merges `fixed` values into `inputs`.

**Setup LLM tool:**
```json
{
  "node_type": "image_generation",
  "node_schema": {
    "provider": { "fixed": "openai" },
    "model": { "fixed": "gpt-image-1" },
    "api_key": { "fixed": "${OPENAI_API_KEY}" },
    "prompt": { "type": "string", "required": true }
  }
}
```

**Resultado esperado:**
- Nodo resuelve values desde inputs (no config), no falla

---

### TC-IG-020: Auto-registration in AttachmentRegistry

**Objetivo:** Verificar que generate image registra automáticamente en AttachmentRegistry con `provider=Generated`.

**Setup:**
- Engine wired con AttachmentRegistry (default en `EngineConfig::from_env`)
- `__colmena_agent_session_id` inyectado en inputs

**Grafo:**
```
inputs: { __colmena_agent_session_id: "agent_test_1" }
```

**Resultado esperado:**
- Registry.lookup_by_document_id("agent_test_1", <document_id>) retorna un row
- Row tiene `provider=Generated`, `storage_key` poblado, `origin=generated_by:image_generation`

---

### TC-IG-021: No registry (skip registration, still emit document_id)

**Objetivo:** Verificar que sin registry, el nodo sigue emitiendo `document_id` (no falla, registration only fail-soft).

**Setup:**
- Engine without AttachmentRegistry (CLI run, o test mock sin registry)

**Resultado esperado:**
- Output tiene `document_id`
- No error ni warning sobre registry

---

### TC-IG-022: COLMENA_LOCAL=true (LocalHttpStorageAdapter)

**Objetivo:** Verificar que artifacts van a `/tmp/colmena-out/` + local HTTP server.

**Setup:**
```bash
export COLMENA_LOCAL=true
mkdir -p /tmp/colmena-out
```

**Resultado esperado:**
- Log: `storage_mode_selected mode=local adapter=LocalHttpStorageAdapter dir=/tmp/colmena-out port=8765`
- `images[0].mime_type` is `image/png`
- File exists en `/tmp/colmena-out/image_0.png`
- Output `document_id` starts with `img_`

---

### TC-IG-023: COLMENA_LOCAL=false (HttpCallbackStorageAdapter) + callback missing

**Objetivo:** Verificar que omitir callback URL cuando `COLMENA_LOCAL=false` explícito falla en startup.

**Setup:**
```bash
export COLMENA_LOCAL=false
unset COLMENA_STORAGE_CALLBACK_URL
```

**Resultado esperado:**
- Engine init panics con mensaje pedante: "HttpCallbackStorageAdapter requires COLMENA_STORAGE_CALLBACK_URL..."

---

### TC-IG-024: Session IDs forwarded to storage

**Objetivo:** Verificar que `__colmena_session_id` + `__colmena_agent_session_id` se propagan a `StoreRequest`.

**Inputs:**
```
{ "__colmena_session_id": "ses_abc", "__colmena_agent_session_id": "agent_xyz" }
```

**Resultado esperado:**
- `StoreRequest.session_id == "ses_abc"`
- `StoreRequest.agent_session_id == "agent_xyz"`
- Storage adapter builds conversation-scoped path usando esos IDs

---

### TC-IG-025: Quality parameter (OpenAI only)

**Objetivo:** Verificar que `quality: "high"` se envía a OpenAI + no afecta Google.

**OpenAI grafo:**
```json
{
  "config": {
    "provider": "openai",
    "model": "gpt-image-1",
    "quality": "high"
  }
}
```

**Resultado esperado:**
- Request a OpenAI contiene `"quality": "high"`

---

### TC-IG-026: OpenAI error handling (e.g., 400)

**Objetivo:** Verificar que OpenAI 400 (bad prompt) propaga error útil.

**Setup mock:**
- Wiremock returns 400 "bad prompt"

**Resultado esperado:**
- Error contains "400" + "bad prompt"
- Exit code ≠ 0

---

### TC-IG-027: OpenAI empty response

**Objetivo:** Verificar que OpenAI response sin `data` array falla.

**Setup mock:**
- OpenAI returns `{ "error": "..." }` (no `data` key)

**Resultado esperado:**
- Error: `"openai response missing 'data' array"`

---

### TC-IG-028: Prompt visibility in description

**Objetivo:** Verificar que la imagen result description contiene el prompt (truncado a 80 chars).

**Grafo:**
```json
{
  "prompt": "A very long prompt with lots of details about what we want to generate and how it should look"
}
```

**Resultado esperado:**
- `images[0].description` starts with `"Image generated with gpt-image-1: A very long prompt with..."`
- Truncated at 80 chars

---

### TC-IG-029: Google Vertex token caching (>1 call in rapid succession)

**Objetivo:** Verificar que el token se cachea y reutiliza dentro del TTL (~50 min).

**Setup:**
- Run dos image_generation calls en el mismo proceso (v.g., en un subgraph con dos image nodes)
- Monitorear que solo UNA call a `yup-oauth2` ocurra

**Resultado esperado:**
- Segundo call reutiliza el token (latencia <<100ms vs ~500ms for reauth)
- Traces show only one `get_vertex_token` "fetch Vertex access token"

---

### TC-IG-030: Google location default (us-central1)

**Objetivo:** Verificar que omitir `google_location` usa default `us-central1`.

**Setup:**
```
No google_location en config
Unset GOOGLE_CLOUD_LOCATION / GOOGLE_LOCATION env vars
```

**Resultado esperado:**
- Vertex call dirigido a `us-central1-aiplatform.googleapis.com`
- No error

---

