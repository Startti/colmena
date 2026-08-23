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
///
/// **Guarantees:** the result is always a valid slice start — `&messages[b..]` never
/// panics, including for an empty input. When `messages` is non-empty the result is
/// additionally a valid element index — `&messages[b]` never panics. When even the
/// newest message alone exceeds `token_budget`, the recent window degenerates to
/// exactly that one message (`b == len - 1`).
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
    // `messages.len()` was never a legal answer: it is the initial accumulator
    // leaking when the walk breaks on the FIRST iteration (the newest message
    // alone exceeds the budget). In that case the newest message IS the recent
    // window — it is the message the model most needs verbatim.
    b.min(messages.len().saturating_sub(1))
}

/// Index where the open interaction starts: right after the last `assistant`
/// message that carried no tool calls.
///
/// The ReAct loop in `agent_service` terminates **if and only if** the assistant
/// returned no tool calls — condition at `agent_service.rs:354`, `return` at
/// `:360` for `Some(empty)`, `return` at `:677` for `None`. A persisted
/// `assistant` with no tool calls is therefore, by construction, the close of an
/// interaction, and everything after it is still in flight.
///
/// Returns `0` when no interaction has closed yet: the whole history belongs to
/// the current one.
pub fn current_interaction_start(messages: &[LlmMessage]) -> usize {
    for i in (0..messages.len()).rev() {
        let closes = matches!(messages[i].role(), MessageRole::Assistant)
            && messages[i].tool_calls().is_none_or(|tcs| tcs.is_empty());
        if closes {
            return i + 1;
        }
    }
    0
}

use crate::llm::application::tool_digest::digest_tool_result;
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
                } else if let Some(d) = matches!(msg.role(), MessageRole::Tool)
                    .then(|| digest_tool_result(msg.content()))
                    .flatten()
                {
                    // Structured tool result → deterministic digest. No LLM, no cache;
                    // the full result is recoverable verbatim via recall_history (lossless).
                    format!(
                        "[T{idx}] {}: {d} · recall_history(turn={idx}) para el detalle",
                        role_tag(msg)
                    )
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
        "Cada línea es un RESUMEN de un mensaje anterior, NO el contenido completo. \
         El [Tn] es el índice de turno. Para CUALQUIER valor exacto, campo, cita \
         textual o dato que no aparezca literalmente en estas líneas, DEBÉS llamar \
         recall_history(turn=N) para leer el original — nunca lo inventes ni lo \
         adivines.\n\n",
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

    /// Message-count/shape/size/budget space covered by
    /// `recent_boundary_is_always_a_valid_index` below.
    #[derive(Debug, Clone, Copy)]
    enum Shape {
        AllContent,
        AllScaffolding,
        Alternating,
        OnlyLastOversized,
        OnlyFirstOversized,
    }

    impl Shape {
        const ALL: [Shape; 5] = [
            Shape::AllContent,
            Shape::AllScaffolding,
            Shape::Alternating,
            Shape::OnlyLastOversized,
            Shape::OnlyFirstOversized,
        ];
    }

    /// Builds `n` user messages of `size` chars each, plus a `ValueClass` vector
    /// per `shape`. Message content itself is uniform; only the CLASS vector
    /// varies, which is exactly what `recent_boundary_by_tokens` reads.
    fn build(n: usize, size: usize, shape: Shape) -> (Vec<LlmMessage>, Vec<ValueClass>) {
        // `LlmMessage::new` rejects empty content, so even the "size=0" case needs
        // at least one char — that still exercises the near-zero boundary case.
        let small = "x".to_string();
        let big = "x".repeat(size.max(1));
        let msgs: Vec<LlmMessage> = (0..n)
            .map(|i| {
                let content = match shape {
                    Shape::OnlyLastOversized if i + 1 == n => big.clone(),
                    Shape::OnlyFirstOversized if i == 0 => big.clone(),
                    Shape::OnlyLastOversized | Shape::OnlyFirstOversized => small.clone(),
                    _ => big.clone(),
                };
                LlmMessage::user(content).unwrap()
            })
            .collect();
        let classes: Vec<ValueClass> = (0..n)
            .map(|i| match shape {
                Shape::AllContent => ValueClass::Content,
                Shape::AllScaffolding => ValueClass::Scaffolding,
                Shape::Alternating => {
                    if i % 2 == 0 {
                        ValueClass::Content
                    } else {
                        ValueClass::Scaffolding
                    }
                }
                Shape::OnlyLastOversized | Shape::OnlyFirstOversized => ValueClass::Content,
            })
            .collect();
        (msgs, classes)
    }

    #[test]
    fn recent_boundary_is_always_a_valid_index() {
        for n in [0, 1, 2, 3, 4, 8] {
            for size in [0, 1, 240, 1_000, 12_000, 40_000] {
                for budget in [0, 1, 250, RECENT_TOKEN_BUDGET] {
                    for shape in Shape::ALL {
                        let (msgs, classes) = build(n, size, shape);
                        let b = recent_boundary_by_tokens(&msgs, &classes, budget);
                        // Slice-safe. The slicing IS the assertion: pre-fix this panicked
                        // whenever the walk broke on its first iteration and `b` leaked
                        // `messages.len()`.
                        let _ = &msgs[b..];
                        assert!(
                            b <= msgs.len(),
                            "boundary {b} > len {} (n={n}/size={size}/budget={budget}/shape={shape:?})",
                            msgs.len()
                        );
                        if !msgs.is_empty() {
                            // Index-safe: `&msgs[b]` must never panic on a non-empty slice.
                            // Same story — the indexing itself is what would blow up.
                            let _ = &msgs[b];
                            assert!(
                                b < msgs.len(),
                                "boundary {b} not < len {} (n={n}/size={size}/budget={budget}/shape={shape:?})",
                                msgs.len()
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn recent_boundary_counts_only_content_tokens() {
        let big = "x".repeat(400);
        let msgs: Vec<LlmMessage> = (0..6)
            .map(|_| LlmMessage::user(big.clone()).unwrap())
            .collect();
        let classes = vec![ValueClass::Content; 6];
        let b = recent_boundary_by_tokens(&msgs, &classes, 250);
        assert!((3..=4).contains(&b), "boundary fue {b}");
    }

    /// The cost of keeping an oversized newest message verbatim is bounded to a
    /// SINGLE turn. As soon as any message is appended after it, the backward walk
    /// breaks on the oversized message instead of on the newest one, so the
    /// boundary lands past it and it leaves the recent window on its own — no
    /// special-casing required. This is the property the role rule is priced on.
    #[test]
    fn oversized_message_leaves_the_recent_window_on_the_next_turn() {
        let budget = 250;
        let oversized = LlmMessage::user("x".repeat(budget * 4 * 2)).unwrap();
        let small = || LlmMessage::user("ok".to_string()).unwrap();

        // Turn N: the oversized message IS the newest — it is the recent window.
        let msgs = vec![small(), small(), small(), oversized.clone()];
        let classes = vec![ValueClass::Content; msgs.len()];
        let b_now = recent_boundary_by_tokens(&msgs, &classes, budget);
        assert_eq!(
            b_now,
            msgs.len() - 1,
            "the oversized newest message must be the whole recent window"
        );

        // Turn N+1: one more message arrives. The oversized message is no longer
        // newest, so it falls out of the recent window without any extra rule.
        let msgs_next = vec![small(), small(), small(), oversized, small()];
        let classes_next = vec![ValueClass::Content; msgs_next.len()];
        let b_next = recent_boundary_by_tokens(&msgs_next, &classes_next, budget);
        assert_eq!(
            b_next, 4,
            "the oversized message must be excluded from the recent window on the next turn"
        );
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

    struct FailSummarizer;
    #[async_trait]
    impl MessageSummarizer for FailSummarizer {
        async fn summarize(
            &self,
            _text: &str,
            _t: usize,
        ) -> Result<String, crate::llm::domain::LlmError> {
            panic!("summarizer must NOT be called for a structured tool result");
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

    #[tokio::test]
    async fn repro_adp_panic_last_content_message_alone_exceeds_budget() {
        // idx 0,1 = keep_first; idx 2 = short; idx 3 = a single content message that
        // alone blows the whole recent budget (a large tool output on resume). This
        // is the EXACT shape ADP reported ("the len is 4 but the index is 4"): total=4,
        // keep_first=2, newest message alone exceeds the budget. Before the clamp in
        // `recent_boundary_by_tokens`, this shape panicked with an index-out-of-bounds
        // slice access. This is a PANIC regression test: it must not merely avoid
        // crashing — the newest Tool message must survive intact and verbatim, since
        // this shape lands on the pair-guard early return
        // (`if b <= keep_first { return messages; }`), which returns the raw vector
        // unmodified.
        let repo = Arc::new(InMemoryConversationRepository::new());
        let k = ckey();
        for _ in 0..3 {
            repo.add_message(&k, LlmMessage::user("x".into()).unwrap())
                .await
                .unwrap();
        }
        let big = "z".repeat(40_000);
        repo.add_message(&k, LlmMessage::tool("call_1".into(), big.clone()).unwrap())
            .await
            .unwrap();

        let stored = repo.get_with_summaries(&k).await.unwrap();
        assert_eq!(stored.len(), 4);

        let out =
            build_compacted_messages(&stored, &k, repo.as_ref(), None, RECENT_TOKEN_BUDGET).await;
        assert!(!out.is_empty());

        let last = out.last().expect("non-empty output");
        assert_eq!(
            last.role(),
            &MessageRole::Tool,
            "newest Tool message must stay last: {last:?}"
        );
        assert_eq!(
            last.tool_call_id(),
            Some("call_1"),
            "tool_call_id must be preserved verbatim"
        );
        assert_eq!(
            last.content(),
            big,
            "newest tool result must survive intact and verbatim — this is the exact ADP panic shape"
        );
    }

    #[tokio::test]
    async fn repro_panic_also_fires_on_a_large_user_prompt() {
        let repo = Arc::new(InMemoryConversationRepository::new());
        let k = ckey();
        for _ in 0..3 {
            repo.add_message(&k, LlmMessage::user("x".into()).unwrap())
                .await
                .unwrap();
        }
        // Not resume-specific: a single pasted user message over the budget does it too.
        repo.add_message(&k, LlmMessage::user("z".repeat(40_000)).unwrap())
            .await
            .unwrap();

        let stored = repo.get_with_summaries(&k).await.unwrap();
        let out =
            build_compacted_messages(&stored, &k, repo.as_ref(), None, RECENT_TOKEN_BUDGET).await;
        assert!(!out.is_empty());
    }

    #[tokio::test]
    async fn structured_tool_result_becomes_digest_without_calling_summarizer() {
        let repo = Arc::new(InMemoryConversationRepository::new());
        let k = ckey();

        // idx 0,1 = keep_first (short user msgs).
        repo.add_message(&k, LlmMessage::user("x".into()).unwrap())
            .await
            .unwrap();
        repo.add_message(&k, LlmMessage::user("x".into()).unwrap())
            .await
            .unwrap();
        // idx 2 = a large structured tool result (≥250 chars) → must become a digest.
        let rows: Vec<String> = (0..8)
            .map(|i| {
                format!(
                    r#"{{"region":"R{i}","revenue":{},"units":{}}}"#,
                    100_000 + i * 1000,
                    500 + i * 10
                )
            })
            .collect();
        let tool_content = format!("[{}]", rows.join(","));
        assert!(tool_content.len() >= 250);
        repo.add_message(&k, LlmMessage::tool("call_1".into(), tool_content).unwrap())
            .await
            .unwrap();
        // idx 3,4,5 = short recents.
        for _ in 0..3 {
            repo.add_message(&k, LlmMessage::user("x".into()).unwrap())
                .await
                .unwrap();
        }

        let stored = repo.get_with_summaries(&k).await.unwrap();
        let fail: Arc<dyn MessageSummarizer> = Arc::new(FailSummarizer);

        // Tiny budget pushes the tool result (idx 2) into the old zone.
        let out = build_compacted_messages(&stored, &k, repo.as_ref(), Some(&fail), 5).await;

        let summary = out
            .iter()
            .find(|m| m.role() == &MessageRole::System)
            .expect("summary block present");
        let body = summary.content();
        assert!(body.contains("[T2]"), "digest turn tag missing: {body}");
        assert!(body.contains("8 filas"), "row count missing: {body}");
        assert!(
            body.contains("cols: region, revenue, units"),
            "columns missing: {body}"
        );
        assert!(
            body.contains("recall_history(turn=2)"),
            "recall hint missing: {body}"
        );

        // The structured tool result must NOT have been persisted as a summary.
        let after = repo.get_with_summaries(&k).await.unwrap();
        assert_eq!(
            after[2].summary, None,
            "digest must not be cached in summary column"
        );
    }

    /// Fixture for `recent_window_is_never_empty`: idx0,1 = keep_first; idx2 =
    /// old-zone filler; idx3 = Assistant issuing a tool call; idx4 = the Tool
    /// result under test (the oversized newest message). Returns the index of
    /// that Tool result.
    async fn build_oversized_newest_tool_fixture(
        repo: &InMemoryConversationRepository,
        k: &ConversationKey,
        tool_content: String,
    ) -> usize {
        repo.add_message(k, LlmMessage::user("x".into()).unwrap())
            .await
            .unwrap();
        repo.add_message(k, LlmMessage::user("x".into()).unwrap())
            .await
            .unwrap();
        repo.add_message(k, LlmMessage::user("filler".into()).unwrap())
            .await
            .unwrap();
        repo.add_message(
            k,
            LlmMessage::assistant_with_tool_calls(String::new(), vec![tc("call_1", "sql_query")])
                .unwrap(),
        )
        .await
        .unwrap();
        repo.add_message(k, LlmMessage::tool("call_1".into(), tool_content).unwrap())
            .await
            .unwrap();
        4
    }

    #[tokio::test]
    async fn oversized_newest_user_prompt_stays_verbatim() {
        let repo = Arc::new(InMemoryConversationRepository::new());
        let k = ckey();
        for _ in 0..2 {
            repo.add_message(&k, LlmMessage::user("x".into()).unwrap())
                .await
                .unwrap();
        }
        repo.add_message(&k, LlmMessage::user("filler".into()).unwrap())
            .await
            .unwrap();
        let big = "z".repeat(40_000);
        repo.add_message(&k, LlmMessage::user(big.clone()).unwrap())
            .await
            .unwrap();

        let stored = repo.get_with_summaries(&k).await.unwrap();
        let out =
            build_compacted_messages(&stored, &k, repo.as_ref(), None, RECENT_TOKEN_BUDGET).await;

        let last = out.last().expect("non-empty output");
        assert_eq!(last.role(), &MessageRole::User);
        assert_eq!(last.content(), big, "oversized user prompt was mutated");
    }

    #[tokio::test]
    async fn oversized_newest_assistant_stays_verbatim() {
        let repo = Arc::new(InMemoryConversationRepository::new());
        let k = ckey();
        for _ in 0..2 {
            repo.add_message(&k, LlmMessage::user("x".into()).unwrap())
                .await
                .unwrap();
        }
        repo.add_message(&k, LlmMessage::user("filler".into()).unwrap())
            .await
            .unwrap();
        let big = "z".repeat(40_000);
        repo.add_message(&k, LlmMessage::assistant(big.clone()).unwrap())
            .await
            .unwrap();

        let stored = repo.get_with_summaries(&k).await.unwrap();
        let out =
            build_compacted_messages(&stored, &k, repo.as_ref(), None, RECENT_TOKEN_BUDGET).await;

        let last = out.last().expect("non-empty output");
        assert_eq!(last.role(), &MessageRole::Assistant);
        assert_eq!(last.content(), big, "oversized assistant reply was mutated");
    }

    #[tokio::test]
    async fn recent_window_is_never_empty() {
        // Pin: none of the oversized-newest-message shapes (Tool or User) should
        // ever let the synthesized System summary be the last message — a wire
        // that ends on the summary block means the recent window emptied out.
        let repo_tool = Arc::new(InMemoryConversationRepository::new());
        let k_tool = ckey();
        build_oversized_newest_tool_fixture(&repo_tool, &k_tool, "z".repeat(40_000)).await;

        let repo_user = Arc::new(InMemoryConversationRepository::new());
        let k_user = ckey();
        for _ in 0..2 {
            repo_user
                .add_message(&k_user, LlmMessage::user("x".into()).unwrap())
                .await
                .unwrap();
        }
        repo_user
            .add_message(&k_user, LlmMessage::user("filler".into()).unwrap())
            .await
            .unwrap();
        repo_user
            .add_message(&k_user, LlmMessage::user("z".repeat(40_000)).unwrap())
            .await
            .unwrap();

        for (name, repo, k) in [
            ("oversized tool result", &repo_tool, &k_tool),
            ("oversized user prompt", &repo_user, &k_user),
        ] {
            let stored = repo.get_with_summaries(k).await.unwrap();
            let fail: Arc<dyn MessageSummarizer> = Arc::new(FailSummarizer);
            let out = build_compacted_messages(
                &stored,
                k,
                repo.as_ref(),
                Some(&fail),
                RECENT_TOKEN_BUDGET,
            )
            .await;
            let last = out.last().expect("non-empty output");
            assert_ne!(
                last.role(),
                &MessageRole::System,
                "{name}: recent window emptied out — last message is the summary block"
            );
        }
    }

    /// The pair guard walks the boundary back over EVERY trailing `Tool` message,
    /// not just the newest one, so a `Tool` never ships without its `Assistant`.
    /// When that walk reaches `keep_first`, the early return hands back the whole
    /// history raw — no summary block at all. Pins the shape the memory guide
    /// documents: parallel tool calls whose last result is oversized.
    #[tokio::test]
    async fn parallel_tool_calls_with_oversized_last_result_ship_the_history_raw() {
        let repo = Arc::new(InMemoryConversationRepository::new());
        let k = ckey();
        let big = "z".repeat(40_000);

        for _ in 0..2 {
            repo.add_message(&k, LlmMessage::user("x".into()).unwrap())
                .await
                .unwrap();
        }
        repo.add_message(
            &k,
            LlmMessage::assistant_with_tool_calls(
                String::new(),
                vec![tc("call_a", "sql_query"), tc("call_b", "sql_query")],
            )
            .unwrap(),
        )
        .await
        .unwrap();
        repo.add_message(
            &k,
            LlmMessage::tool("call_a".into(), "small".into()).unwrap(),
        )
        .await
        .unwrap();
        repo.add_message(&k, LlmMessage::tool("call_b".into(), big.clone()).unwrap())
            .await
            .unwrap();

        let stored = repo.get_with_summaries(&k).await.unwrap();
        let out =
            build_compacted_messages(&stored, &k, repo.as_ref(), None, RECENT_TOKEN_BUDGET).await;

        assert_eq!(
            out.len(),
            stored.len(),
            "the whole history must ship raw when the pair guard reaches keep_first"
        );
        assert!(
            !out.iter().any(|m| m.role() == &MessageRole::System),
            "no summary block is synthesized on the raw-return path"
        );
        assert_eq!(
            out.last().unwrap().content(),
            big,
            "the oversized tool result stays verbatim"
        );
    }

    #[test]
    fn interaction_start_is_after_the_last_assistant_without_tool_calls() {
        let closing = LlmMessage::assistant("listo".to_string()).unwrap();
        let msgs = vec![
            LlmMessage::user("vieja".into()).unwrap(),
            closing.clone(),
            LlmMessage::user("actual".into()).unwrap(),
            LlmMessage::assistant_with_tool_calls(String::new(), vec![tc("c1", "sql_query")])
                .unwrap(),
            LlmMessage::tool("c1".into(), "filas".into()).unwrap(),
        ];
        assert_eq!(current_interaction_start(&msgs), 2);
    }

    #[test]
    fn an_assistant_with_an_empty_tool_call_vec_also_closes() {
        // The ReAct loop returns on BOTH `Some(vec![])` and `None`. Detecting
        // the close with `is_none()` alone would miss the non-streaming path.
        let msgs = vec![
            LlmMessage::user("x".into()).unwrap(),
            LlmMessage::assistant_with_tool_calls("listo".to_string(), vec![]).unwrap(),
            LlmMessage::user("actual".into()).unwrap(),
        ];
        assert_eq!(current_interaction_start(&msgs), 2);
    }

    #[test]
    fn several_unanswered_user_messages_all_belong_to_the_open_interaction() {
        let msgs = vec![
            LlmMessage::assistant("listo".to_string()).unwrap(),
            LlmMessage::user("uno".into()).unwrap(),
            LlmMessage::user("dos".into()).unwrap(),
            LlmMessage::user("tres".into()).unwrap(),
        ];
        assert_eq!(current_interaction_start(&msgs), 1);
    }

    #[test]
    fn a_closing_assistant_as_the_newest_message_leaves_nothing_open() {
        // Reachable on a resume with no new prompt: the newest stored message is
        // the previous turn's final answer. Task 2 must not let this empty the
        // recent window.
        let msgs = vec![
            LlmMessage::user("x".into()).unwrap(),
            LlmMessage::assistant("listo".to_string()).unwrap(),
        ];
        assert_eq!(current_interaction_start(&msgs), msgs.len());
    }

    #[test]
    fn without_a_closed_interaction_everything_is_current() {
        let msgs = vec![
            LlmMessage::user("x".into()).unwrap(),
            LlmMessage::assistant_with_tool_calls(String::new(), vec![tc("c1", "sql_query")])
                .unwrap(),
            LlmMessage::tool("c1".into(), "filas".into()).unwrap(),
        ];
        assert_eq!(current_interaction_start(&msgs), 0);
        assert_eq!(current_interaction_start(&[]), 0);
    }

    #[test]
    fn interaction_start_uses_the_last_close_not_the_first() {
        // Regression guard: the scan MUST use `.rev()` to find the LAST closing
        // assistant, not the first. If the scan ran forward, this test would fail.
        // Two turns ago, the agent finished (closing assistant at idx 1).
        // One turn ago, it finished again (closing assistant at idx 3).
        // Right now, the user has a new question (idx 4) — the boundary is after idx 3.
        let msgs = vec![
            LlmMessage::user("viejo_turno_1".into()).unwrap(), // idx 0
            LlmMessage::assistant("listo_turno_1".to_string()).unwrap(), // idx 1 — closes
            LlmMessage::user("viejo_turno_2".into()).unwrap(), // idx 2
            LlmMessage::assistant("listo_turno_2".to_string()).unwrap(), // idx 3 — closes
            LlmMessage::user("pregunta_nueva".into()).unwrap(), // idx 4 — open interaction
        ];
        assert_eq!(
            current_interaction_start(&msgs),
            4,
            "must find the LAST closing assistant (idx 3), not the first (idx 1)"
        );
    }
}
