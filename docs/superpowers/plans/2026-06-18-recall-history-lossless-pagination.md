# recall_history lossless (paginación) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Hacer que `recall_history` recupere el contenido completo de cualquier mensaje pasado mediante paginación (`offset`/`limit`), eliminando el truncado silencioso de 10 KB que hoy pierde datos sin recuperación.

**Architecture:** Es la **Fase 1** (pieza clave) del spec `docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md`. Cambio aditivo y aislado en la tool sintética `recall_history`: se agregan args opcionales `offset`/`limit`, se reemplaza el cap de 10 KB por una página default acotada con cursor de continuación (`next_offset`). No toca esquema de DB ni el resto del pipeline. Habilita que las fases siguientes resuman con pérdida de forma segura (todo es recuperable).

**Tech Stack:** Rust, `serde`/`schemars` (args de tool), `serde_json` (resultado), `tokio` (tests async), patrón de tools sintéticas en `dag_engine/infrastructure/nodes/llm_synthetic_tools/`.

---

## File Structure

- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs` — args, constantes y `dispatch_recall_history` (lógica de paginación + tests unitarios inline).
- `src/libs/colmena/text/tools/helpers.yaml` — descripción LLM-facing de `recall_history` (explica `offset`/`limit`/`next_offset`).

No hay archivos nuevos. Todo el cambio vive en el archivo de la tool + su texto.

---

### Task 1: Paginación en `recall_history`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs`
- Test: mismo archivo (módulo `#[cfg(test)]` inline existente)

**Contexto del estado actual** (para el ejecutor): hoy `RecallHistoryArgs` solo tiene
`turn: usize`. `dispatch_recall_history` carga la conversación, valida el rango del turno,
toma `msg.content()`, y si supera `RECALL_OUTPUT_CHAR_CAP = 10*1024` lo **trunca** con un
marcador y agrega `_truncated: true`. Vamos a reemplazar ese truncado por paginación.

- [ ] **Step 1: Escribir el test que falla (paginación de contenido grande)**

Agregar al módulo `#[cfg(test)]` de `recall_history.rs`:

```rust
    #[tokio::test]
    async fn recall_paginates_large_content() {
        // 20_000 chars 'x' → con página default de 8_192 hacen falta 3 páginas.
        let huge = "x".repeat(20_000);
        let msgs = vec![LlmMessage::user(huge).unwrap()];
        let repo: Arc<dyn ConversationRepository> = Arc::new(StubRepo { msgs });
        let k = key();

        // Página 1: offset por defecto (0).
        let p1 = dispatch_recall_history(&repo, &k, serde_json::json!({"turn": 0})).await;
        assert_eq!(p1["total_chars"], 20_000);
        assert_eq!(p1["offset"], 0);
        assert_eq!(p1["returned_chars"], 8_192);
        assert_eq!(p1["next_offset"], 8_192);
        assert_eq!(p1["content"].as_str().unwrap().chars().count(), 8_192);
        assert!(p1.get("_truncated").is_none());

        // Página 2: continuar desde next_offset.
        let p2 =
            dispatch_recall_history(&repo, &k, serde_json::json!({"turn": 0, "offset": 8_192}))
                .await;
        assert_eq!(p2["offset"], 8_192);
        assert_eq!(p2["returned_chars"], 8_192);
        assert_eq!(p2["next_offset"], 16_384);

        // Página 3: resto, sin más páginas.
        let p3 =
            dispatch_recall_history(&repo, &k, serde_json::json!({"turn": 0, "offset": 16_384}))
                .await;
        assert_eq!(p3["offset"], 16_384);
        assert_eq!(p3["returned_chars"], 3_616);
        assert_eq!(p3["next_offset"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn recall_small_content_single_page() {
        let msgs = vec![LlmMessage::user("hi".to_string()).unwrap()];
        let repo: Arc<dyn ConversationRepository> = Arc::new(StubRepo { msgs });
        let r = dispatch_recall_history(&repo, &key(), serde_json::json!({"turn": 0})).await;
        assert_eq!(r["content"], "hi");
        assert_eq!(r["total_chars"], 2);
        assert_eq!(r["returned_chars"], 2);
        assert_eq!(r["next_offset"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn recall_clamps_limit_to_max() {
        let huge = "y".repeat(50_000);
        let msgs = vec![LlmMessage::user(huge).unwrap()];
        let repo: Arc<dyn ConversationRepository> = Arc::new(StubRepo { msgs });
        // Pedimos 1_000_000 pero se clampa al máximo por página (16_384).
        let r = dispatch_recall_history(
            &repo,
            &key(),
            serde_json::json!({"turn": 0, "limit": 1_000_000}),
        )
        .await;
        assert_eq!(r["returned_chars"], 16_384);
        assert_eq!(r["next_offset"], 16_384);
    }
```

También **eliminar** el test obsoleto `recall_truncates_oversized_content` (su
comportamiento — `_truncated` — ya no existe; lo reemplaza `recall_paginates_large_content`).

- [ ] **Step 2: Correr los tests y verificar que fallan**

Run: `cargo test --lib recall_history -- --nocapture`
Expected: FAIL — `recall_paginates_large_content` y `recall_clamps_limit_to_max` fallan
porque el dispatch todavía no expone `offset`/`limit`/`total_chars`/`returned_chars`/`next_offset`
(las claves dan `Null`), y `recall_small_content_single_page` falla por las claves nuevas.

- [ ] **Step 3: Implementar args + constantes de paginación**

En `recall_history.rs`, reemplazar las constantes y el struct de args:

```rust
/// Tamaño de página por defecto (chars) cuando el caller no pasa `limit`.
/// Acota cada recall para no inundar el contexto del agente; el contenido
/// completo se recupera paginando con `offset`/`next_offset`.
const RECALL_PAGE_DEFAULT_CHARS: usize = 8 * 1024;

/// Techo duro por página, aun si el caller pide un `limit` mayor.
const RECALL_PAGE_MAX_CHARS: usize = 16 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecallHistoryArgs {
    /// Persisted turn index — the same number that appears as `[T<n>]` in the
    /// Conversation summary earlier in your context. 0 is the first message.
    pub turn: usize,
    /// Character offset to start reading from (default 0). Use the `next_offset`
    /// returned by a previous call to page through a large message.
    #[serde(default)]
    pub offset: usize,
    /// Max characters to return in this page. Defaults to a bounded page size
    /// and is clamped to a hard per-call ceiling. Page again with `offset` to
    /// read more.
    #[serde(default)]
    pub limit: Option<usize>,
}
```

- [ ] **Step 4: Implementar la paginación en `dispatch_recall_history`**

Reemplazar el bloque que hoy hace el truncado (desde
`// Truncate content if it exceeds the cap.` hasta la construcción de `out`) por
slicing por **char** con cursor de continuación. El bloque nuevo, ubicado justo
después de `let msg = &conv.messages[parsed.turn];`:

```rust
    let raw_content = msg.content();
    let total_chars = raw_content.chars().count();

    let start = parsed.offset.min(total_chars);
    let page = parsed
        .limit
        .unwrap_or(RECALL_PAGE_DEFAULT_CHARS)
        .min(RECALL_PAGE_MAX_CHARS)
        .max(1);
    let end = start.saturating_add(page).min(total_chars);

    // Slice char-safe en el rango [start, end).
    let content_out: String = raw_content.chars().skip(start).take(end - start).collect();
    let next_offset: Option<usize> = if end < total_chars { Some(end) } else { None };

    let role_str = match msg.role() {
        MessageRole::User => "User",
        MessageRole::System => "System",
        MessageRole::Assistant => "Assistant",
        MessageRole::Tool => "Tool",
    };

    let mut out = serde_json::json!({
        "turn": parsed.turn,
        "role": role_str,
        "content": content_out,
        "offset": start,
        "returned_chars": end - start,
        "total_chars": total_chars,
        "next_offset": next_offset,
    });

    if let Some(tcid) = msg.tool_call_id() {
        out["tool_call_id"] = serde_json::json!(tcid);
    }
    if let Some(tcs) = msg.tool_calls() {
        let calls: Vec<serde_json::Value> = tcs
            .iter()
            .map(|tc| {
                serde_json::json!({
                    "id": tc.id,
                    "name": tc.function.name,
                    "arguments": tc.function.arguments,
                })
            })
            .collect();
        out["tool_calls"] = serde_json::Value::Array(calls);
    }

    out
```

Verificar que se eliminó toda referencia a `RECALL_OUTPUT_CHAR_CAP` y a `_truncated`
(ya no se usan; dejar una constante muerta haría fallar el build por
`warnings = "deny"`).

- [ ] **Step 5: Correr los tests y verificar que pasan**

Run: `cargo test --lib recall_history -- --nocapture`
Expected: PASS — los 3 tests nuevos + los existentes (`recall_returns_message_at_turn`,
`recall_out_of_range_returns_error`, `recall_invalid_args_returns_error`,
`recall_includes_tool_call_metadata`).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/recall_history.rs
git commit -m "feat(memory): paginate recall_history, remove silent 10KB truncation"
```

---

### Task 2: Actualizar el texto LLM-facing de `recall_history`

**Files:**
- Modify: `src/libs/colmena/text/tools/helpers.yaml` (entrada `recall_history`, líneas 12-15)

- [ ] **Step 1: Reescribir la descripción para documentar la paginación**

Reemplazar la entrada `recall_history` en `helpers.yaml` por:

```yaml
recall_history:
  summary: Re-read the original content of one past message by its turn index
  description: |
    Re-read the FULL original content of one past message by its turn index (the
    [T<n>] shown in the Conversation summary earlier in your context). The result
    is returned in bounded pages: each call gives `content` plus `offset`,
    `returned_chars`, `total_chars`, and `next_offset`. If `next_offset` is not
    null, call again with `offset` set to that value to read the next page, until
    `next_offset` is null. Use this to reconstruct large artifacts (e.g. a graph
    JSON) verbatim. Call sparingly — each page re-loads content into your context.
```

- [ ] **Step 2: Verificar que el registry de texto sigue cargando (no rompió el YAML)**

Run: `cargo test --lib text::`
Expected: PASS — el loader de `text/tools/*.yaml` parsea sin panic y no hay claves duplicadas.

- [ ] **Step 3: Verificar consistencia summary/description del registry**

Run: `cargo test --lib llm_synthetic_tools`
Expected: PASS — incluye el guard que verifica que el `summary` de la `ToolDefinition`
no diverge de `text/tools/*.yaml`.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/text/tools/helpers.yaml
git commit -m "docs(memory): document recall_history pagination in tool text"
```

---

### Task 3: Verificación final de la fase

**Files:** ninguno (solo verificación)

- [ ] **Step 1: Suite completa del módulo + clippy + fmt**

Run:
```bash
cargo test --lib recall_history && \
cargo clippy -p colmena_dag_engine --all-targets -- -D warnings && \
cargo fmt --check
```
Expected: PASS / sin warnings / sin diffs de formato.

- [ ] **Step 2: Smoke E2E real (recall lossless de un mensaje grande)**

Crear un grafo que persista un mensaje grande y luego correr y verificar que un
`recall_history` paginado lo reconstruye. Reusar el patrón ya validado en esta sesión
(`/tmp/colmena_e2e/`). Mínimo: ejecutar un grafo `llm_call` con Postgres
(`--agent-session-id recall_e2e_001`), confirmar en DB que el `content` quedó completo,
y validar `dispatch_recall_history` con `offset` sucesivos cubre `total_chars`.

Run (ejemplo de verificación en DB):
```bash
cd /Users/danielgarcia/startti/colmena && set -a && source .env && set +a && \
psql "$DATABASE_URL" -P pager=off -c \
  "SELECT length(content) FROM llm_node_history WHERE agent_session_id='recall_e2e_001' ORDER BY created_at;"
```
Expected: las longitudes coinciden con el contenido original (sin recorte a 10 KB).
Guardar el SSE en `/tmp/colmena_e2e/recall_e2e.sse` y reportar.

- [ ] **Step 3: (Opcional) limpiar la fila de prueba**

```bash
cd /Users/danielgarcia/startti/colmena && set -a && source .env && set +a && \
psql "$DATABASE_URL" -c "DELETE FROM llm_node_history WHERE agent_session_id='recall_e2e_001';"
```

---

## Self-Review

- **Spec coverage:** cubre la "pieza clave" del spec (§Arquitectura 1 — recall lossless,
  args `offset`/`limit`/`next_offset`, quitar cap de 10 KB). Las demás fases del spec
  (config de modelos baratos, columna `summary` + trait, política por rol + summarizer +
  andamiaje, integración) son **planes separados** (ver Roadmap abajo) — esta fase es
  independiente y shippeable sola.
- **Placeholder scan:** sin TODOs/TBD; todo el código de cada step está completo.
- **Type consistency:** `RecallHistoryArgs { turn, offset, limit }` y las claves del
  resultado (`offset`, `returned_chars`, `total_chars`, `next_offset`) son consistentes
  entre el código del dispatch, los tests y el texto del YAML. `StubRepo`/`key()` ya
  existen en el módulo de tests del archivo.

---

## Roadmap (planes siguientes — se escriben uno por uno)

Esta es la Fase 1 de 4 del spec. Las siguientes, cada una shippeable y testeable:

- **Fase 2 — `cheap_models.yaml` + resolución de modelo barato** (config embebida +
  env override + override por nodo). Independiente.
- **Fase 3 — Columna `summary` + extensión del `ConversationRepository`** (migración
  pg+sqlite; `get` con summaries + `set_summary`; adapters pg/sqlite/in-memory). Sin
  cambio de comportamiento.
- **Fase 4 — Núcleo de compactación** (`SummarizeMessageUseCase`, generalización a
  `compact_discovery_tools_in_history`, política por rol/clase de valor, ventana reciente
  por tokens sobre `content`, integración reemplazando `compact_history_to_summary`,
  migración/backlog con truncado-runtime + E2E creador-de-agentes y agentes-con-tools).

Depende de esta fase: la Fase 4 asume recall lossless para que el resumen con pérdida sea seguro.
