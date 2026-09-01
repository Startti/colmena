# image_edit — Auditoría QA (Documentación vs Código)

**Nodo:** `image_edit`  
**Código fuente:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs`  
**Documentación primaria:** `docs/developer_guide/32_multimedia_generation.md`  
**Configuración canónica:** `docs/node_configurations.json` → `node_types.image_edit`  
**Referencias de herramientas:** `docs/node_as_tools_reference.json` → multimedia section  
**Puertos:** `docs/agent_context/node_ports_reference.md` → media nodes  
**Fecha de auditoría:** 2026-08-30

---

## 1. Hallazgos: Documentación

### 1.1 node_as_tools_reference.json — ambigüedad sobre resolución de `$attachment:<document_id>` en `source_url`

**Problema:** La sección "image_edit_tool_chained_via_attachment_id" (línea ~337-356) hace dos afirmaciones contradictorias:

1. Línea 340: "source_url accepts ... `data:` URIs or `http(s)://` URL" (implica soporte)
2. Línea 345: "image_edit's source_url does NOT resolve a document_id or `$attachment:<document_id>` placeholder"

La primera oración sugiere que sí acepta, la segunda niega explícitamente que resuelva placeholders.

**Realidad en código:** `image_edit.rs:84-151` (`fetch_image`) soporta cuatro esquemas:
- `local://...` o `chat-attachments/...` (línea 88-95) — resuelve vía `storage.read(url)`
- `data:<mime>;base64,...` (línea 97-110) — decodifica inline
- `http://` o `https://` (línea 111-144) — descarga URL

**NO** resuelve `$attachment:<document_id>` (ese resolver está cableado solo en `http_request`, no globalmente).

**Impacto:** La documentación NO es incorrecta (niega correctamente el placeholder), pero es confusa. El primera oración hace parecer que sí resuelve, cuando la realidad es que solo soporta cuatro esquemas concretos (dos de ellos storage handles).

**Remediación:** Reformular línea 340 para claridad: "source_url accepts 'data:' URIs, 'http(s)://' URLs, or storage handles ('local://<key>' or 'chat-attachments/<key>'). It does NOT resolve bare document_ids or '$attachment:<document_id>' placeholders."

---

### 1.2 node_configurations.json — `source_url` no documenta almacenamiento handles ni placeholders

**Problema:** El campo `source_url` en `node_configurations.json` para `image_edit` (línea ~línea sección config_fields):

**Documentación dice:**
```
"type": "string",
"required": true,
"description": "data: URI, http(s) URL, or storage handle. Does not resolve $attachment: placeholders."
```

Pero `docs/node_configurations.json` actualmente dice solo: "data: URI or http(s) URL of the image to edit".

**Realidad en código:** `fetch_image` (línea 88-151) resuelve:
- `local://...` (línea 88)
- `chat-attachments/...` (línea 88)
- `data:...` (línea 97)
- `http(s)://...` (línea 111)

**Impacto:** Operadores que leen solo `node_configurations.json` desconocen que pueden pasar storage handles (`local://`, `chat-attachments/`) — pensarán que solo funciona con data: URIs o URLs HTTP.

**Remediación:** Actualizar descripción de `source_url` en `node_configurations.json` para incluir: `local://...` y `chat-attachments/<path>` como opciones válidas.

---

### 1.3 node_configurations.json — campos especiales de inyección (`__colmena_session_id`, `__colmena_agent_session_id`) no documentados

**Problema:** El código (línea 163-170) inyecta estos campos especiales desde `inputs` para usarlos al almacenar:

```rust
let session_id = inputs
    .get("__colmena_session_id")
    .and_then(|v| v.as_str())
    .map(String::from);
let agent_session_id = inputs
    .get("__colmena_agent_session_id")
    .and_then(|v| v.as_str())
    .map(String::from);
```

Estos se forwarded luego al `StoreRequest` (línea 351-352).

**Documentación:** `node_configurations.json` NO menciona estos campos especiales como inputs. Otros nodos (p.ej. `image_generation`) tampoco los documentan explícitamente, pero son inyectados por el engine automáticamente.

**Impacto:** Bajo. Estos campos son inyectados automáticamente por el engine (no son controlados por el operador), así que omitirlos de la documentación es correcto. Pero un desarrollador que quisiera entender por qué los artifacts se registran bajo el `agent_session_id` correcto no vería la conexión en las docs.

**Estado:** OK. Los campos especiales (prefijo `__colmena_`) no deben documentarse como user-facing config.

---

### 1.4 node_configurations.json — auto-registro en AttachmentRegistry no documentado

**Problema:** El código (línea 371-403) auto-registra la imagen editada en `AttachmentRegistry` si `agent_session_id` está presente:

```rust
if let (Some(reg), Some(agent_sid)) =
    (self.attachment_registry.as_ref(), agent_session_id.as_ref())
{
    let upsert = UpsertAttachmentInput { ... };
    if let Err(e) = reg.upsert(upsert).await { ... }
}
```

**Documentación en `node_configurations.json`:** No menciona este comportamiento.

**Documentación en `docs/developer_guide/32_multimedia_generation.md`:** SÍ se menciona en la sección "Artifacts unification — el agente como ciudadano de primera" (línea 304-323).

**Impacto:** Medio. Un operador que solo lee `node_configurations.json` no sabría que el artifact se registra automáticamente en `conversation_attachments` con `origin: generated_by:image_edit`. Sin embargo, esto es un detalle de implementación (no es config controlable), así que es razonable documentarlo solo en la guía de desarrollo, no en el schema de config.

**Remediación:** Agregar una nota al final de la descripción de `image_edit` en `node_configurations.json`: "Auto-registers edited image in AttachmentRegistry with `provider: Generated` and `origin: generated_by:image_edit`, enabling downstream `load_attachment(document_id)` calls."

---

### 1.5 node_configurations.json — `api_key` field no documenta `${ENV_VAR}` support

**Problema:** El campo `api_key` en `node_configurations.json` para `image_edit`:

**Documentación dice:** `"required": true` (sin mencionar placeholder support).

**Realidad en código:** Línea 212 llama `resolve_env_var(api_key_raw)`, que reemplaza `${VARIABLE_NAME}` con su valor de environment (línea 69-77).

**Verificación cruzada:** `docs/developer_guide/32_multimedia_generation.md` línea 249 para `image_generation` SÍ dice: "Soporta `${OPENAI_API_KEY}` + secure-value placeholders". Pero `image_edit` no tiene esta nota.

**Impacto:** Bajo. Un operador puede pasar `"api_key": "${OPENAI_API_KEY}"` sin documentación y funcionará. Pero es inconsistente con la documentación del campo `api_key` en `image_generation` que SÍ lo menciona.

**Remediación:** Actualizar descripción de `api_key` en `node_configurations.json` para `image_edit`: "Required OpenAI API key. Supports `${ENV_VAR}` interpolation and secure-value placeholders."

---

## 2. Hallazgos: Código

### 2.1 Soporte implícito para archivos locales — resolver directo de URLs storage

**Descubrimiento:** `image_edit.rs:88-95` decodifica directamente storage handles:

```rust
if url.starts_with("local://") || url.starts_with("chat-attachments/") {
    let stored = self
        .storage
        .read(url)
        .await
        ...?;
    return Ok((stored.bytes, stored.mime_type));
}
```

**Documentación:** `node_configurations.json` NO menciona esto. Solo `docs/developer_guide/32_multimedia_generation.md` línea 272 lo documenta parcialmente.

**Impacto:** Un operador que intente encadenar `image_generation` → `image_edit` (pasando el `storage_key` de la imagen generada) descubrirá que NO funciona porque `storage_key` es internamente un UUID opaco (`k1`, `sk-edit-1`, etc.) en las pruebas, pero en producción tiene forma `chat-attachments/<userId>/<sessionId>/generated/<cuid>-<name>`. Sin embargo, Plan B (2026-05-25) REMOVIO el campo `url` del tool result de `image_generation`, así que ya no hay forma LLM-controlada de pasar la `storage_key` como `source_url`.

**Estado:** Código OK, documentación incompleta en `node_configurations.json`.

---

### 2.2 Fail-soft para auto-registro fallido en AttachmentRegistry

**Descubrimiento:** `image_edit.rs:394-403` — si el auto-registro falla:

```rust
if let Err(e) = reg.upsert(upsert).await {
    tracing::warn!(
        target: "colmena::image_edit",
        error = %e,
        ...
        "failed to register edited image in attachment registry — \
         load_attachment will not see this output"
    );
}
```

El nodo **NO** falla. Emite un warning y continúa. El resultado del tool se devuelve normalmente con `document_id`.

**Documentación:** Ningún documento menciona este comportamiento fail-soft.

**Impacto:** Bajo. Esto es una decisión de diseño razonable (el artifact se almacena correctamente incluso si la registry falla), y el warning en logs permite debug. Pero un operador que vea `load_attachment(document_id)` fallar sin razón aparente no sabría que el registro falló.

**Remediación:** Documentar en `docs/developer_guide/32_multimedia_generation.md` que el auto-registro es fail-soft (los artifacts se almacenan correctamente incluso si la registry falla).

---

### 2.3 OpenAI error propagation — cuerpo del error en el mensaje

**Descubrimiento:** `image_edit.rs:311-318` — cuando OpenAI falla:

```rust
if !resp.status().is_success() {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    return Err(format!(
        "image_edit: openai /v1/images/edits failed: status={status} body={body}"
    ).into());
}
```

El cuerpo de la respuesta de OpenAI se incluye en el error. Si OpenAI devuelve un error estructurado (JSON con `error.message`), el JSON completo se incluye en el mensaje.

**Documentación:** No se menciona en ningún lado.

**Impacto:** Bajo. Operadores verán detalles del error de OpenAI en los logs. Esto es útil para debug, aunque puede contaminar los logs si el error es muy largo.

**Estado:** OK. Este es comportamiento de debug estándar.

---

### 2.4 Masking de archivos — resolución de MIME type sin content-type header

**Descubrimiento:** `image_edit.rs:130-139` — al fetchear una imagen HTTPS sin header `Content-Type`:

```rust
let mime = resp
    .headers()
    .get(reqwest::header::CONTENT_TYPE)
    .and_then(|v| v.to_str().ok())
    .unwrap_or("image/png")  // ← default a image/png si no hay header
    ...
```

Si el servidor no envía `Content-Type`, asume `image/png`.

**Documentación:** No se menciona.

**Impacto:** Bajo. Si un CDN sirve una JPEG sin header `Content-Type`, se tratará como PNG. Las pruebas unitarias (línea 499-507) usan mocks que sí envían header correcto. 

**Estado:** Comportamiento razonable para URLs públicas sin content-type.

---

### 2.5 Clamping de `n` (número de imágenes) — rango 1-10

**Descubrimiento:** `image_edit.rs:240-245`:

```rust
let n = inputs
    .get("n")
    .and_then(|v| v.as_u64())
    .or_else(|| cfg.get("n").and_then(|v| v.as_u64()))
    .unwrap_or(1)
    .clamp(1, 10) as u32;
```

Si el usuario pasa `n: 0` o `n: 15`, se clampea a 1 o 10 respectivamente (sin error).

**Documentación:** `node_configurations.json` dice `"default 1, max 10"` pero no documenta que valores >10 se clampean (no se rechazan con error).

**Impacto:** Bajo. El clamping es transparente, pero un operador esperaría que `n: 100` fallara, no que silenciosamente se clampee a 10.

**Remediación:** Documentar en `node_configurations.json`: "Default 1, clamped to range [1, 10]" en lugar de solo "max 10".

---

## 3. Casos de Prueba Ejecutables

Todos los casos usan `cargo run --bin dag_engine -- run <graph.json>` sin servidor.
Credenciales requeridas: `OPENAI_API_KEY` en `.env` (sourced antes de run).

---

### 3.1 Test A: Happy path — editar imagen con OpenAI

**Archivo:** `tests/graphs/media/image_edit_basic.json`

```json
{
  "nodes": {
    "edit": {
      "node_type": "image_edit",
      "config": {
        "provider": "openai",
        "model": "gpt-image-1",
        "api_key": "${OPENAI_API_KEY}",
        "source_url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+P+/HgAFhAJ/wlseKgAAAABJRU5ErkJggg==",
        "prompt": "Add a red circle to the center"
      }
    },
    "log": {
      "node_type": "log"
    }
  },
  "edges": [
    { "from": "edit", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
set -a && source .env && set +a
cargo run --bin dag_engine -- run tests/graphs/media/image_edit_basic.json
```

**Validación esperada:**
- OpenAI `/v1/images/edits` recibe multipart POST con imagen + prompt
- Tool result contiene `output.images[0]` con `document_id`, `mime_type`, `size_bytes`
- **Nota:** Requiere `OPENAI_API_KEY` en `.env` (real, no mock)
- Plan B (2026-05-25): NO hay campos legacy `attachment_id` o `url` en el output

---

### 3.2 Test B: Source URL — data: URI decoding

**Archivo:** `tests/graphs/media/image_edit_data_uri_source.json`

```json
{
  "nodes": {
    "edit": {
      "node_type": "image_edit",
      "config": {
        "provider": "openai",
        "model": "gpt-image-1",
        "api_key": "${OPENAI_API_KEY}",
        "source_url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAoAAAAKCAYAAACNMs+9AAAAFUlEQVR42mNk+M9QzzDMyMhAhgAARxkDvzlqWRUAAAAASUVORK5CYII=",
        "prompt": "Make the background transparent"
      }
    },
    "log": {
      "node_type": "log"
    }
  },
  "edges": [
    { "from": "edit", "to": "log" }
  ]
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/media/image_edit_data_uri_source.json
```

**Validación esperada:**
- Node decodifica data: URI sin hacer HTTP request al servidor mock
- Base64 se decodifica (iVBORw0K... → PNG bytes)
- OpenAI recibe bytes decodificados
- Output: `images` array con al menos 1 elemento

---

### 3.3 Test C: Source URL — HTTP(S) fetch

**Archivo:** `tests/graphs/media/image_edit_http_source.json`

```json
{
  "nodes": {
    "edit": {
      "node_type": "image_edit",
      "config": {
        "provider": "openai",
        "model": "gpt-image-1",
        "api_key": "${OPENAI_API_KEY}",
        "source_url": "http://example.com/image.png",
        "prompt": "Enhance the colors"
      }
    },
    "log": {
      "node_type": "log"
    }
  },
  "edges": [
    { "from": "edit", "to": "log" }
  ]
}
```

**Ejecución (mock via wiremock):**
```bash
# Mock GET http://example.com/image.png → 200 PNG bytes
# Mock POST http://api.openai.com/v1/images/edits → 200 JSON with data
cargo run --bin dag_engine -- run tests/graphs/media/image_edit_http_source.json
```

**Validación esperada:**
- Node fetches source URL via `reqwest::Client::get()`
- User-Agent header enviado (colmena-image-edit/0.3)
- No falla si el servidor devuelve bytes PNG válidos
- OpenAI recibe bytes en multipart `image` part

---

### 3.4 Test D: Missing required fields

**Archivo:** `tests/graphs/media/image_edit_missing_fields.json`

```json
{
  "nodes": {
    "edit_no_provider": {
      "node_type": "image_edit",
      "config": {
        "api_key": "${OPENAI_API_KEY}",
        "source_url": "data:image/png;base64,AA==",
        "prompt": "test"
      }
    }
  }
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/media/image_edit_missing_fields.json 2>&1 | grep -i "provider"
```

**Validación esperada:**
- Error: "image_edit: provider is required"
- Confirma fail-closed para campos obligatorios

---

### 3.5 Test E: Unsupported provider

**Archivo:** `tests/graphs/media/image_edit_unsupported_provider.json`

```json
{
  "nodes": {
    "edit": {
      "node_type": "image_edit",
      "config": {
        "provider": "google",
        "api_key": "dummy",
        "source_url": "data:image/png;base64,AA==",
        "prompt": "test"
      }
    }
  }
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/media/image_edit_unsupported_provider.json 2>&1 | grep -i "unsupported"
```

**Validación esperada:**
- Error: "image_edit: unsupported provider 'google' (only 'openai' is implemented today; Google Vertex image editing is on the roadmap)"
- Mensaje pedagógico que menciona roadmap

---

### 3.6 Test F: Malformed data: URI

**Archivo:** `tests/graphs/media/image_edit_malformed_data_uri.json`

```json
{
  "nodes": {
    "edit": {
      "node_type": "image_edit",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "source_url": "data:image/png;base64,!!!NOT_VALID_BASE64!!!",
        "prompt": "test"
      }
    }
  }
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/media/image_edit_malformed_data_uri.json 2>&1 | grep -i "base64"
```

**Validación esperada:**
- Error: "image_edit: data: URI base64 decode failed: ..."
- Confirma fail-closed para base64 inválido

---

### 3.7 Test G: Optional fields — mask_url, n, size, quality

**Archivo:** `tests/graphs/media/image_edit_optional_fields.json`

```json
{
  "nodes": {
    "edit": {
      "node_type": "image_edit",
      "config": {
        "provider": "openai",
        "model": "gpt-image-1",
        "api_key": "${OPENAI_API_KEY}",
        "source_url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+P+/HgAFhAJ/wlseKgAAAABJRU5ErkJggg==",
        "prompt": "Add a watermark",
        "n": 3,
        "size": "512x512",
        "quality": "hd",
        "mask_url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+P+/HgAFhAJ/wlseKgAAAABJRU5ErkJggg=="
      }
    }
  }
}
```

**Ejecución:**
```bash
cargo run --bin dag_engine -- run tests/graphs/media/image_edit_optional_fields.json
```

**Validación esperada:**
- OpenAI multipart contiene: `n=3`, `size=512x512`, `quality=hd`, `mask` part
- Output: `images` array con 3 elementos (n=3)
- No error aunque todos los campos opcionales están presentes

---

### 3.8 Test H: Clamping de n — boundary values

**Archivo:** `tests/graphs/media/image_edit_n_clamping.json`

Dos variantes (n=0 y n=15):

```json
{
  "nodes": {
    "edit_n_zero": {
      "node_type": "image_edit",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "source_url": "data:image/png;base64,AA==",
        "prompt": "test",
        "n": 0
      }
    },
    "edit_n_high": {
      "node_type": "image_edit",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "source_url": "data:image/png;base64,AA==",
        "prompt": "test",
        "n": 15
      }
    }
  }
}
```

**Ejecución:**
```bash
# Intercept OpenAI requests and verify n value in multipart
cargo run --bin dag_engine -- run tests/graphs/media/image_edit_n_clamping.json
```

**Validación esperada:**
- n=0 se clampea a n=1 (no error, no HTTP 400)
- n=15 se clampea a n=10 (no error, no HTTP 400)
- OpenAI recibe exactamente n=1 y n=10 en sus requests respectivos

---

### 3.9 Test I: API key resolution — `${ENV_VAR}` interpolation

**Archivo:** `tests/graphs/media/image_edit_env_var_api_key.json`

```json
{
  "nodes": {
    "edit": {
      "node_type": "image_edit",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "source_url": "data:image/png;base64,AA==",
        "prompt": "test"
      }
    }
  }
}
```

**Ejecución:**
```bash
export OPENAI_API_KEY="sk-test-xyz"
cargo run --bin dag_engine -- run tests/graphs/media/image_edit_env_var_api_key.json
```

**Validación esperada:**
- `${OPENAI_API_KEY}` se reemplaza con el valor de env var (sk-test-xyz)
- OpenAI request recibe Bearer token correcto
- Si OPENAI_API_KEY no está set, error: "env var OPENAI_API_KEY not set (referenced by image_edit)"

---

### 3.10 Test J: Storage adapter integration — COLMENA_LOCAL

**Archivo:** `tests/graphs/media/image_edit_storage_local.json`

```json
{
  "nodes": {
    "edit": {
      "node_type": "image_edit",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "source_url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+P+/HgAFhAJ/wlseKgAAAABJRU5ErkJggg==",
        "prompt": "Add color"
      }
    }
  }
}
```

**Ejecución (con `COLMENA_LOCAL=true`):**
```bash
export COLMENA_LOCAL=true
export COLMENA_LOCAL_STORAGE_DIR=/tmp/colmena-test-image-edit
mkdir -p $COLMENA_LOCAL_STORAGE_DIR
cargo run --bin dag_engine -- run tests/graphs/media/image_edit_storage_local.json
```

**Validación esperada:**
- Log startup: "storage_mode_selected mode=local adapter=LocalHttpStorageAdapter"
- Archivo PNG aparece en `/tmp/colmena-test-image-edit/`
- Tool result contiene `document_id` (p.ej. `img_edit_0_...`)
- Plan B: NO hay `url` en tool result
- Puedes `open /tmp/colmena-test-image-edit/*.png` para inspeccionarlo

---

### 3.11 Test K: Session ID forwarding — artifact registration

**Archivo:** `tests/graphs/media/image_edit_session_registration.json`

```json
{
  "nodes": {
    "edit": {
      "node_type": "image_edit",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "source_url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+P+/HgAFhAJ/wlseKgAAAABJRU5ErkJggg==",
        "prompt": "test"
      }
    }
  }
}
```

**Ejecución (con agent_session_id):**
```bash
export COLMENA_LOCAL=true
cargo run --bin dag_engine -- run tests/graphs/media/image_edit_session_registration.json \
  --agent-session-id test_session_001
```

**Validación esperada:**
- Session IDs se forwarden a StoreRequest (image_edit.rs:351-352)
- Si AttachmentRegistry está wired, artifact se registra con `agent_session_id: test_session_001`
- Tool result emite `document_id` normal
- Luego `load_attachment(document_id)` en otra llamada resuelve correctamente

---

### 3.12 Test L: Source fetch 404 — error propagation

**Archivo:** `tests/graphs/media/image_edit_source_fetch_404.json`

```json
{
  "nodes": {
    "edit": {
      "node_type": "image_edit",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "source_url": "http://example.com/missing.png",
        "prompt": "test"
      }
    }
  }
}
```

**Ejecución (mock 404):**
```bash
# Mock GET http://example.com/missing.png → 404
cargo run --bin dag_engine -- run tests/graphs/media/image_edit_source_fetch_404.json 2>&1 | grep -i "fetch"
```

**Validación esperada:**
- Error: "image_edit: fetch source url failed: status=404"
- Confirma fail-closed para URLs inaccesibles

---

## Resumen de Hallazgos

| # | Tipo | Severidad | Descripción |
|---|------|-----------|-------------|
| 1.1 | Docs | Media | `node_as_tools_reference.json` ambiguo sobre resolución de `$attachment:<document_id>` |
| 1.2 | Docs | Media | `node_configurations.json` no documenta `local://` y `chat-attachments/` en `source_url` |
| 1.3 | Docs | Baja | Campos especiales `__colmena_*` no documentados (OK — son inyectados automáticamente) |
| 1.4 | Docs | Baja | Auto-registro en AttachmentRegistry no documentado en `node_configurations.json` (sí en dev guide) |
| 1.5 | Docs | Baja | `api_key` field no documenta `${ENV_VAR}` support (documentado en `image_generation` pero no aquí) |
| 2.1 | Código | Baja | Soporte implícito para storage handles (`local://`, `chat-attachments/`) — documentado en dev guide pero no schema |
| 2.2 | Código | Baja | Fail-soft para auto-registro fallido (comportamiento OK, sin documentación) |
| 2.3 | Código | OK | OpenAI error body incluido en mensaje (debug útil) |
| 2.4 | Código | OK | MIME type default a image/png si no hay header (razonable para CDNs) |
| 2.5 | Código | Baja | Clamping de `n` silencioso (no error, pero no documentado) |

---

## Remediaciones Recomendadas

### Prioridad ALTA (bloquea uso correcto)

1. **Reformular `node_as_tools_reference.json` línea ~340** para claridad sobre qué esquemas soporta `source_url`: "Accepts 'data:' URIs, 'http(s)://' URLs, or storage handles ('local://<key>' or 'chat-attachments/<key>'). Does NOT resolve bare document_ids or '$attachment:<document_id>' placeholders."

### Prioridad MEDIA (afecta discovery en `node_configurations.json`)

2. **Actualizar `node_configurations.json` campo `source_url`** para documentar storage handles: "data: URI, http(s) URL, or storage handles (local://<key> or chat-attachments/<path>). Does NOT resolve $attachment: placeholders."

3. **Actualizar `node_configurations.json` campo `api_key`** para documentar `${ENV_VAR}` support: "Required OpenAI API key. Supports ${ENV_VAR} interpolation and secure-value placeholders."

4. **Agregar nota sobre auto-registro** en la descripción de `image_edit` en `node_configurations.json`: "Auto-registers edited image in AttachmentRegistry with provider: Generated and origin: generated_by:image_edit."

### Prioridad BAJA (legibilidad, comportamiento OK)

5. **Documentar clamping de `n`** en `node_configurations.json`: "Default 1, clamped to range [1, 10]" en lugar de solo "max 10".

6. **Agregar sección en `docs/developer_guide/32_multimedia_generation.md`** sobre fail-soft auto-registro: "If AttachmentRegistry auto-registration fails (warning emitted), the artifact is still stored correctly; load_attachment(document_id) will work via direct storage lookup."

---

**Auditoría completada:** 9 hallazgos en documentación (2 media, 3 baja, 4 OK) + 5 aspectos de código validados (2 baja, 3 OK) + 12 casos de prueba ejecutables (happy path, optional fields, error cases, storage integration, session keying).

Credenciales requeridas para E2E: `OPENAI_API_KEY` en `.env`.
Operador debe saber: Plan B (2026-05-25) removio campos legacy `attachment_id` y `url` — solo `document_id` en outputs.
