# Load Attachment — Documentos on-demand dentro del loop LLM

> **Estado:** Disponible desde 0.4.0
> **Spec:** [docs/superpowers/specs/2026-05-13-load-attachment-design.md](../superpowers/specs/2026-05-13-load-attachment-design.md)

## Por qué existe

`LlmMessage.files` no se persiste en `llm_node_history`. Cuando una conversación con un documento adjunto retoma en un turno posterior, el archivo deja de estar en el contexto del modelo. Re-adjuntarlo en cada turno es caro en tokens.

`load_attachment` resuelve esto: el LLM ve un catálogo de documentos disponibles en la descripción del tool, y pide el que necesita cuando lo necesita.

## Cómo funciona

1. Adjuntás un archivo al primer `llm_call` mediante `files[]` como siempre.
2. El motor lo sube al provider y registra metadata (no bytes) en `conversation_attachments`, scoped por `agent_session_id`.
3. En cualquier turno siguiente — incluyendo `llm_call`s dentro de subgrafos — el LLM ve la tool sintética `load_attachment` con el catálogo de la sesión en su descripción.
4. Cuando el LLM llama `load_attachment(document_id)`, el motor inyecta un mensaje `user` sintético con el archivo y persiste ese mensaje en la historia. Próximo turno: el archivo ya está en contexto.

## Flag por nodo

`attachments_enabled` (default `true`) en `llm_call.config`:

- `true` — el nodo expone la tool y aporta sus `files[]` al catálogo.
- `false` — el nodo NO expone la tool (no la ve) pero igualmente registra cualquier `files[]` que reciba, para que otros nodos los lean.

Usá `false` cuando un agente especialista NO debería tener acceso a documentos cargados por otras partes de la sesión.

## Prompt auto-inyectado

Cuando hay al menos un attachment en el catálogo de la sesión Y `attachments_enabled` es `true`, el motor inyecta automáticamente `ATTACHMENTS_SYSTEM_PRELUDE` al final del `system_message`. El prelude le explica al modelo:

- Que hay documentos disponibles (listados en la descripción del tool).
- Que debe llamar `load_attachment` antes de responder cuando el usuario referencia un documento.
- Que no debe listar/parafrasear los attachments salvo que se le pida.
- Que la tool acepta un solo `document_id` por llamada.
- Que NO debe llamar `load_attachment` preemptivamente cuando la pregunta no depende de un attachment.

**El graph author NO necesita repetir estas instrucciones en su propio `system_message`.** El `system_message` del nodo se reserva para la persona/rol del agente.

## Campos opcionales en `files[]`

```jsonc
{
  "files": [
    {
      "id": "q3-report",                       // opcional, auto-id (att_<hex16>) si falta
      "mime_type": "application/pdf",
      "filename": "Q3_Financial.pdf",
      "label": "Reporte Financiero Q3",        // opcional, fallback = filename
      "description": "Ingresos y gastos Q3",   // opcional
      "url": "https://storage.googleapis.com/...?X-Goog-Signature=..."
    }
  ]
}
```

## Subgrafos

`agent_session_id` se propaga automáticamente al subgrafo. Eso significa que un `llm_call` dentro de un subgrafo ve el mismo catálogo que el padre, sin código adicional. Si querés aislamiento estricto, usá `attachments_enabled: false` en el `llm_call` del subgrafo.

## Recuperación por expiración

Gemini caduca los `file_id` a las 48h. Cuando una fila en `conversation_attachments` tiene `source_kind = 'signed_url' | 'path'` y `refreshed_at` ≥ 24h, el resolver re-sube el archivo silenciosamente al provider y actualiza el row. Si el archivo se subió como `InlineBytes` (bytes embebidos en el JSON), NO retenemos los bytes — el load fallará con `attachment_expired_unrecoverable` y el LLM puede pedir al usuario que re-suba.

## Errores que el LLM puede ver como resultado de la tool

```json
{ "error": "unknown_document_id", "document_id": "...", "hint": "..." }
{ "error": "attachment_expired_unrecoverable", "document_id": "...", "reason": "..." }
```

Ambos se devuelven como `ToolResult` ordinario para que el modelo pueda recuperarse (pedir al usuario, intentar otro id, etc.).

## Tabla `conversation_attachments`

```sql
agent_session_id, document_id, provider, provider_file_id,
mime_type, filename, size_bytes,
label, description,
source_kind, source_value,
registered_at, refreshed_at
PRIMARY KEY (agent_session_id, document_id, provider)
```

`source_kind` controla la estrategia de recuperación:
- `signed_url` / `path` → recuperable (re-upload silencioso al pasar 24h)
- `inline` → no recuperable (bytes no retenidos)

## Test graphs

- `tests/graphs/agents/load_attachment_basic.json` — single `llm_call` con un archivo + pregunta posterior
- `tests/graphs/agents/load_attachment_subgraph.json` — parent registra + child subgrafo lee
- `tests/graphs/agents/load_attachment_opt_out.json` — verifica que `attachments_enabled: false` oculta la tool

Todos usan `google` / `gemini-2.5-flash` con `${DATABASE_URL}` para memoria Postgres. Los URLs `$REPLACE_WITH_SIGNED_URL` son placeholders — sustituí por una signed URL real (GCS) antes de correr.
