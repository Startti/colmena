//! Shared row → [`LlmMessage`] hydration for the conversation repositories.
//!
//! Both the Postgres and SQLite conversation repositories rebuild domain
//! messages from persisted `llm_node_history` rows. The domain constructors are
//! fallible (e.g. [`LlmMessage::tool`] rejects empty content), so a single
//! legacy or corrupt row must not be allowed to panic the whole conversation
//! load. This helper centralizes that mapping and turns a rejected row into a
//! logged skip instead of a process-killing `unwrap()`.

use crate::llm::domain::{LlmMessage, MessageRole, ToolCall};

/// Rehydrate one persisted history row into an [`LlmMessage`].
///
/// Returns `Some(message)` when the domain constructors accept the row, and
/// `None` (after a `warn!`) when they reject it — for example a non-assistant
/// row whose `content` is empty or whitespace-only. Callers use this inside a
/// `filter_map`, so a malformed row is skipped rather than panicking the load.
///
/// `tool_calls` is pre-parsed by each caller because the two backends store it
/// differently (Postgres `jsonb` vs SQLite `TEXT`).
pub(super) fn hydrate_message(
    role_str: &str,
    content: String,
    tool_call_id: Option<String>,
    tool_calls: Option<Vec<ToolCall>>,
) -> Option<LlmMessage> {
    let role = match role_str {
        "system" => MessageRole::System,
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        "tool" => MessageRole::Tool,
        _ => MessageRole::User,
    };

    let result = match role {
        MessageRole::System => LlmMessage::system(content),
        MessageRole::User => LlmMessage::user(content),
        MessageRole::Assistant => match tool_calls {
            Some(tool_calls) => LlmMessage::assistant_with_tool_calls(content, tool_calls),
            None => LlmMessage::assistant(content),
        },
        MessageRole::Tool => LlmMessage::tool(
            tool_call_id.unwrap_or_else(|| "unknown".to_string()),
            content,
        ),
    };

    match result {
        Ok(message) => Some(message),
        Err(error) => {
            tracing::warn!(
                role = role_str,
                error = %error,
                "Skipping malformed llm_node_history row during conversation hydration"
            );
            None
        }
    }
}
