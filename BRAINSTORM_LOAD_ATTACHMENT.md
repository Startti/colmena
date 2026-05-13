# Brainstorm — Load Attachment Feature

**Estado:** En progreso — esperando respuesta del usuario a la última pregunta  
**Skill activo:** `superpowers:brainstorming`  
**Siguiente paso:** Respuesta del usuario → diseño completo → spec → writing-plans

---

## Contexto del problema

El nodo `llm_call` soporta archivos adjuntos (`files[]` en `config` o `inputs`). Cuando se adjunta un documento, el motor lo sube a la Files API del provider (Anthropic/OpenAI/Gemini) y lo envía en el primer turno. **Problema:** el campo `LlmMessage.files` nunca se persiste en `llm_node_history` — solo se guardan `role, content, tool_call_id, tool_calls`. Entonces en turnos posteriores el historial reconstruido no tiene los archivos y el LLM pierde contexto del documento.

El usuario NO quiere re-enviar el documento en cada turno (costo de tokens). Quiere una herramienta que el LLM pueda invocar on-demand para traer el documento cuando lo necesite.

---

## Decisiones tomadas

### ✅ Decisión 1 — Estrategia de storage: Opción α (upload-on-first-touch)

Cuando llega un archivo (inline binary, path local, o signed URL de GCS), se sube a la Files API del provider en el primer turno — igual que ya se hace para signed URLs grandes. El registro de la conversación nunca guarda bytes: solo guarda `provider_file_id`s. Reutiliza el `provider_file_cache` existente y la infraestructura de upload (pipe streaming end-to-end, resumable para Gemini, etc.).

**Por qué α y no las otras:**
- β (Postgres BYTEA para binarios chicos): infla la DB, requiere gestión de lifecycle de blobs.
- γ (object store propio GCS/FS): infra nueva, más invasivo.
- α: cero infra nueva de storage. La única tabla nueva es un registro de "qué files están disponibles en esta conversación" (metadata solamente, sin bytes).

### ✅ Decisión 2 — Discoverability: catálogo dentro de la descripción del tool

Igual que `load_skill` ([load_skill_tool.rs](src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs)):
- La descripción del tool lista todos los attachments disponibles para la conversación.
- Ejemplo: `"Available attachments: doc-abc (Q3 Financial.pdf, 47MB, application/pdf), doc-xyz (photo.jpg, 2MB, image/jpeg)"`.
- Si no hay attachments registrados → el tool no aparece en el tool list. Cero overhead.
- **Label/summary**: caller-supplied (`label: "Q3 Financial Report"`, `description: "..."` opcional). Fallback a `filename + mime_type + size`. Sin auto-generación por LLM (YAGNI).

### ✅ Decisión 3 — Mecanismo del tool: sentinel pattern (como SUSPENDED)

`load_skill` devuelve texto directamente en el tool result. Para archivos esto no funciona (no se pueden enviar binarios como texto). La solución usa el patrón sentinel que ya existe para `SUSPENDED` en [agent_service.rs:280-298](src/libs/colmena/src/llm/application/agent_service.rs#L280):

```
LLM llama load_attachment(id: "doc-abc-123")
   ↓
Tool executor devuelve:
  { "__colmena_status": "LOAD_ATTACHMENT", "document_id": "doc-abc-123" }
   ↓
AgentService detecta el sentinel
   ↓
Busca provider_file_id en conversation_attachments registry
   ↓
Construye LlmMessage::user_with_files(
  "[Archivo adjunto solicitado]",
  [FileData { source: FileSource::Uploaded(ref) }]
)
   ↓
Persiste ese mensaje en llm_node_history
push a messages[]
   ↓
Siguiente iteración del ReAct loop — el LLM ve el archivo en contexto
```

La persistencia en historia es clave: en el próximo run, el historial ya tiene ese mensaje user con el `file_id`. Si el `file_id` expiró (Gemini 48h), hay que detectarlo y re-subir usando el `provider_file_cache` existente con recuperación de `SignedUrl` original.

---

## Pregunta pendiente del usuario (sin responder aún)

> ¿El registro de attachments vive a nivel `agent_session_id` (toda la conversación compartida), o a nivel del `llm_call` node dentro de esa sesión?
>
> Si hay dos nodos `llm_call` distintos en el mismo DAG, ¿comparten los attachments o cada uno tiene los suyos?

**Opciones:**
- **A) Por `agent_session_id`** — todos los nodos llm_call de la sesión comparten el mismo pool de attachments. Un nodo adjunta, otro puede cargar. Más flexible.
- **B) Por `(agent_session_id, node_id)`** — cada nodo tiene su propio registro, aislado. Más predecible.
- **C) Configurable en el nodo** — el grafo declara si quiere scope compartido o aislado.

---

## Arquitectura emergente (borrador, no aprobada aún)

### Nueva tabla `conversation_attachments`

```sql
CREATE TABLE conversation_attachments (
    agent_session_id  TEXT NOT NULL,
    -- node_id opcional si el scope es por nodo
    document_id       TEXT NOT NULL,
    provider          TEXT NOT NULL,  -- "openai" | "anthropic" | "gemini"
    provider_file_id  TEXT NOT NULL,
    mime_type         TEXT NOT NULL,
    filename          TEXT NOT NULL,
    label             TEXT,           -- caller-supplied, fallback a filename
    description       TEXT,           -- caller-supplied resumen corto (opcional)
    size_bytes        BIGINT,
    registered_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_session_id, document_id, provider)
);
```

### Nuevo synthetic tool `load_attachment`

Archivo a crear: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_attachment_tool.rs`

Patrón idéntico a `load_skill_tool.rs`:
- `build_load_attachment_tool_definition(registry)` → `ToolDefinition` con catálogo en descripción
- `dispatch_load_attachment(tool_call, registry, provider)` → devuelve sentinel JSON
- `into_tool_result(call_id, result)` → `ToolResult`

### Cambios en `AgentService::run`

En el bloque de tool execution ([agent_service.rs:258-319](src/libs/colmena/src/llm/application/agent_service.rs#L258)):

```rust
// Después del bloque SUSPENDED detection:
if parsed.get("__colmena_status").and_then(|v| v.as_str()) == Some("LOAD_ATTACHMENT") {
    let document_id = parsed.get("document_id")...;
    let file_data = attachment_registry.get(session_id, document_id, provider).await?;
    let synthetic_msg = LlmMessage::user_with_files(
        "[Archivo adjunto por solicitud del modelo]",
        vec![file_data],
    )?;
    messages.push(synthetic_msg.clone());
    self.conversation_repository.add_message(session_id, synthetic_msg).await?;
    // No persist tool result — continuar loop con el file inyectado
    continue;
}
```

### Flujo completo de primer attach (modificación al nodo `llm_call`)

```
parse_file_entries → FileSource::InlineBytes | SignedUrl | Uploaded
   ↓
resolve_files (ya existente) → todos los FileSource::Uploaded(ref)
   ↓
[NUEVO] conversation_attachments.register(
    agent_session_id, document_id, provider, provider_file_id,
    mime, filename, label, description, size_bytes
)
   ↓
LLM call normal con los archivos adjuntos
```

### Registro desde el JSON del nodo

Para que el llamador pueda pasar label/description, el contrato del nodo se extiende:

```json
{
  "files": [
    {
      "id": "doc-abc-123",
      "mime_type": "application/pdf",
      "filename": "Q3_Financial.pdf",
      "label": "Reporte Financiero Q3",
      "description": "Desglose de ingresos y gastos Q3 2026",
      "url": "https://storage.googleapis.com/...?X-Goog-Signature=..."
    }
  ]
}
```

---

## Archivos relevantes del codebase

| Archivo | Rol |
|---------|-----|
| [llm_message.rs](src/libs/colmena/src/llm/domain/llm_message.rs) | `FileData`, `FileSource`, `LlmMessage::user_with_files` |
| [agent_service.rs](src/libs/colmena/src/llm/application/agent_service.rs) | ReAct loop, detección sentineles, donde va el LOAD_ATTACHMENT handler |
| [sqlite_conversation_repository.rs](src/libs/colmena/src/llm/infrastructure/persistence/sqlite_conversation_repository.rs) | Confirmado: `files` NO se persiste — gap real |
| [postgres_file_cache.rs](src/libs/colmena/src/llm/infrastructure/files/postgres_file_cache.rs) | Cache de provider_file_ids — reutilizable |
| [load_skill_tool.rs](src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/load_skill_tool.rs) | Patrón exacto a seguir para el nuevo tool |
| [document_tools.rs](src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/document_tools.rs) | Synthetic tools existentes (artifacts de output, diferente a este feature) |
| [28_large_files_api.md](docs/developer_guide/28_large_files_api.md) | Documentación completa del pipeline de archivos |
| [15_memory_guide.md](docs/developer_guide/15_memory_guide.md) | Documentación de memoria conversacional |

---

## Cómo retomar esta sesión

1. Abrir Claude Code en el repo `colmena`
2. Invocar skill: `/superpowers:brainstorming`
3. Decir: _"Retomá desde BRAINSTORM_LOAD_ATTACHMENT.md — el usuario no respondió aún la pregunta de scope (agent_session_id global vs por node_id). Una vez que responda, avanzar al diseño completo, luego escribir el spec en `docs/superpowers/specs/` y commitear."_
4. Leer este archivo completo como contexto
5. Continuar desde **Pregunta pendiente del usuario**
