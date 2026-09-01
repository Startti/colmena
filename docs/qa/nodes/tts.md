# QA — Nodo `tts`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/tts.rs`

Fuentes de doc revisadas:
- `docs/node_configurations.json` (líneas 3112–3184)
- `docs/node_as_tools_reference.json` (líneas 1038–1048)
- `docs/agent_context/node_ports_reference.md` (tabla principal + row `tts`)
- `docs/developer_guide/32_multimedia_generation.md` (§tts, líneas 288–302)

---

## 1) Config documentada NO soportada por el código

**Sin discrepancias detectadas.**

Todas las opciones descritas en las 4 fuentes son implementadas. El código valida:
- `provider` obligatorio (línea 150), acepta "openai" | "elevenlabs" | "google"
- `model` obligatorio (línea 156)
- `api_key` obligatorio (línea 161), resuelve `${ENV_VAR}` (línea 162)
- `text` obligatorio, inputs-over-config (línea 170)
- `voice` obligatorio, inputs-over-config (línea 176)
- `format` opcional, default mp3 (línea 182–186), valida via `AudioFormat::from_str`
- `speed` opcional float (línea 187–191)

---

## 2) Código NO documentado

### 2.1 — Comportamiento inputs-over-config para campos infraestructurales

**Líneas 141–192 (tts.rs)**

El código lee `provider`, `model`, `api_key` tanto de `inputs` como de `config`, **priorizando inputs** (implementa patrón inputs-over-config). Las docs describen estos campos como "en config" sin mencionar que pueden llegar vía inputs como parte de la ejecución de tool.

**Impacto QA:** cuando el nodo se invoca como LLM tool, el executor pasa `config={}` y cuelga los campos `fixed` del `node_schema` en `inputs`. Los tests unitarios línea 427–450 verifican esto pero no está documentado en el guide de usuario.

**Referencia doc:** `node_as_tools_reference.json` línea 1041–1045 muestra ejemplo con `fixed:` para provider/model/api_key/voice/format, pero no explica que tts.rs lee desde inputs.

### 2.2 — Auto-registro en AttachmentRegistry es fail-soft

**Líneas 250–283 (tts.rs)**

Cuando `agent_session_id` está presente inyectado por el engine, el nodo intenta auto-registrar el audio en `AttachmentRegistry`. Si falla (línea 273), solo log de warn — no bloquea la ejecución. Test línea 636–665 confirma: "no_registry_means_no_registration_but_still_emits_document_id".

**Docs:** `developer_guide/32` línea 306–326 menciona auto-registro pero NO dice que es fail-soft. `node_configurations.json` no menciona registry en absoluto.

**Impacto QA:** un fallo en la registry (DB caído, permisos, etc.) no impide que el audio se devuelva al LLM, pero los subsistemas que lean via `load_attachment` no lo encontrarán.

### 2.3 — Origin metadata: "generated_by:tts"

**Línea 271 (tts.rs)**

La fila en `AttachmentRegistry` usa `origin: Some(origin::generated_by("tts"))`.

**Docs:** `developer_guide/32` línea 306–326 menciona `provider: ProviderKind::Generated` pero no cita el string de origin específico. No está en `node_configurations.json` ni `node_as_tools_reference.json`.

**Impacto QA:** un test que espere un origin específico debe saber que es `generated_by:tts`, no otro.

### 2.4 — Text truncation a 80 caracteres en descripción

**Línea 233 (tts.rs): `text.chars().take(80).collect()`**

La descripción del audio (que aparece en el output y en la registry) trunca el text a 80 chars.

**Docs:** `node_configurations.json` línea 3172 dice "string — 'TTS synthesized with <model>: <text prefix>'" pero no especifica "80 caracteres".

**Impacto QA:** si `text` es muy largo, QA debe verificar que output.audio.description contiene solo los primeros 80 chars.

### 2.5 — Filename estándar: "speech.<ext>"

**Línea 219 (tts.rs): `format!("speech.{}", format.file_extension())`**

El archivo persisted usa nombre fijo "speech.mp3" / "speech.wav" / etc., no el documento_id ni el text.

**Docs:** no documentado en node_configurations.json, developer_guide/32, ni node_as_tools_reference.json.

**Impacto QA:** tests que creen que pueden inspeccionar `/tmp/colmena-out/` deben saber que verán archivos "speech.*", no nombres derivados del content.

### 2.6 — Session IDs forwarded a storage y registry

**Líneas 121–128, 227–228, 250–251 (tts.rs)**

El nodo extrae `__colmena_session_id` y `__colmena_agent_session_id` inyectados por el engine y los pasa a:
- `StoreRequest` (línea 227–228)
- `UpsertAttachmentInput` (línea 255–256)

**Docs:** no documentado. `developer_guide/32` no menciona estos campos.

**Impacto QA:** si tomas valores de `conversation_attachments`, deben matchear la sesión correcta. Test línea 531–555 verifica esto.

### 2.7 — Errores específicos y su texto exacto

**Línea 150, 156, 161, 170, 176, 184**

Los mensajes de error fail-closed son:
- "tts: provider is required (openai|elevenlabs|google)"
- "tts: model is required"
- "tts: api_key is required"
- "tts: text is required (via inputs or config)"
- "tts: voice is required (via inputs or config)"
- "tts: <parsed error from AudioFormat::from_str>"

El error de env var no resuelto es (línea 104): "env var {var} not set (referenced by tts)"

El error de unknown provider viene de `build_tts_repository()` factory (línea 198–205), no documentado.

**Docs:** no enumerados en node_configurations.json ni developer_guide.

**Impacto QA:** si un test espera un error específico, debe matchear exactamente el texto o al menos el substring distintivo ("provider", "tts", etc.).

---

## 3) Plan de pruebas QA

### Caso 1: Happy path — OpenAI tts-1, voz alloy, formato mp3

**Objetivo:** Verificar ejecución E2E básica con el stack por defecto documentado.

**Grafo mínimo:**
```json
{
  "nodes": [
    {
      "id": "input_text",
      "node_type": "input",
      "config": { "data": { "text": "Hello world" } }
    },
    {
      "id": "speak",
      "node_type": "tts",
      "config": {
        "provider": "openai",
        "model": "tts-1",
        "api_key": "${OPENAI_API_KEY}",
        "text": "Hola mundo, esta es una prueba",
        "voice": "alloy"
      }
    },
    {
      "id": "output",
      "node_type": "output",
      "config": {}
    }
  ],
  "edges": [
    { "from": "speak", "to": "output" }
  ]
}
```

**Ejecución:**
```bash
source .env
cargo run --bin dag_engine -- run <graph.json>
```

**Entrada:** ninguna (config autosuficiente).

**Resultado esperado:**
```json
{
  "output": {
    "audio": {
      "document_id": "audio_speech_mp3_*",
      "mime_type": "audio/mpeg",
      "size_bytes": <positivo>,
      "duration_ms": <null o número>,
      "description": "TTS synthesized with tts-1: Hola mundo, esta es una p"
    },
    "provider": "openai",
    "model": "tts-1"
  }
}
```

**Verificación pass/fail:**
- `document_id` comienza con "audio_"
- `mime_type` es "audio/mpeg"
- `size_bytes` > 0
- `description` contiene "tts-1" + primeros 80 chars del text
- **NO** hay campos legacy `attachment_id` ni `url`
- Exit code 0

---

### Caso 2: ElevenLabs — model eleven_multilingual_v2

**Objetivo:** Validar soporte de provider distinto, requiere `ELEVENLABS_API_KEY`.

**Cambios vs Caso 1:**
```json
{
  "provider": "elevenlabs",
  "model": "eleven_multilingual_v2",
  "api_key": "${ELEVENLABS_API_KEY}",
  "text": "Bonjour le monde",
  "voice": "21m00Tcm4TlvDq8ikWAM",
  "format": "wav"
}
```

**Resultado esperado:**
- `mime_type`: "audio/wav"
- `description`: "TTS synthesized with eleven_multilingual_v2: Bonjour le m"

**Verificación pass/fail:**
- filename en storage es "speech.wav" (ver caso 5)
- Audio es escuchable (binary no-NaN)

**Nota especial:** si `ELEVENLABS_API_KEY` no está en `.env`, el test se salta (`#[ignore]`).

---

### Caso 3: Google Gemini TTS — modelo gemini-2.5-flash-preview-tts

**Objetivo:** Validar Google provider + verificar que formato se ignora y devuelve L16 PCM real.

**Config:**
```json
{
  "provider": "google",
  "model": "gemini-2.5-flash-preview-tts",
  "api_key": "${GEMINI_API_KEY}",
  "text": "Hola, esto es Google TTS",
  "voice": "Kore",
  "format": "wav"
}
```

**Resultado esperado:**
- `mime_type`: "audio/L16;rate=24000" (o similar, según implementación de google_tts_adapter — el nodo ignora `format` para Google)
- `format: wav` en config se ignora silenciosamente
- `speed` si está presente, se mapea a `speakingRate`

**Verificación pass/fail:**
- `mime_type` comienza con "audio/" (no es "audio/wav" como se solicitó)
- Log no contiene error sobre "wav inválido"

---

### Caso 4: Format variaciones — mp3, wav, opus, pcm

**Objetivo:** Validar cada formato válido + rechazar inválidos.

**Configuración base (OpenAI):**
```json
{
  "provider": "openai",
  "model": "tts-1",
  "api_key": "${OPENAI_API_KEY}",
  "text": "Test format",
  "voice": "alloy"
}
```

**Subcasos:**

**4a: format: "mp3" (default si omitido)**
- Omitir `format` en config
- Esperar `mime_type: "audio/mpeg"`

**4b: format: "wav"**
- `format: "wav"`
- Esperar `mime_type: "audio/wav"`

**4c: format: "opus"**
- `format: "opus"`
- Esperar `mime_type: "audio/opus"`

**4d: format: "pcm"**
- `format: "pcm"`
- Esperar `mime_type` = "audio/pcm" o similar

**4e: format: "flac" (inválido)**
- `format: "flac"`
- Esperar error que contiene "unknown audio format"
- Exit code != 0

**Verificación pass/fail:**
- Cada format válido devuelve mime_type esperado
- El error de format inválido es el de línea 184 de tts.rs

---

### Caso 5: Speed variación — OpenAI 0.25 a 4.0

**Objetivo:** Validar rango de speed en OpenAI; ElevenLabs lo ignora.

**5a: speed: 0.5 (OpenAI)**
```json
{
  "provider": "openai",
  "model": "tts-1",
  "api_key": "${OPENAI_API_KEY}",
  "text": "Hablo lentamente",
  "voice": "alloy",
  "speed": 0.5
}
```
- Esperar audio más lento (verificación manual)

**5b: speed: 2.0**
- Esperar audio más rápido

**5c: speed: 0.1 (por debajo de rango)**
- OpenAI rechaza < 0.25 (validación en API, no en colmena)
- Esperar error de OpenAI API

**5d: ElevenLabs + speed: 1.5**
- Speed se ignora silenciosamente (logged as warn en adapter)
- No error

**Verificación pass/fail:**
- OpenAI respeta speed
- ElevenLabs no falla pero ignora speed

---

### Caso 6: Text vía inputs vs config

**Objetivo:** Validar inputs-over-config para `text`.

**6a: text en config solo**
```json
{
  "config": {
    "provider": "openai",
    "model": "tts-1",
    "api_key": "${OPENAI_API_KEY}",
    "text": "Desde config",
    "voice": "alloy"
  }
}
```
- Esperar descripción con "Desde config"

**6b: text en inputs, config vacío**
```json
{
  "config": {
    "provider": "openai",
    "model": "tts-1",
    "api_key": "${OPENAI_API_KEY}",
    "voice": "alloy"
  },
  "inputs": {
    "text": "Desde inputs"
  }
}
```
- Esperar descripción con "Desde inputs"

**6c: text en ambos (inputs gana)**
```json
{
  "config": { "text": "Config text", ... },
  "inputs": { "text": "Inputs text" }
}
```
- Esperar descripción con "Inputs text"

**Verificación pass/fail:**
- Línea 165–170 de tts.rs: inputs gana sobre config

---

### Caso 7: Voice vía inputs vs config

**Objetivo:** Validar inputs-over-config para `voice` (OpenAI).

**7a: voice en config**
```json
{ "voice": "nova" }
```
- Esperar audio con voz nova (verificación manual)

**7b: voice en inputs, config vacío**
```json
{ /* no voice en config */ },
"inputs": { "voice": "echo" }
```
- Esperar audio con voz echo

**7c: voice en ambos (inputs gana)**
- inputs.voice debe dominar config.voice

**Verificación pass/fail:**
- Línea 171–176 de tts.rs: inputs gana

---

### Caso 8: Missing required fields — fail-closed

**Objetivo:** Cada campo required debe rechazarse.

**8a: Missing provider**
```json
{
  "provider": null,
  "model": "tts-1",
  "api_key": "${OPENAI_API_KEY}",
  "text": "Test",
  "voice": "alloy"
}
```
- Esperar error que contiene "provider is required"
- Exit code != 0

**8b: Missing model**
- Omitir `model`
- Esperar "model is required"

**8c: Missing api_key**
- Omitir `api_key`
- Esperar "api_key is required"

**8d: Missing text**
- Omitir `text` en config e inputs
- Esperar "text is required (via inputs or config)"

**8e: Missing voice**
- Omitir `voice`
- Esperar "voice is required (via inputs or config)"

**Verificación pass/fail:**
- Mensajes de error exactos per líneas 150, 156, 161, 170, 176

---

### Caso 9: API key env var resolution

**Objetivo:** Validar `${OPENAI_API_KEY}` → valor real desde env.

**9a: Env var presente**
```bash
export OPENAI_API_KEY="sk-proj-test..."
```
```json
{ "api_key": "${OPENAI_API_KEY}", ... }
```
- Esperar que el nodo resuelva y use la key real

**9b: Env var ausente**
```bash
unset OPENAI_API_KEY
```
```json
{ "api_key": "${OPENAI_API_KEY}", ... }
```
- Esperar error "env var OPENAI_API_KEY not set (referenced by tts)" (línea 104)

**9c: No-env literal key**
```json
{ "api_key": "sk-hardcoded-test", ... }
```
- Esperar que se use literalmente (no lo intenta resolver como env var)

**Verificación pass/fail:**
- Línea 101–108 de tts.rs: logic correcto

---

### Caso 10: Invalid provider (unknown)

**Objetivo:** Rechazar provider no registrado en factory.

**Config:**
```json
{
  "provider": "nuance",
  "model": "nuance-model",
  "api_key": "${NUANCE_API_KEY}",
  "text": "Test",
  "voice": "default"
}
```

**Resultado esperado:**
- Error línea 198–205: "unknown tts provider"
- Exit code != 0

**Verificación pass/fail:**
- Error message contiene "unknown tts provider"
- Test línea 517–528 verifica esto

---

### Caso 11: Session IDs forwarded a storage

**Objetivo:** Validar que `__colmena_session_id` y `__colmena_agent_session_id` inyectados por engine llegan a `StoreRequest`.

**Setup (requiere investigación manual de storage mock):**
- Inyectar inputs con `__colmena_session_id: "ses_abc"` e `__colmena_agent_session_id: "agent_xyz"`
- Mock o spy en `OutputStorageRepository::store()`
- Verificar que `StoreRequest` contiene session_id y agent_session_id exactos

**Test unitario existente:** línea 531–555 de tts.rs

**Verificación pass/fail:**
- Storage recibe session_id/agent_session_id correctos (si requiere DB real, test es `#[ignore]`)

---

### Caso 12: Auto-registration en AttachmentRegistry (fail-soft)

**Objetivo:** Verificar que audio se registra automáticamente cuando `agent_session_id` está presente, y que fallos no bloquean.

**Setup (requiere SqliteAttachmentRegistry):**
- Inyectar `__colmena_agent_session_id: "agent_test_001"`
- Mock o construir `AttachmentRegistry` real
- Verificar que tras execute(), una fila aparece en registry

**Test unitario existente:** línea 561–634 de tts.rs

**Llamada esperada:**
```rust
registry.lookup_by_document_id("agent_test_001", <document_id>)
```

**Resultado esperado:**
```rust
entry.storage_key == "sk-tts-1"
entry.origin == Some("generated_by:tts")
entry.provider == ProviderKind::Generated
```

**Fail-soft:** línea 636–665 verifica que incluso sin registry, output sigue siendo válido.

**Verificación pass/fail:**
- Entry encontrada en registry después de execute
- `origin` es exactamente "generated_by:tts"

---

### Caso 13: Plan B — fields legacy removidos

**Objetivo:** Validar que tool result NOT contiene campos legacy `attachment_id` ni `url`.

**Config:**
```json
{
  "provider": "openai",
  "model": "tts-1",
  "api_key": "${OPENAI_API_KEY}",
  "text": "Test legacy removal",
  "voice": "alloy"
}
```

**Resultado esperado:**
```json
{
  "output": {
    "audio": {
      "document_id": "...",
      "mime_type": "...",
      "size_bytes": ...,
      "duration_ms": ...
      /* NO: "attachment_id" */
      /* NO: "url" */
    },
    "provider": "openai",
    "model": "tts-1"
  }
}
```

**Verificación pass/fail:**
- `output.audio` contiene exactamente: document_id, mime_type, size_bytes, duration_ms, description
- No hay keys extra "attachment_id" o "url"
- Test línea 401–411 verifica (asserts `is_none()`)

---

### Casos especiales para credenciales + COLMENA_LOCAL

Los casos 1–3 requieren API keys reales (OPENAI_API_KEY, ELEVENLABS_API_KEY, GEMINI_API_KEY). En CI/pruebas sin keys:
- Marcar tests con `#[ignore = "requires OPENAI_API_KEY — run with cargo test -- --ignored"]`
- O mock `build_tts_repository()` con test double (ya lo hace el test `with_test_repository()`)

Para inspeccionar archivos generados:
```bash
export COLMENA_LOCAL=true
cargo run --bin dag_engine -- run <graph.json>
ls /tmp/colmena-out/speech.*
```

---

### Resumen de casos

| Caso | Objetivo | Requisitos | Pass criterio |
|------|----------|------------|---------------|
| 1 | OpenAI happy path | OPENAI_API_KEY | document_id + mime audio/mpeg |
| 2 | ElevenLabs provider | ELEVENLABS_API_KEY | document_id + mime audio/wav |
| 3 | Google TTS + ignore format | GEMINI_API_KEY | mime audio/L16 no importa format input |
| 4a–4e | Formatos válidos + inválido | None | mime_type correcto; error FLAC |
| 5a–5d | Speed: 0.25–4.0 (OpenAI), ignore (ElevenLabs) | OPENAI_API_KEY, ELEVENLABS_API_KEY | Speed se usa / ignora según spec |
| 6 | Text: inputs-over-config | None | Descripción refleja inputs.text si presente |
| 7 | Voice: inputs-over-config | None | Voice resuelto desde inputs si presente |
| 8a–8e | Missing required fields | None | Error exacto per campo |
| 9 | Env var resolution | Varies | ${VAR} → env value; missing → error |
| 10 | Unknown provider | None | "unknown tts provider" error |
| 11 | Session IDs → storage | Mock storage | StoreRequest.session_id/agent_session_id |
| 12 | Auto-register attachment | AttachmentRegistry | registry.lookup_by_document_id OK |
| 13 | Plan B: no legacy fields | None | output.audio NOT has attachment_id/url |
