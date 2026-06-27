---
name: capability-multimedia
description: Use when the user wants to create or edit an image, or turn text into spoken audio/voice. Covers image_generation, image_edit and tts.
---

# Capacidades multimedia: imágenes y voz

Esta skill cubre tres nodos para generar y editar media:

- `image_generation` — crear una imagen desde un texto (prompt).
- `image_edit` — modificar una imagen ya existente.
- `tts` — convertir texto en audio hablado (text-to-speech).

Los tres devuelven un **handle** (`document_id`) más metadata, nunca los bytes
crudos de la imagen o el audio. El usuario final luego ve/escucha el resultado a
través del frontend, que resuelve ese `document_id`.

> ⚠️ **CAVEAT DE DESPLIEGUE — leer antes de prometerle multimedia al usuario.**
> Estos tres nodos **solo quedan registrados cuando hay un adapter de storage
> presente** (es decir, cuando el worker está configurado con almacenamiento de
> artifacts). En local eso es `COLMENA_LOCAL=true` (escribe a `/tmp/colmena-out/`
> + sirve por HTTP local); en dev/prod es el callback firmado a GCS
> (`COLMENA_STORAGE_CALLBACK_URL` + `COLMENA_STORAGE_CALLBACK_SECRET`). Si el
> entorno donde correrá el grafo no tiene storage configurado, estos nodos **no
> existen** y el grafo fallará. Cuando el usuario pida crear/editar imágenes o
> generar voz, **avisale de esta dependencia de entorno**: solo funciona donde el
> almacenamiento de artifacts esté habilitado.

---

## image_generation — crear una imagen desde texto

Campos de config (nombres exactos):

| Campo | Requerido | Descripción |
|---|---|---|
| `provider` | sí | `openai` o `google`. |
| `model` | sí | OpenAI: `gpt-image-1` o `dall-e-3`. Google: `imagen-4.0-generate-001`. |
| `api_key` | sí (openai) | Clave OpenAI. Soporta `${OPENAI_API_KEY}` y secure-values. Google no la necesita (auth por credenciales del worker). |
| `prompt` | sí | Texto que describe la imagen a generar. |
| `size` | opcional | Default `1024x1024`. |
| `quality` | opcional (openai) | `low` / `medium` / `high` / `auto`. |
| `n` | opcional | Cantidad de imágenes. Default 1, máx 10. |

**Salida**: `{ images: [ { document_id, mime_type, size_bytes, description } ], provider, model }`.
El campo clave es `images[].document_id`.

---

## image_edit — editar una imagen existente

Campos de config (nombres exactos):

| Campo | Requerido | Descripción |
|---|---|---|
| `provider` | sí | `openai` (único soportado hoy). |
| `model` | opcional | Default `gpt-image-1`. |
| `api_key` | sí | Clave OpenAI. Soporta `${OPENAI_API_KEY}` y secure-values. |
| `source_url` | sí | Imagen a editar. Acepta `data:<mime>;base64,...`, una URL `http(s)://`, o un handle de storage (`local://<key>`, `chat-attachments/<key>`). También admite la forma `$attachment:<document_id>` cuando se la pasa por un canal que resuelve attachments. |
| `mask_url` | opcional | PNG con transparencia que marca el área a editar. Mismos formatos que `source_url`. |
| `prompt` | sí | Describe la edición deseada. |
| `size` / `quality` / `n` | opcional | Igual que `image_generation`. |

**Salida**: mismo shape que `image_generation` (`{ images: [...], provider, model }`).

> ⚠️ El encadenamiento automático `image_generation` → `image_edit` manejado por
> un LLM está limitado: `source_url` NO resuelve un `document_id` pelado. Para
> editar, pasale a `source_url` una URL `http(s)://` o un `data:` URI
> independientemente accesible.

---

## tts — convertir texto en voz

Campos de config (nombres exactos):

| Campo | Requerido | Descripción |
|---|---|---|
| `provider` | sí | `openai`, `elevenlabs` o `google`. |
| `model` | sí | OpenAI: `tts-1` / `tts-1-hd` / `gpt-4o-mini-tts`. ElevenLabs: `eleven_multilingual_v2` / `eleven_turbo_v2_5`. Google: `gemini-2.5-flash-preview-tts`. |
| `api_key` | sí | Clave del provider. Soporta `${...}` y secure-values. |
| `text` | sí | Texto a sintetizar. |
| `voice` | sí | OpenAI: `alloy` / `echo` / `fable` / `onyx` / `nova` / `shimmer`. ElevenLabs: voice_id. Google: nombre prebuilt (ej. `Kore`). |
| `format` | opcional | `mp3` (default), `wav`, `opus`, `pcm`. Google ignora este campo. |
| `speed` | opcional | 0.25–4.0 (openai/google). |

**Salida**: `{ audio: { document_id, mime_type, size_bytes, duration_ms, description }, provider, model }`.
El campo clave es `audio.document_id`.

---

## Ejemplo ejecutable VERBATIM — texto → imagen → log

Pegá este grafo tal cual. Recibe un `prompt`, genera una imagen y la loggea.
Requiere `OPENAI_API_KEY` en el entorno y un adapter de storage activo (ver el
caveat de arriba).

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/img-gen-basic",
        "method": "POST",
        "test_payload": {
          "prompt": "Una imagen realista de un gato negro en el techo de una casa vieja de 1880 en Francia con cielo anaranjado"
        }
      }
    },
    "gen": {
      "type": "image_generation",
      "config": {
        "provider": "openai",
        "model": "gpt-image-1",
        "api_key": "${OPENAI_API_KEY}",
        "prompt": "A minimalist red circle centered on a white background, vector style, no text",
        "size": "1024x1024",
        "n": 1
      }
    },
    "log_step": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "trigger", "to": "gen" },
    { "from": "gen", "to": "log_step" }
  ]
}
```

### Variante voz — texto → tts → log

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/tts-basic",
        "method": "POST",
        "test_payload": {
          "text": "Hola, esto es una prueba de síntesis de voz desde Colmena."
        }
      }
    },
    "speak": {
      "type": "tts",
      "config": {
        "provider": "openai",
        "model": "tts-1",
        "api_key": "${OPENAI_API_KEY}",
        "text": "Hola, esto es una prueba de síntesis de voz desde Colmena.",
        "voice": "alloy",
        "format": "mp3"
      }
    },
    "log_step": { "type": "log" }
  },
  "edges": [
    { "from": "trigger", "to": "speak" },
    { "from": "speak", "to": "log_step" }
  ]
}
```

---

## Cómo cablear estos nodos

Para conectar el trigger, pasar el `prompt`/`text` entre nodos, y enlazar las
salidas con otros pasos del grafo, consultá [[building-graphs-core]].
