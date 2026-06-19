//! F-T15: `recall_history` synthetic LLM tool.
//!
//! Lets the agent re-read the FULL ORIGINAL content of a single past message
//! by its persisted turn index. Designed to compose with the rolling-summary
//! compaction in `agent_service::compact_history_to_summary`: when older
//! turns are collapsed to one-line summaries, the agent calls this tool with
//! the `[T<n>]` index from the summary to retrieve the verbatim message.
//!
//! Scope is intentionally narrow (only `turn=N`, no filter/search) to keep
//! each call bounded — see the F-T15 design note for the rationale.
//!
//! Wiring lives in `dag_tool_executor`: a `with_conversation_history(repo, key)`
//! builder sets the dependencies; the dispatch arm intercepts the tool name
//! and calls `dispatch_recall_history(repo, key, args)`.

use crate::llm::domain::tools::ToolDefinition;
use crate::llm::domain::{ConversationKey, ConversationRepository, MessageRole};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

pub const TOOL_RECALL_HISTORY: &str = "recall_history";

/// Tamaño de página por defecto (chars) cuando el caller no pasa `limit`.
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

pub fn tool_recall_history() -> ToolDefinition {
    use crate::text;
    super::build_synthetic_tool_with_summary::<RecallHistoryArgs>(
        TOOL_RECALL_HISTORY,
        text::tool_description(TOOL_RECALL_HISTORY),
        text::tool_summary(TOOL_RECALL_HISTORY),
    )
}

/// Dispatch a `recall_history` call against the persisted conversation.
/// Returns a serde_json value to surface to the LLM as a tool result.
pub async fn dispatch_recall_history(
    repository: &Arc<dyn ConversationRepository>,
    key: &ConversationKey,
    args: serde_json::Value,
) -> serde_json::Value {
    let parsed: RecallHistoryArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => {
            return serde_json::json!({ "error": format!("invalid_args: {e}") });
        }
    };

    let conv = match repository.get_by_id(key).await {
        Ok(c) => c,
        Err(e) => {
            return serde_json::json!({ "error": format!("conversation_load_failed: {e}") });
        }
    };

    let total = conv.messages.len();
    if parsed.turn >= total {
        return serde_json::json!({
            "error": "turn_out_of_range",
            "requested_turn": parsed.turn,
            "total_turns": total,
        });
    }

    let msg = &conv.messages[parsed.turn];

    let raw_content = msg.content();
    let total_chars = raw_content.chars().count();

    let start = parsed.offset.min(total_chars);
    let page = parsed
        .limit
        .unwrap_or(RECALL_PAGE_DEFAULT_CHARS)
        .clamp(1, RECALL_PAGE_MAX_CHARS);
    let end = start.saturating_add(page).min(total_chars);

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{Conversation, ConversationKey, LlmError, LlmMessage};
    use crate::{AgentSessionId, NodeIdPath, SessionId};
    use async_trait::async_trait;

    struct StubRepo {
        msgs: Vec<LlmMessage>,
    }

    #[async_trait]
    impl ConversationRepository for StubRepo {
        async fn get_by_id(&self, _key: &ConversationKey) -> Result<Conversation, LlmError> {
            Ok(Conversation {
                key: ConversationKey {
                    session_id: SessionId("s".to_string()),
                    agent_session_id: None,
                    node_id: NodeIdPath("n".to_string()),
                },
                messages: self.msgs.clone(),
            })
        }
        async fn add_message(
            &self,
            _key: &ConversationKey,
            _message: LlmMessage,
        ) -> Result<(), LlmError> {
            Ok(())
        }
        async fn delete(&self, _key: &ConversationKey) -> Result<(), LlmError> {
            Ok(())
        }
    }

    fn key() -> ConversationKey {
        ConversationKey {
            session_id: SessionId("s".to_string()),
            agent_session_id: Some(AgentSessionId("agent_test".to_string())),
            node_id: NodeIdPath("n".to_string()),
        }
    }

    #[tokio::test]
    async fn recall_returns_message_at_turn() {
        let msgs = vec![
            LlmMessage::user("hello".to_string()).unwrap(),
            LlmMessage::system("prelude".to_string()).unwrap(),
            LlmMessage::assistant("hi".to_string()).unwrap(),
        ];
        let repo: Arc<dyn ConversationRepository> = Arc::new(StubRepo { msgs });
        let k = key();
        let r = dispatch_recall_history(&repo, &k, serde_json::json!({"turn": 2})).await;
        assert_eq!(r["turn"], 2);
        assert_eq!(r["role"], "Assistant");
        assert_eq!(r["content"], "hi");
        assert!(r.get("_truncated").is_none());
    }

    #[tokio::test]
    async fn recall_out_of_range_returns_error() {
        let msgs = vec![LlmMessage::user("only one".to_string()).unwrap()];
        let repo: Arc<dyn ConversationRepository> = Arc::new(StubRepo { msgs });
        let r = dispatch_recall_history(&repo, &key(), serde_json::json!({"turn": 99})).await;
        assert_eq!(r["error"], "turn_out_of_range");
        assert_eq!(r["requested_turn"], 99);
        assert_eq!(r["total_turns"], 1);
    }

    #[tokio::test]
    async fn recall_invalid_args_returns_error() {
        let repo: Arc<dyn ConversationRepository> = Arc::new(StubRepo { msgs: vec![] });
        let r = dispatch_recall_history(&repo, &key(), serde_json::json!({"foo": 1})).await;
        assert!(r["error"]
            .as_str()
            .map(|s| s.starts_with("invalid_args"))
            .unwrap_or(false));
    }

    #[tokio::test]
    async fn recall_paginates_large_content() {
        let huge = "x".repeat(20_000);
        let msgs = vec![LlmMessage::user(huge).unwrap()];
        let repo: Arc<dyn ConversationRepository> = Arc::new(StubRepo { msgs });
        let k = key();

        let p1 = dispatch_recall_history(&repo, &k, serde_json::json!({"turn": 0})).await;
        assert_eq!(p1["total_chars"], 20_000);
        assert_eq!(p1["offset"], 0);
        assert_eq!(p1["returned_chars"], 8_192);
        assert_eq!(p1["next_offset"], 8_192);
        assert_eq!(p1["content"].as_str().unwrap().chars().count(), 8_192);
        assert!(p1.get("_truncated").is_none());

        let p2 =
            dispatch_recall_history(&repo, &k, serde_json::json!({"turn": 0, "offset": 8_192}))
                .await;
        assert_eq!(p2["offset"], 8_192);
        assert_eq!(p2["returned_chars"], 8_192);
        assert_eq!(p2["next_offset"], 16_384);

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
        let r = dispatch_recall_history(
            &repo,
            &key(),
            serde_json::json!({"turn": 0, "limit": 1_000_000}),
        )
        .await;
        assert_eq!(r["returned_chars"], 16_384);
        assert_eq!(r["next_offset"], 16_384);
    }

    #[tokio::test]
    async fn recall_includes_tool_call_metadata() {
        use crate::llm::domain::tools::{FunctionCall, ToolCall};
        let asst = LlmMessage::assistant_with_tool_calls(
            String::new(),
            vec![ToolCall::new(
                "call_42".to_string(),
                FunctionCall {
                    name: "do_thing".to_string(),
                    arguments: "{\"a\":1}".to_string(),
                },
            )],
        )
        .unwrap();
        let msgs = vec![asst];
        let repo: Arc<dyn ConversationRepository> = Arc::new(StubRepo { msgs });
        let r = dispatch_recall_history(&repo, &key(), serde_json::json!({"turn": 0})).await;
        let tcs = r["tool_calls"].as_array().expect("tool_calls present");
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["name"], "do_thing");
        assert_eq!(tcs[0]["id"], "call_42");
    }
}
