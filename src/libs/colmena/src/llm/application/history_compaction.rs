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

/// Classifies a message as Scaffolding (discovery tool round-trips) or Content.
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

use crate::llm::domain::{
    ConversationKey, ConversationRepository, MessageSummarizer, StoredMessage,
};

/// Trunca a `cap` chars (char-safe) con elipsis — SOLO puente runtime (nunca se persiste).
fn bridge_truncate(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let kept: String = s.chars().take(cap).collect();
    format!("{kept}…")
}

fn role_tag(m: &LlmMessage) -> &'static str {
    match m.role() {
        MessageRole::User => "USER",
        MessageRole::System => "SYSTEM",
        MessageRole::Assistant => "AGENT",
        MessageRole::Tool => "TOOL",
    }
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

    // tool_call_id → name (para líneas estructurales / markers).
    let mut tool_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
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
                    .or_else(|| {
                        msg.tool_call_id()
                            .and_then(|id| tool_names.get(id).cloned())
                    })
                    .unwrap_or_else(|| "discovery".into());
                format!("[T{idx}] (andamiaje: {name} — re-llamar para releer)")
            }
            ValueClass::Content => {
                if let Some(tcs) = msg.tool_calls() {
                    let calls: Vec<String> = tcs
                        .iter()
                        .map(|tc| {
                            format!(
                                "{}({})",
                                tc.function.name,
                                bridge_truncate(&tc.function.arguments, 120)
                            )
                        })
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
                            let _ = repo.set_summary(key, idx, &s).await;
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
        summary.push_str(&format!(
            "(turnos {keep_first}..{} omitidos — recuperables)\n",
            keep_first + dropped - 1
        ));
    }
    for l in &kept {
        summary.push_str(l);
        summary.push('\n');
    }

    let mut out: Vec<LlmMessage> = Vec::new();
    out.extend(messages[..keep_first].iter().cloned());
    // Merge en el system previo si el último keep_first es System (evita systems consecutivos).
    if keep_first > 0 && matches!(messages[keep_first - 1].role(), MessageRole::System) {
        let combined = format!(
            "{}\n\n---\n\n{}",
            messages[keep_first - 1].content(),
            summary
        );
        out.pop();
        out.push(LlmMessage::system(combined).unwrap_or_else(|_| messages[keep_first - 1].clone()));
    } else if let Ok(s) = LlmMessage::system(summary) {
        out.push(s);
    }
    out.extend(messages[b..].iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::tools::FunctionCall;
    use crate::llm::domain::{LlmMessage, ToolCall};

    fn tc(id: &str, name: &str) -> ToolCall {
        ToolCall::new(
            id.to_string(),
            FunctionCall {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
        )
    }

    #[test]
    fn classifies_scaffolding_vs_content() {
        let msgs = vec![
            LlmMessage::user("pregunta real".to_string()).unwrap(),
            LlmMessage::assistant_with_tool_calls(String::new(), vec![tc("c1", "describe_tool")])
                .unwrap(),
            LlmMessage::tool("c1".to_string(), "schema...".to_string()).unwrap(),
            LlmMessage::assistant_with_tool_calls(String::new(), vec![tc("c2", "sql_query")])
                .unwrap(),
            LlmMessage::tool("c2".to_string(), "rows...".to_string()).unwrap(),
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
        let big = "x".repeat(400);
        let msgs: Vec<LlmMessage> = (0..6)
            .map(|_| LlmMessage::user(big.clone()).unwrap())
            .collect();
        let classes = vec![ValueClass::Content; 6];
        let b = recent_boundary_by_tokens(&msgs, &classes, 250);
        assert!(b >= 3 && b <= 4, "boundary fue {b}");
    }

    #[test]
    fn rendered_size_includes_tool_call_args() {
        let m = LlmMessage::assistant_with_tool_calls(
            String::new(),
            vec![ToolCall::new(
                "c".to_string(),
                FunctionCall {
                    name: "f".to_string(),
                    arguments: "x".repeat(300),
                },
            )],
        )
        .unwrap();
        assert!(rendered_size(&m) >= 300);
    }

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
        async fn summarize(
            &self,
            _text: &str,
            _t: usize,
        ) -> Result<String, crate::llm::domain::LlmError> {
            Ok("RESUMEN".to_string())
        }
    }

    fn ckey() -> ConversationKey {
        ConversationKey {
            session_id: SessionId("s".into()),
            agent_session_id: Some(AgentSessionId("a".into())),
            node_id: NodeIdPath("n".into()),
        }
    }

    #[tokio::test]
    async fn old_long_nl_gets_summarized_and_cached_recent_stays_full() {
        let repo = Arc::new(InMemoryConversationRepository::new());
        let k = ckey();
        let long = "y".repeat(600);
        for i in 0..10 {
            repo.add_message(&k, LlmMessage::user(format!("{long} msg{i}")).unwrap())
                .await
                .unwrap();
        }
        let stored: Vec<StoredMessage> = repo.get_with_summaries(&k).await.unwrap();
        let summarizer: Arc<dyn MessageSummarizer> = Arc::new(StubSummarizer);

        // Budget of 300 tokens (~1200 chars) forces older messages into the summary zone.
        // Each message is ~600 chars ≈ 151 tokens, so only the last 1-2 messages fit in
        // the recent window, leaving idx 2 (and others) in the old zone to be summarized.
        let out =
            build_compacted_messages(&stored, &k, repo.as_ref(), Some(&summarizer), 300).await;

        assert!(out
            .iter()
            .any(|m| m.role() == &MessageRole::System && m.content().contains("[T2]")));
        let after = repo.get_with_summaries(&k).await.unwrap();
        assert_eq!(after[2].summary.as_deref(), Some("RESUMEN"));
    }

    #[tokio::test]
    async fn short_messages_pass_verbatim_no_summary_block() {
        let repo = Arc::new(InMemoryConversationRepository::new());
        let k = ckey();
        for i in 0..3 {
            repo.add_message(&k, LlmMessage::user(format!("corto {i}")).unwrap())
                .await
                .unwrap();
        }
        let stored = repo.get_with_summaries(&k).await.unwrap();
        let out =
            build_compacted_messages(&stored, &k, repo.as_ref(), None, RECENT_TOKEN_BUDGET).await;
        assert_eq!(out.len(), 3);
    }
}
