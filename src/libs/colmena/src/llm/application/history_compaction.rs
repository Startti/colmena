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
}
