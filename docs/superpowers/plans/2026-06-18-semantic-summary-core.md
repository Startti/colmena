# Núcleo: resumen semántico por rol + andamiaje + integración — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reemplazar el truncado por caracteres por una compactación con conciencia de rol/clase de valor: resumen semántico por mensaje (cacheado en la columna `summary`), andamiaje colapsado a markers, ventana reciente por tokens sobre `content`, integrado en el load del agente.

**Architecture:** Es la **Fase 4** (núcleo) del spec `docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md`. **Depende de:** Fase 1 (recall lossless), Fase 2 (`cheap_model_for`), Fase 3 (columna `summary` + `get_with_summaries`/`set_summary`). El grueso de la lógica son **funciones puras** sobre `&[StoredMessage]` (testeables sin DB ni LLM); la parte async (summarizer) se inyecta por trait y se mockea. La compactación se computa **una vez al cargar** (Hook C) y se mantiene fija durante el run.

**Tech Stack:** Rust, `async_trait`, `tokio::time::timeout`, `LlmRepository`/`LlmProviderFactory`, `MockLlmRepository`/in-memory repo para tests.

---

## File Structure

- `src/libs/colmena/src/llm/domain/message_summarizer.rs` — **nuevo**: trait `MessageSummarizer`.
- `src/libs/colmena/src/llm/infrastructure/message_summarizer/llm_message_summarizer.rs` — **nuevo**: adapter one-shot sobre `LlmRepository`.
- `src/libs/colmena/src/llm/application/history_compaction.rs` — **nuevo**: funciones puras (clase de valor, ventana por tokens, line-builder) + el orquestador `build_compacted_messages`.
- `src/libs/colmena/src/llm/application/agent_service.rs` — **modificar**: usar `build_compacted_messages` al cargar; generalizar el paso de andamiaje; quitar `compact_history_to_summary` del loop.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — **modificar**: construir el `MessageSummarizer` (modelo barato) y pasarlo a `AgentService`.

**Constantes nuevas** (en `history_compaction.rs`):
```rust
pub const SUMMARY_SKIP_THRESHOLD_CHARS: usize = 250; // < esto → verbatim, sin LLM
pub const SUMMARY_TARGET_CHARS: usize = 250;         // target pedido por prompt
pub const RECENT_TOKEN_BUDGET: usize = 2_500;        // ventana reciente (est. chars/4)
pub const DISCOVERY_KEEP_RECENT_MSGS: usize = 8;     // andamiaje reciente full
pub const SUMMARY_KEEP_FIRST_MSGS: usize = 2;        // primeros mensajes full
pub const SUMMARY_MAX_LINES: usize = 100;            // tope de líneas del resumen
pub const SUMMARIZE_PER_LOAD_CAP: usize = 30;        // máx summaries nuevos por load
```

---

### Task 1: Trait `MessageSummarizer` + adapter LLM

**Files:**
- Create: `src/libs/colmena/src/llm/domain/message_summarizer.rs`
- Create: `src/libs/colmena/src/llm/infrastructure/message_summarizer/llm_message_summarizer.rs`
- Create: `src/libs/colmena/src/llm/infrastructure/message_summarizer/mod.rs`
- Modify: `src/libs/colmena/src/llm/domain/mod.rs` y `src/libs/colmena/src/llm/infrastructure/mod.rs` (registrar módulos)

- [ ] **Step 1: Definir el trait de dominio**

`message_summarizer.rs`:

```rust
use crate::llm::domain::LlmError;
use async_trait::async_trait;

/// Resume un único bloque de texto a una línea concisa (~250 chars), sin historia.
/// La implementación real usa un modelo barato; los tests usan un stub.
#[async_trait]
pub trait MessageSummarizer: Send + Sync {
    /// `target_chars` es un objetivo blando (se pide por prompt, no se hard-corta).
    async fn summarize(&self, text: &str, target_chars: usize) -> Result<String, LlmError>;
}
```

Registrar en `domain/mod.rs`: `pub mod message_summarizer; pub use message_summarizer::MessageSummarizer;`

- [ ] **Step 2: Escribir el test del adapter (con `MockLlmRepository`)**

`llm_message_summarizer.rs` (módulo de tests inline):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{LlmRequestId, LlmResponse, MockLlmRepository, ProviderKind, LlmProvider};

    fn mock_response(text: &str) -> LlmResponse {
        LlmResponse::new(
            LlmRequestId::from_string("req-sum".into()).unwrap(),
            text.into(),
            LlmProvider::new(ProviderKind::Mock, "k".into(), Some("m".into())).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn summarize_returns_trimmed_one_line() {
        let mut mock = MockLlmRepository::new();
        mock.expect_call()
            .times(1)
            .returning(|_| Ok(mock_response("  Resumió la cadena de cálculos.\n")));
        let s = LlmMessageSummarizer::new(
            std::sync::Arc::new(mock),
            ProviderKind::Mock,
            "k".into(),
            "m".into(),
            std::time::Duration::from_secs(5),
        );
        let out = s.summarize("texto largo...", 250).await.unwrap();
        assert_eq!(out, "Resumió la cadena de cálculos.");
        assert!(!out.contains('\n'));
    }
}
```

- [ ] **Step 3: Implementar el adapter (patrón de `LlmAttachmentSummaryGenerator`)**

`llm_message_summarizer.rs`:

```rust
//! Adapter one-shot de `MessageSummarizer`: hace una llamada sin historia con un
//! modelo barato y NO hard-corta la salida (el target es blando, por prompt).
//! Bypassa `LlmCallUseCase`, así el turno nunca entra a `llm_node_history`.

use crate::llm::domain::{
    LlmConfig, LlmError, LlmMessage, LlmProvider, LlmRepository, LlmRequest, MessageSummarizer,
    ProviderKind,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

pub struct LlmMessageSummarizer {
    repo: Arc<dyn LlmRepository>,
    provider: ProviderKind,
    api_key: String,
    model: String,
    timeout: Duration,
}

impl LlmMessageSummarizer {
    pub fn new(
        repo: Arc<dyn LlmRepository>,
        provider: ProviderKind,
        api_key: String,
        model: String,
        timeout: Duration,
    ) -> Self {
        Self { repo, provider, api_key, model, timeout }
    }
}

#[async_trait]
impl MessageSummarizer for LlmMessageSummarizer {
    async fn summarize(&self, text: &str, target_chars: usize) -> Result<String, LlmError> {
        let system = format!(
            "Resumí el siguiente mensaje de una conversación en ~{target_chars} caracteres, \
             en UNA línea, conservando lo accionable (hechos, decisiones, resultados, ids). \
             Sin markdown, sin comillas, sin comentarios. Solo el resumen."
        );
        let sys = LlmMessage::system(system)?;
        let usr = LlmMessage::user(text.to_string())?;
        let provider = LlmProvider::new(
            self.provider.clone(),
            self.api_key.clone(),
            Some(self.model.clone()),
        )?;
        let request = LlmRequest::new(vec![sys, usr], LlmConfig::new(provider), false)?;

        let response = match tokio::time::timeout(self.timeout, self.repo.call(request)).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(LlmError::RequestFailed {
                    message: format!("summarizer timeout after {:?}", self.timeout),
                })
            }
        };

        let out = response
            .content()
            .trim()
            .trim_matches('"')
            .replace(['\n', '\r'], " ")
            .trim()
            .to_string();
        if out.is_empty() {
            return Err(LlmError::RequestFailed {
                message: "summarizer returned empty".into(),
            });
        }
        Ok(out) // NO hard-cut: el target es blando.
    }
}
```

`message_summarizer/mod.rs`: `pub mod llm_message_summarizer; pub use llm_message_summarizer::LlmMessageSummarizer;`
Registrar en `infrastructure/mod.rs`: `pub mod message_summarizer;`

- [ ] **Step 4: Correr tests + commit**

Run: `cargo test --lib message_summarizer`
Expected: PASS.

```bash
git add src/libs/colmena/src/llm/domain/message_summarizer.rs \
        src/libs/colmena/src/llm/domain/mod.rs \
        src/libs/colmena/src/llm/infrastructure/message_summarizer/ \
        src/libs/colmena/src/llm/infrastructure/mod.rs
git commit -m "feat(memory): MessageSummarizer trait + cheap one-shot LLM adapter"
```

---

### Task 2: Generalizar andamiaje a `compact_discovery_tools_in_history`

**Files:**
- Modify: `src/libs/colmena/src/llm/application/agent_service.rs` (la función `compact_old_load_skill_in_history`, ~líneas 875-951, y su llamada en el loop ~línea 218)

- [ ] **Step 1: Test que falla (describe_tool también se colapsa)**

Agregar al módulo de tests de `agent_service.rs`:

```rust
    #[test]
    fn discovery_compaction_markers_old_describe_tool() {
        // 12 mensajes: un describe_tool viejo + relleno para superar keep_recent=8.
        let mut msgs = vec![
            LlmMessage::user("hola".into()).unwrap(),
            LlmMessage::assistant_with_tool_calls(
                String::new(),
                vec![tool_call("c1", "describe_tool", r#"{"name":"sql_query"}"#)],
            )
            .unwrap(),
            LlmMessage::tool("c1", "# sql_query\n\n<schema gigante de 5000 chars...>".into())
                .unwrap(),
        ];
        for i in 0..9 {
            msgs.push(LlmMessage::user(format!("relleno {i}")).unwrap());
        }
        let out = compact_discovery_tools_in_history(&msgs, DISCOVERY_KEEP_RECENT_MSGS);
        // El tool result viejo del describe_tool quedó como marker corto.
        assert!(out[2].content().starts_with("[tool 'describe_tool'"));
        assert!(out[2].content().len() < 120);
    }
```

- [ ] **Step 2: Correr y verificar que falla**

Run: `cargo test --lib discovery_compaction_markers_old_describe_tool`
Expected: FAIL — la función `compact_discovery_tools_in_history` no existe aún.

- [ ] **Step 3: Renombrar y generalizar la función**

Renombrar `compact_old_load_skill_in_history` → `compact_discovery_tools_in_history` y
ampliar el set de nombres detectados. Reemplazar el bloque que arma `load_skill_calls`
(que hoy filtra `if tc.function.name == "load_skill"`) por un set de nombres de
descubrimiento, y el marker para usar el nombre real de la tool:

```rust
/// Nombres de tools de "andamiaje": discovery/scaffolding del lazy loading + skills.
/// Sus resultados viejos se colapsan a markers (recuperables re-llamando la tool).
const DISCOVERY_TOOL_NAMES: &[&str] = &["load_skill", "describe_tool"];

fn compact_discovery_tools_in_history(
    messages: &[LlmMessage],
    keep_recent_msgs: usize,
) -> Vec<LlmMessage> {
    let mut out: Vec<LlmMessage> = messages.to_vec();
    if out.len() <= keep_recent_msgs {
        return out;
    }

    // tool_call_id → (tool_name, arguments) para las discovery tools.
    let mut discovery_calls: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    for msg in out.iter() {
        if let Some(tcs) = msg.tool_calls() {
            for tc in tcs {
                if DISCOVERY_TOOL_NAMES.contains(&tc.function.name.as_str()) {
                    discovery_calls.insert(
                        tc.id.clone(),
                        (tc.function.name.clone(), tc.function.arguments.clone()),
                    );
                }
            }
        }
    }
    if discovery_calls.is_empty() {
        return out;
    }

    let boundary = out.len().saturating_sub(keep_recent_msgs);
    let mut to_compact: Vec<(usize, String, String)> = Vec::new();
    for (i, msg) in out.iter().enumerate().take(boundary) {
        if msg.role() != &MessageRole::Tool {
            continue;
        }
        let Some(tcid) = msg.tool_call_id().map(|s| s.to_string()) else {
            continue;
        };
        let Some((name, args)) = discovery_calls.get(&tcid) else {
            continue;
        };
        // Idempotente: saltar los ya marcados.
        if msg.content().starts_with("[tool '") && msg.content().ends_with(']') {
            continue;
        }
        to_compact.push((i, name.clone(), args.clone()));
    }

    for (i, name, _args) in to_compact {
        let original_size = out[i].content().len();
        let tcid = out[i].tool_call_id().unwrap_or("unknown").to_string();
        let marker = format!(
            "[tool '{name}' result loaded earlier ({original_size} chars). \
             Call {name} again to re-read.]"
        );
        if let Ok(new_msg) = LlmMessage::tool(tcid, marker) {
            out[i] = new_msg;
        }
    }

    out
}
```

Actualizar la llamada en el loop (~línea 218) de
`compact_old_load_skill_in_history(&messages, COMPACT_LOAD_SKILL_KEEP_RECENT_MSGS)`
a `compact_discovery_tools_in_history(&messages, DISCOVERY_KEEP_RECENT_MSGS)`.
Si `COMPACT_LOAD_SKILL_KEEP_RECENT_MSGS` queda sin uso, eliminarlo (deny-warnings) o
reusarlo como `DISCOVERY_KEEP_RECENT_MSGS`.

- [ ] **Step 4: Correr tests (nuevos + los viejos de load_skill siguen verdes) + commit**

Run: `cargo test --lib compact_discovery_tools_in_history && cargo test --lib compact_`
Expected: PASS — el test nuevo + los existentes de compactación de load_skill (que ahora
pasan por la función generalizada).

```bash
git add src/libs/colmena/src/llm/application/agent_service.rs
git commit -m "feat(memory): generalize discovery-tool compaction (load_skill + describe_tool)"
```

---

### Task 3: Funciones puras — clase de valor y ventana reciente por tokens

**Files:**
- Create: `src/libs/colmena/src/llm/application/history_compaction.rs`
- Modify: `src/libs/colmena/src/llm/application/mod.rs` (registrar el módulo)

- [ ] **Step 1: Tests de las funciones puras**

`history_compaction.rs` (módulo de tests inline):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{LlmMessage, ToolCall};
    use crate::llm::domain::tools::FunctionCall;

    fn tc(id: &str, name: &str) -> ToolCall {
        ToolCall::new(id.into(), FunctionCall { name: name.into(), arguments: "{}".into() })
    }

    #[test]
    fn classifies_scaffolding_vs_content() {
        let msgs = vec![
            LlmMessage::user("pregunta real".into()).unwrap(),                       // content
            LlmMessage::assistant_with_tool_calls(String::new(), vec![tc("c1", "describe_tool")]).unwrap(), // scaffolding
            LlmMessage::tool("c1", "schema...".into()).unwrap(),                     // scaffolding (result de describe_tool)
            LlmMessage::assistant_with_tool_calls(String::new(), vec![tc("c2", "sql_query")]).unwrap(),     // content (tool_call real)
            LlmMessage::tool("c2", "rows...".into()).unwrap(),                       // content (result real)
        ];
        let classes = classify_value_class(&msgs);
        assert_eq!(classes[0], ValueClass::Content);
        assert_eq!(classes[1], ValueClass::Scaffolding);
        assert_eq!(classes[2], ValueClass::Scaffolding);
        assert_eq!(classes[3], ValueClass::Content);
        assert_eq!(classes[4], ValueClass::Content);
    }

    #[test]
    fn recent_boundary_counts_only_content_tokens() {
        // 6 mensajes content de ~400 chars (~100 tokens c/u). Budget 250 tokens → ~2-3 caben.
        let big = "x".repeat(400);
        let msgs: Vec<LlmMessage> = (0..6).map(|_| LlmMessage::user(big.clone()).unwrap()).collect();
        let classes = vec![ValueClass::Content; 6];
        let b = recent_boundary_by_tokens(&msgs, &classes, 250);
        assert!(b >= 3 && b <= 4, "boundary fue {b}");
    }

    #[test]
    fn rendered_size_includes_tool_call_args() {
        let m = LlmMessage::assistant_with_tool_calls(
            String::new(),
            vec![ToolCall::new("c".into(), FunctionCall { name: "f".into(), arguments: "x".repeat(300) })],
        )
        .unwrap();
        assert!(rendered_size(&m) >= 300);
    }
}
```

- [ ] **Step 2: Correr y verificar que falla**

Run: `cargo test --lib history_compaction`
Expected: FAIL (módulo/funciones no existen).

- [ ] **Step 3: Implementar las funciones puras**

`history_compaction.rs` (encabezado + funciones; las constantes del File Structure van acá):

```rust
//! Funciones puras de compactación de historial (Fase 4).
//! Operan sobre slices de mensajes; la parte async (summarizer) vive en el orquestador.

use crate::llm::domain::{LlmMessage, MessageRole};
use std::collections::HashMap;

pub const SUMMARY_SKIP_THRESHOLD_CHARS: usize = 250;
pub const SUMMARY_TARGET_CHARS: usize = 250;
pub const RECENT_TOKEN_BUDGET: usize = 2_500;
pub const DISCOVERY_KEEP_RECENT_MSGS: usize = 8;
pub const SUMMARY_KEEP_FIRST_MSGS: usize = 2;
pub const SUMMARY_MAX_LINES: usize = 100;
pub const SUMMARIZE_PER_LOAD_CAP: usize = 30;

const DISCOVERY_TOOL_NAMES: &[&str] = &["load_skill", "describe_tool"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueClass {
    Scaffolding,
    Content,
}

/// Tamaño "renderizado" del mensaje: content + args de tool_calls serializados.
pub fn rendered_size(msg: &LlmMessage) -> usize {
    let mut n = msg.content().chars().count();
    if let Some(tcs) = msg.tool_calls() {
        for tc in tcs {
            n += tc.function.name.chars().count() + tc.function.arguments.chars().count();
        }
    }
    n
}

/// Estimación de tokens (chars/4), consistente con los dumps del repo.
fn est_tokens(msg: &LlmMessage) -> usize {
    rendered_size(msg) / 4 + 1
}

/// Clasifica cada mensaje como Scaffolding (round-trip de discovery tools) o Content.
pub fn classify_value_class(messages: &[LlmMessage]) -> Vec<ValueClass> {
    // tool_call_id → nombre, solo para discovery tools.
    let mut discovery_ids: HashMap<String, ()> = HashMap::new();
    for m in messages {
        if let Some(tcs) = m.tool_calls() {
            for tc in tcs {
                if DISCOVERY_TOOL_NAMES.contains(&tc.function.name.as_str()) {
                    discovery_ids.insert(tc.id.clone(), ());
                }
            }
        }
    }
    messages
        .iter()
        .map(|m| {
            let is_scaffold = match m.role() {
                MessageRole::Assistant => m
                    .tool_calls()
                    .map(|tcs| {
                        !tcs.is_empty()
                            && tcs
                                .iter()
                                .all(|tc| DISCOVERY_TOOL_NAMES.contains(&tc.function.name.as_str()))
                    })
                    .unwrap_or(false),
                MessageRole::Tool => m
                    .tool_call_id()
                    .map(|id| discovery_ids.contains_key(id))
                    .unwrap_or(false),
                _ => false,
            };
            if is_scaffold {
                ValueClass::Scaffolding
            } else {
                ValueClass::Content
            }
        })
        .collect()
}

/// Borde `B` de la ventana reciente: camina desde el final acumulando tokens SOLO de
/// mensajes `Content` hasta `token_budget`. Devuelve el índice del primer mensaje reciente.
pub fn recent_boundary_by_tokens(
    messages: &[LlmMessage],
    classes: &[ValueClass],
    token_budget: usize,
) -> usize {
    let mut budget = token_budget as i64;
    let mut b = messages.len();
    for i in (0..messages.len()).rev() {
        if classes[i] == ValueClass::Content {
            budget -= est_tokens(&messages[i]) as i64;
            if budget < 0 {
                break;
            }
        }
        b = i;
    }
    b
}
```

Registrar en `application/mod.rs`: `pub mod history_compaction;`

- [ ] **Step 4: Correr tests + commit**

Run: `cargo test --lib history_compaction`
Expected: PASS.

```bash
git add src/libs/colmena/src/llm/application/history_compaction.rs src/libs/colmena/src/llm/application/mod.rs
git commit -m "feat(memory): pure value-class + token-window helpers for compaction"
```

---

### Task 4: Orquestador `build_compacted_messages` (async, con summarizer + cache)

**Files:**
- Modify: `src/libs/colmena/src/llm/application/history_compaction.rs`

- [ ] **Step 1: Test con summarizer stub + repo in-memory**

Agregar al módulo de tests de `history_compaction.rs`:

```rust
    use crate::llm::domain::{
        AgentSessionId, ConversationKey, ConversationRepository, MessageSummarizer, NodeIdPath,
        SessionId, StoredMessage,
    };
    use crate::llm::infrastructure::persistence::InMemoryConversationRepository;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct StubSummarizer;
    #[async_trait]
    impl MessageSummarizer for StubSummarizer {
        async fn summarize(&self, _text: &str, _t: usize) -> Result<String, crate::llm::domain::LlmError> {
            Ok("RESUMEN".to_string())
        }
    }

    fn key() -> ConversationKey {
        ConversationKey {
            session_id: SessionId("s".into()),
            agent_session_id: Some(AgentSessionId("a".into())),
            node_id: NodeIdPath("n".into()),
        }
    }

    #[tokio::test]
    async fn old_long_nl_gets_summarized_and_cached_recent_stays_full() {
        let repo = Arc::new(InMemoryConversationRepository::new());
        let k = key();
        // 0,1 = keep_first; 2..n-? viejos; últimos = recientes.
        let long = "y".repeat(600); // ≥250 → debe resumirse
        for i in 0..10 {
            let m = LlmMessage::user(format!("{long} msg{i}")).unwrap();
            repo.add_message(&k, m).await.unwrap();
        }
        let stored: Vec<StoredMessage> = repo.get_with_summaries(&k).await.unwrap();
        let summarizer: Arc<dyn MessageSummarizer> = Arc::new(StubSummarizer);

        let out = build_compacted_messages(
            &stored,
            &k,
            repo.as_ref(),
            Some(&summarizer),
            RECENT_TOKEN_BUDGET,
        )
        .await;

        // Hay un mensaje system de resumen con líneas [Tn].
        assert!(out.iter().any(|m| m.role() == &MessageRole::System
            && m.content().contains("[T2]")));
        // El summary se persistió en cache (ordinal 2).
        let after = repo.get_with_summaries(&k).await.unwrap();
        assert_eq!(after[2].summary.as_deref(), Some("RESUMEN"));
    }

    #[tokio::test]
    async fn short_messages_pass_verbatim_no_summarizer_call() {
        // Si todo es corto (<250) y cabe en la ventana, no se arma bloque de resumen.
        let repo = Arc::new(InMemoryConversationRepository::new());
        let k = key();
        for i in 0..3 {
            repo.add_message(&k, LlmMessage::user(format!("corto {i}")).unwrap())
                .await
                .unwrap();
        }
        let stored = repo.get_with_summaries(&k).await.unwrap();
        let out = build_compacted_messages(&stored, &k, repo.as_ref(), None, RECENT_TOKEN_BUDGET).await;
        assert_eq!(out.len(), 3); // todos full, sin bloque de resumen
    }
```

- [ ] **Step 2: Correr y verificar que falla**

Run: `cargo test --lib history_compaction`
Expected: FAIL (`build_compacted_messages` no existe).

- [ ] **Step 3: Implementar el orquestador**

Agregar a `history_compaction.rs`. Reglas: clasifica → andamiaje viejo a marker → borde por
tokens sobre content → zona vieja `[KEEP_FIRST..B)` arma líneas `[Tn]` por política de rol
(scaffolding=marker, tool_calls=estructural, NL`<250`=verbatim, NL`≥250`=cache∥resumir∥
bridge-truncado), persiste summaries nuevos (cap por load), ensambla
`keep_first + [system resumen] + recientes`.

```rust
use crate::llm::domain::{ConversationKey, ConversationRepository, MessageSummarizer, StoredMessage};

/// Trunca a `cap` chars (char-safe) con elipsis — SOLO puente runtime (nunca se persiste).
fn bridge_truncate(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let kept: String = s.chars().take(cap).collect();
    format!("{kept}…")
}

/// Construye el contexto compactado para enviar al LLM. Computar UNA vez al cargar.
pub async fn build_compacted_messages(
    stored: &[StoredMessage],
    key: &ConversationKey,
    repo: &dyn ConversationRepository,
    summarizer: Option<&std::sync::Arc<dyn MessageSummarizer>>,
    recent_token_budget: usize,
) -> Vec<LlmMessage> {
    let messages: Vec<LlmMessage> = stored.iter().map(|s| s.message.clone()).collect();
    let total = messages.len();
    let keep_first = SUMMARY_KEEP_FIRST_MSGS;
    if total <= keep_first + 1 {
        return messages;
    }

    let classes = classify_value_class(&messages);
    let mut b = recent_boundary_by_tokens(&messages, &classes, recent_token_budget);

    // Guard de pares: no cortar dejando un Tool sin su Assistant.
    while b > keep_first && matches!(messages[b].role(), MessageRole::Tool) {
        b -= 1;
    }
    if b <= keep_first {
        return messages;
    }

    // tool_call_id → name para nombrar líneas estructurales/markers.
    let mut tool_names: HashMap<String, String> = HashMap::new();
    for m in &messages {
        if let Some(tcs) = m.tool_calls() {
            for tc in tcs {
                tool_names.insert(tc.id.clone(), tc.function.name.clone());
            }
        }
    }

    let mut lines: Vec<String> = Vec::new();
    let mut summarized_this_load = 0usize;

    for idx in keep_first..b {
        let msg = &messages[idx];
        let line = match classes[idx] {
            ValueClass::Scaffolding => {
                let name = msg
                    .tool_calls()
                    .and_then(|t| t.first())
                    .map(|t| t.function.name.clone())
                    .or_else(|| msg.tool_call_id().and_then(|id| tool_names.get(id).cloned()))
                    .unwrap_or_else(|| "discovery".into());
                format!("[T{idx}] (andamiaje: {name} — re-llamar para releer)")
            }
            ValueClass::Content => {
                if let Some(tcs) = msg.tool_calls() {
                    // tool_calls reales → línea estructural.
                    let calls: Vec<String> = tcs
                        .iter()
                        .map(|tc| format!("{}({})", tc.function.name,
                            bridge_truncate(&tc.function.arguments, 120)))
                        .collect();
                    format!("[T{idx}] AGENT llamó {}", calls.join("; "))
                } else if rendered_size(msg) < SUMMARY_SKIP_THRESHOLD_CHARS {
                    format!("[T{idx}] {}: {}", role_tag(msg), msg.content())
                } else if let Some(cached) = stored[idx].summary.as_deref() {
                    format!("[T{idx}] {}: {}", role_tag(msg), cached)
                } else if let (Some(sz), true) =
                    (summarizer, summarized_this_load < SUMMARIZE_PER_LOAD_CAP)
                {
                    match sz.summarize(msg.content(), SUMMARY_TARGET_CHARS).await {
                        Ok(s) => {
                            let _ = repo.set_summary(key, idx, &s).await; // cache (best-effort)
                            summarized_this_load += 1;
                            format!("[T{idx}] {}: {}", role_tag(msg), s)
                        }
                        Err(_) => format!(
                            "[T{idx}] {}: {} (completo en recall_history(turn={idx}))",
                            role_tag(msg),
                            bridge_truncate(msg.content(), SUMMARY_TARGET_CHARS)
                        ),
                    }
                } else {
                    // Sin summarizer o cap alcanzado → puente truncado (recuperable).
                    format!(
                        "[T{idx}] {}: {} (completo en recall_history(turn={idx}))",
                        role_tag(msg),
                        bridge_truncate(msg.content(), SUMMARY_TARGET_CHARS)
                    )
                }
            }
        };
        lines.push(line);
    }

    // Cap de líneas: drop de las más viejas (recuperables por turno).
    let dropped = lines.len().saturating_sub(SUMMARY_MAX_LINES);
    let kept: Vec<String> = lines.into_iter().skip(dropped).collect();

    let mut summary = String::from("## Conversation summary (older turns)\n");
    summary.push_str(
        "Cada línea es un mensaje anterior. El [Tn] es el índice de turno: usá \
         recall_history(turn=N) para releer el original completo.\n\n",
    );
    if dropped > 0 {
        summary.push_str(&format!("(turnos {keep_first}..{} omitidos — recuperables)\n",
            keep_first + dropped - 1));
    }
    for l in &kept {
        summary.push_str(l);
        summary.push('\n');
    }

    let mut out: Vec<LlmMessage> = Vec::new();
    out.extend(messages[..keep_first].iter().cloned());
    // Merge en el system previo si el último keep_first es System (evita systems consecutivos).
    if keep_first > 0 && matches!(messages[keep_first - 1].role(), MessageRole::System) {
        let combined = format!("{}\n\n---\n\n{}", messages[keep_first - 1].content(), summary);
        out.pop();
        out.push(LlmMessage::system(combined).unwrap_or_else(|_| messages[keep_first - 1].clone()));
    } else if let Ok(s) = LlmMessage::system(summary) {
        out.push(s);
    }
    out.extend(messages[b..].iter().cloned());
    out
}

fn role_tag(m: &LlmMessage) -> &'static str {
    match m.role() {
        MessageRole::User => "USER",
        MessageRole::System => "SYSTEM",
        MessageRole::Assistant => "AGENT",
        MessageRole::Tool => "TOOL",
    }
}
```

- [ ] **Step 4: Correr tests + commit**

Run: `cargo test --lib history_compaction`
Expected: PASS.

```bash
git add src/libs/colmena/src/llm/application/history_compaction.rs
git commit -m "feat(memory): build_compacted_messages — role-aware semantic summary + cache"
```

---

### Task 5: Integración en `agent_service.run` + wiring en `llm.rs`

**Files:**
- Modify: `src/libs/colmena/src/llm/application/agent_service.rs` (load ~138-184; loop compaction ~218-233; struct/params para inyectar el summarizer)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (construir el summarizer y pasarlo; ~línea 2223 donde se crea `AgentService`)

- [ ] **Step 1: Inyectar el summarizer en `AgentService`**

`AgentService` gana un campo opcional `message_summarizer: Option<Arc<dyn MessageSummarizer>>`.
Agregar un setter builder (no romper `AgentService::new`):

```rust
    pub fn with_message_summarizer(
        mut self,
        summarizer: std::sync::Arc<dyn crate::llm::domain::MessageSummarizer>,
    ) -> Self {
        self.message_summarizer = Some(summarizer);
        self
    }
```

(Y agregar el campo al struct con default `None` en `new`.)

- [ ] **Step 2: Reemplazar el load + la compactación por iteración**

En `run`, reemplazar el bloque de carga (hoy `get_by_id` + shim + push del prompt) para usar
`get_with_summaries` y construir el contexto compactado UNA vez tras agregar el prompt:

```rust
        // 1. Cargar historia con summaries (orden crudo de DB).
        let stored = self.conversation_repository.get_with_summaries(session_id).await?;
        // (shim temporal: aplicar strip_leading_temporal_block sobre stored[i].message
        //  cuando role==System, SIN dropear filas — mantener alineación de índices.
        //  Si el system quedara vacío, dejar un placeholder mínimo en vez de eliminar.)
        let mut messages: Vec<crate::llm::domain::LlmMessage> =
            stored.iter().map(|s| s.message.clone()).collect();
```

Luego, donde hoy se agrega el prompt/custom_messages (líneas 171-184) — sin cambios en la
persistencia (`add_message`). Después, **reemplazar** las dos llamadas de compactación por
iteración (líneas ~218-233) por:
- mantener `compact_discovery_tools_in_history(&messages, DISCOVERY_KEEP_RECENT_MSGS)` (Task 2), y
- **quitar** `compact_history_to_summary(...)`; en su lugar, computar el bloque compactado
  con `build_compacted_messages` **una sola vez** antes del loop (con los `stored` recargados
  tras persistir el prompt) y reutilizarlo:

```rust
        // Computar el contexto compactado UNA vez (Hook C). Recarga con summaries
        // para incluir el prompt recién persistido y cualquier summary cacheado.
        let stored_now = self.conversation_repository.get_with_summaries(session_id).await?;
        let base_compacted = crate::llm::application::history_compaction::build_compacted_messages(
            &stored_now,
            session_id,
            self.conversation_repository.as_ref(),
            self.message_summarizer.as_ref(),
            crate::llm::application::history_compaction::RECENT_TOKEN_BUDGET,
        )
        .await;
```

Dentro del loop, `request_messages` se arma a partir de `base_compacted` + los mensajes
generados en el run (assistant/tool de esta corrida), aplicando solo el paso barato de
andamiaje:

```rust
            let request_messages = compact_discovery_tools_in_history(
                &live_messages, // base_compacted + lo agregado en el loop
                DISCOVERY_KEEP_RECENT_MSGS,
            );
```

> **Nota de implementación (anclas):** el loop actual acumula en `messages`. La refactor
> mantiene `messages` para persistencia/append y deriva `live_messages = base_compacted`
> seguido de los mensajes nuevos de esta corrida (los que se agregan tras el load). Eliminar
> el uso de `compact_history_to_summary` y sus constantes `COMPACT_SUMMARY_*` si quedan
> huérfanas (deny-warnings). Conservar `summary_line_for_message`/`compact_history_to_summary`
> solo si algún test los referencia; si no, borrarlos.

- [ ] **Step 3: Wiring en `llm.rs` — construir el summarizer con modelo barato**

Donde se crea `AgentService` (~línea 2223), construir el `MessageSummarizer` con el provider
del nodo y el modelo barato resuelto, respetando el override por nodo `summary_model`:

```rust
        let summary_model = config
            .get("summary_model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                crate::llm::infrastructure::cheap_model_for(provider_kind.clone())
            });
        let summarizer: std::sync::Arc<dyn crate::llm::domain::MessageSummarizer> =
            std::sync::Arc::new(
                crate::llm::infrastructure::message_summarizer::LlmMessageSummarizer::new(
                    LlmProviderFactory::create(provider_kind.clone()),
                    provider_kind.clone(),
                    api_key.clone(),
                    summary_model,
                    std::time::Duration::from_secs(10),
                ),
            );
        let agent_service = AgentService::new(llm_repo_arc, conversation_repo.clone())
            .with_message_summarizer(summarizer);
```

- [ ] **Step 4: Test de integración (mock summarizer vía AgentService)**

Agregar a los tests de `agent_service.rs` un test que arme una conversación larga vía un
`MockConversationRepository`/in-memory + `MockLlmRepository` para el loop, inyecte un
`StubSummarizer`, y verifique que el primer request al LLM contiene el bloque
`## Conversation summary` con líneas `[Tn]` y que los mensajes recientes van full. (Reusar
el patrón de mocks ya presente en el módulo de tests del archivo, ~línea 1660+.)

- [ ] **Step 5: Build + tests + commit**

Run: `cargo test --lib agent_service && cargo clippy -p colmena_dag_engine --all-targets -- -D warnings`
Expected: PASS / sin warnings.

```bash
git add src/libs/colmena/src/llm/application/agent_service.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(memory): integrate semantic summary at load; remove per-iteration char truncation"
```

---

### Task 6: Verificación E2E real (los 3 arquetipos)

**Files:** grafos de prueba en `/tmp/colmena_e2e/`

- [ ] **Step 1: `cargo test --verbose` completo (pre-push)**

Run: `set -a && source /Users/danielgarcia/startti/colmena/.env && set +a && cargo test --verbose`
Expected: PASS (unit + integration + doctests; CI usa `--verbose`).

- [ ] **Step 2: E2E chat normal (no regresión)**

Correr una conversación multi-turn con `--agent-session-id e2e_chat_norm` (>8 mensajes),
guardar SSE en `/tmp/colmena_e2e/`, y con `COLMENA_DUMP_PROMPT_FULL=1` confirmar que el
medio aparece como líneas `[Tn]` semánticas (no prefijos de 180) y los recientes full.

- [ ] **Step 3: E2E creador-de-agentes (artefacto recuperable)**

Grafo donde el agente emite un artefacto JSON grande (>10 KB), que envejezca, y luego se le
pida editarlo: verificar que llama `recall_history` paginado y reconstruye el JSON completo.
Guardar SSE + reporte.

- [ ] **Step 4: E2E agente con tools/skills (andamiaje colapsado)**

Grafo con lazy loading + `describe_tool`/`load_skill` + tools reales (`sql_query`/`add`).
Con `COLMENA_DUMP_PROMPT_FULL=1`, confirmar que los round-trips viejos de `describe_tool`/
`load_skill` aparecen como markers (no schemas full) y que la pregunta del user + resultados
con datos siguen presentes (full o resumidos, no como andamiaje). Guardar SSE + reporte.

- [ ] **Step 5: Barrido ADP (breaking changes)**

Confirmar que ningún cambio rompe el worker ADP: la API pública de colmena
(`EngineConfig`/`ColmenaEngine`/traits exportados) no cambió de firma (solo se agregaron
métodos default al trait `ConversationRepository` y campos opcionales). Revisar
`apps/service/ia/platform/{worker,api}/src/` en el repo ADP por usos directos del trait.

---

## Self-Review

- **Spec coverage:** política por rol/clase de valor (Task 3-4), andamiaje generalizado
  (Task 2), summarizer barato (Task 1), ventana reciente por tokens sobre content (Task 3),
  recovery-aware + bridge-truncado (Task 4), integración Hook C + remoción del truncado de
  180 (Task 5), E2E de los 3 arquetipos (Task 6). Invariante #2 (shim sin dropear) anotado
  en Task 5 Step 2.
- **Desvío consciente del spec:** el spec menciona "lote+concurrencia"; este plan implementa
  un **cap secuencial por load** (`SUMMARIZE_PER_LOAD_CAP`) + bridge-truncado, más simple y
  correcto sin combinators de streams. La concurrencia queda como optimización futura.
- **Placeholder scan:** sin TODOs; el código de las funciones puras y el adapter está
  completo. Las anclas de Task 5 referencian líneas/identificadores reales ya inspeccionados.
- **Type consistency:** `ValueClass`, `rendered_size`, `classify_value_class`,
  `recent_boundary_by_tokens`, `build_compacted_messages(stored, key, repo, summarizer, budget)`
  y `MessageSummarizer::summarize(text, target_chars)` son consistentes entre funciones, tests
  e integración. Reusa `StoredMessage`/`set_summary`/`get_with_summaries` de la Fase 3,
  `cheap_model_for` de la Fase 2 y `recall_history` lossless de la Fase 1.
