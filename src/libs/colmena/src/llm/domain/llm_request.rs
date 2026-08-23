use crate::llm::domain::llm_message::FileData;
use crate::llm::domain::{
    LlmConfig, LlmError, LlmMessage, LlmRequestId, MessageRole, ToolCall, ToolDefinition,
};
use serde::{Deserialize, Serialize};

/// Merge adjacent messages that share the same role — EXCEPT `Tool` and
/// `System`, where consecutive entries are legitimate.
///
/// Providers require strictly alternating user/assistant turns; a turn that
/// fails after persisting the user message leaves a dangling `user` row that
/// would otherwise make every later turn fail at `LlmRequest::new`. Coalescing
/// normalizes the wire shape and self-heals such conversations. Pure; never
/// touches persistence (recall_history keeps the originals verbatim).
///
/// The two exemptions:
///
/// * `Tool` — parallel tool results are keyed by distinct `tool_call_id`, so
///   merging them would destroy the pairing with their `tool_use` blocks.
/// * `System` — System messages are never part of the user/assistant
///   alternation any provider enforces: Anthropic hoists them into `system[]`
///   content blocks, Gemini joins them into `system_instruction`, and OpenAI
///   accepts consecutive `system` entries as-is. Merging them WAS actively
///   harmful: `history_compaction` emits the volatile conversation summary as
///   its own System message precisely so it stays out of the Anthropic
///   prompt-cache breakpoint, which sits on the FIRST system block. Coalescing
///   folded that summary back into the agent's stable prompt and moved the
///   cached prefix on every turn — measured as a full cache re-write per turn
///   (3029 → 4684 tokens over five turns) with zero cache reads.
pub fn coalesce_consecutive_same_role(messages: Vec<LlmMessage>) -> Vec<LlmMessage> {
    let mut out: Vec<LlmMessage> = Vec::with_capacity(messages.len());
    for msg in messages {
        let mergeable = matches!(out.last(), Some(last)
            if last.role() == msg.role()
                && *msg.role() != MessageRole::Tool
                && *msg.role() != MessageRole::System);
        if mergeable {
            let prev = out.pop().expect("checked non-empty");
            out.push(merge_same_role(prev, msg));
        } else {
            out.push(msg);
        }
    }
    out
}

/// Merge two same-role messages: join non-empty contents, concat tool_calls and
/// files. Construction is infallible for valid inputs (the only `new` failure is
/// empty content for a non-assistant role, and the joined content of two valid
/// non-assistant messages is non-empty).
fn merge_same_role(a: LlmMessage, b: LlmMessage) -> LlmMessage {
    let role = a.role().clone();
    let content = [a.content(), b.content()]
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut tool_calls: Vec<ToolCall> = Vec::new();
    if let Some(tc) = a.tool_calls() {
        tool_calls.extend_from_slice(tc);
    }
    if let Some(tc) = b.tool_calls() {
        tool_calls.extend_from_slice(tc);
    }

    let mut files: Vec<FileData> = Vec::new();
    if let Some(f) = a.files() {
        files.extend_from_slice(f);
    }
    if let Some(f) = b.files() {
        files.extend_from_slice(f);
    }

    // assistant carries tool_calls; user carries files. A same-role merge of
    // mixed assistant(tool_calls)+assistant(files) can't occur in practice
    // (files are a user concept); recall_history keeps originals regardless.
    let built = if role == MessageRole::Assistant && !tool_calls.is_empty() {
        LlmMessage::assistant_with_tool_calls(content, tool_calls)
    } else if role == MessageRole::User && !files.is_empty() {
        LlmMessage::user_with_files(content, files)
    } else {
        LlmMessage::new(role.clone(), content)
    };

    built.unwrap_or_else(|_| {
        LlmMessage::new(role, " ".to_string()).unwrap_or_else(|_| {
            LlmMessage::assistant(String::new()).expect("assistant allows empty")
        })
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    id: LlmRequestId,
    messages: Vec<LlmMessage>,
    config: LlmConfig,
    stream: bool,

    /// Optional tools available for this request
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolDefinition>>,

    /// Control how the model uses tools ("auto", "none", or specific function name)
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

impl LlmRequest {
    pub fn new(
        messages: Vec<LlmMessage>,
        config: LlmConfig,
        stream: bool,
    ) -> Result<Self, LlmError> {
        // Normalize the wire shape: providers require alternating roles. Merge
        // any adjacent same-role messages (e.g. a dangling user left by a failed
        // turn) so a poisoned conversation self-heals instead of erroring here.
        // Persistence is untouched — recall_history keeps the originals.
        let messages = coalesce_consecutive_same_role(messages);

        if messages.is_empty() {
            return Err(LlmError::EmptyMessages);
        }

        // Defensive: after coalescing only consecutive Tool and System messages
        // can remain — both are deliberate exemptions above.
        for i in 1..messages.len() {
            let prev_msg = &messages[i - 1];
            let current_msg = &messages[i];

            if prev_msg.role() == current_msg.role() {
                // Allow consecutive Tool messages (parallel tool calls) and
                // consecutive System messages (stable prompt + volatile
                // compaction summary, kept apart for prompt caching).
                if matches!(
                    current_msg.role(),
                    crate::llm::domain::MessageRole::Tool | crate::llm::domain::MessageRole::System
                ) {
                    continue;
                }

                return Err(LlmError::ConsecutiveRoles {
                    role: current_msg.role().to_string(),
                    index1: i - 1,
                    index2: i,
                });
            }
        }

        Ok(Self {
            id: LlmRequestId::new(),
            messages,
            config,
            stream,
            tools: None,
            tool_choice: None,
        })
    }

    pub fn with_id(mut self, id: LlmRequestId) -> Self {
        self.id = id;
        self
    }

    /// Add tools to the request
    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Set how the model should use tools
    pub fn with_tool_choice(mut self, choice: String) -> Self {
        self.tool_choice = Some(choice);
        self
    }

    // Getters
    pub fn id(&self) -> &LlmRequestId {
        &self.id
    }

    pub fn messages(&self) -> &[LlmMessage] {
        &self.messages
    }

    pub fn config(&self) -> &LlmConfig {
        &self.config
    }

    pub fn stream(&self) -> bool {
        self.stream
    }

    // Convenience methods
    pub fn is_streaming(&self) -> bool {
        self.stream
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    pub fn last_message(&self) -> Option<&LlmMessage> {
        self.messages.last()
    }

    pub fn first_message(&self) -> Option<&LlmMessage> {
        self.messages.first()
    }

    /// Get the tools available for this request
    pub fn tools(&self) -> Option<&[ToolDefinition]> {
        self.tools.as_deref()
    }

    /// Get the tool choice setting
    pub fn tool_choice(&self) -> Option<&str> {
        self.tool_choice.as_deref()
    }

    /// Check if tools are available
    pub fn has_tools(&self) -> bool {
        self.tools.as_ref().map(|t| !t.is_empty()).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::tools::FunctionCall;
    use crate::llm::domain::{LlmConfig, LlmProvider, MessageRole, ProviderKind};

    // Helper para crear una configuración de prueba
    fn create_test_config() -> LlmConfig {
        let provider = LlmProvider::new(
            ProviderKind::Google,
            "test_api_key".to_string(),
            Some("gemini-pro".to_string()),
        )
        .unwrap();
        LlmConfig::new(provider)
    }

    // Helper para crear mensajes de prueba
    fn create_test_messages() -> Vec<LlmMessage> {
        vec![LlmMessage::new(MessageRole::User, "Hello".to_string()).unwrap()]
    }

    #[test]
    fn test_request_creation_success() {
        let config = create_test_config();
        let messages = create_test_messages();
        let request = LlmRequest::new(messages, config, true).unwrap();

        assert!(!request.id().value().to_string().is_empty());
        assert_eq!(request.message_count(), 1);
        assert_eq!(request.config().provider().kind(), &ProviderKind::Google);
        assert!(request.is_streaming());
    }

    #[test]
    fn test_request_creation_fails_on_empty_messages() {
        let config = create_test_config();
        let messages: Vec<LlmMessage> = vec![];
        let result = LlmRequest::new(messages, config, false);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), LlmError::EmptyMessages);
    }

    #[test]
    fn test_getters_return_correct_values() {
        let config = create_test_config();
        let messages = create_test_messages();
        let request = LlmRequest::new(messages.clone(), config.clone(), false).unwrap();

        assert_eq!(request.messages(), &messages[..]);
        assert_eq!(
            request.config().provider().api_key(),
            config.provider().api_key()
        );
        assert!(!request.stream());
        assert_eq!(request.last_message(), messages.last());
    }

    #[test]
    fn consecutive_user_messages_are_coalesced_into_one() {
        let config = create_test_config();
        let messages = vec![
            LlmMessage::new(MessageRole::User, "Hello".to_string()).unwrap(),
            LlmMessage::new(MessageRole::User, "How are you?".to_string()).unwrap(),
        ];
        let request = LlmRequest::new(messages, config, false).unwrap();
        assert_eq!(request.message_count(), 1);
        assert_eq!(request.messages()[0].content(), "Hello\n\nHow are you?");
    }

    #[test]
    fn poisoned_history_with_dangling_user_self_heals() {
        let config = create_test_config();
        let messages = vec![
            LlmMessage::new(MessageRole::User, "q1".to_string()).unwrap(),
            LlmMessage::new(MessageRole::Assistant, "a1".to_string()).unwrap(),
            LlmMessage::new(MessageRole::User, "dangling".to_string()).unwrap(),
            LlmMessage::new(MessageRole::User, "nueva".to_string()).unwrap(),
        ];
        let request = LlmRequest::new(messages, config, false).unwrap();
        assert_eq!(request.message_count(), 3);
        assert_eq!(request.messages()[2].content(), "dangling\n\nnueva");
    }

    #[test]
    fn test_request_creation_succeeds_with_interspersed_system_messages() {
        let config = create_test_config();
        let messages = vec![
            LlmMessage::new(MessageRole::User, "Hello".to_string()).unwrap(),
            LlmMessage::new(MessageRole::System, "You are a bot.".to_string()).unwrap(),
            LlmMessage::new(MessageRole::User, "How are you?".to_string()).unwrap(),
        ];
        // This should not fail because the consecutive check ignores system messages
        let result = LlmRequest::new(messages, config, false);
        assert!(result.is_ok());
    }

    #[test]
    fn coalesces_two_consecutive_user_messages() {
        let msgs = vec![
            LlmMessage::user("primera pregunta".into()).unwrap(),
            LlmMessage::user("segunda pregunta".into()).unwrap(),
        ];
        let out = coalesce_consecutive_same_role(msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role(), &MessageRole::User);
        assert_eq!(out[0].content(), "primera pregunta\n\nsegunda pregunta");
    }

    #[test]
    fn coalesces_three_plus_consecutive_and_leaves_alternating_intact() {
        let msgs = vec![
            LlmMessage::user("u1".into()).unwrap(),
            LlmMessage::assistant("a1".into()).unwrap(),
            LlmMessage::user("u2".into()).unwrap(),
            LlmMessage::user("u3".into()).unwrap(),
            LlmMessage::user("u4".into()).unwrap(),
        ];
        let out = coalesce_consecutive_same_role(msgs);
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].content(), "u2\n\nu3\n\nu4");
    }

    #[test]
    fn does_not_coalesce_consecutive_tool_messages() {
        let msgs = vec![
            LlmMessage::tool("call_a".into(), "result a".into()).unwrap(),
            LlmMessage::tool("call_b".into(), "result b".into()).unwrap(),
        ];
        let out = coalesce_consecutive_same_role(msgs);
        assert_eq!(out.len(), 2, "parallel tool results must stay separate");
    }

    #[test]
    fn merges_assistant_tool_calls_when_coalescing_assistants() {
        let tc = |id: &str| {
            ToolCall::new(
                id.to_string(),
                FunctionCall {
                    name: "f".into(),
                    arguments: "{}".into(),
                },
            )
        };
        let msgs = vec![
            LlmMessage::assistant_with_tool_calls("".into(), vec![tc("c1")]).unwrap(),
            LlmMessage::assistant_with_tool_calls("texto".into(), vec![tc("c2")]).unwrap(),
        ];
        let out = coalesce_consecutive_same_role(msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tool_calls().map(|t| t.len()), Some(2));
        assert_eq!(out[0].content(), "texto");
    }

    #[test]
    fn empty_and_singleton_are_passthrough() {
        assert!(coalesce_consecutive_same_role(vec![]).is_empty());
        let one = vec![LlmMessage::user("hola".into()).unwrap()];
        assert_eq!(coalesce_consecutive_same_role(one).len(), 1);
    }

    #[test]
    fn merges_user_files_when_coalescing_users() {
        // Build two user-with-files messages and confirm files concatenate.
        let f = |name: &str| {
            FileData::inline("image/png".to_string(), name.to_string(), b"data".to_vec())
        };
        let msgs = vec![
            LlmMessage::user_with_files("uno".into(), vec![f("a.png")]).unwrap(),
            LlmMessage::user_with_files("dos".into(), vec![f("b.png")]).unwrap(),
        ];
        let out = coalesce_consecutive_same_role(msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].files().map(|f| f.len()), Some(2));
        assert_eq!(out[0].content(), "uno\n\ndos");
    }

    #[test]
    fn consecutive_tool_messages_pass_through_new_unmerged() {
        let config = create_test_config();
        let messages = vec![
            LlmMessage::new(MessageRole::User, "do two things".to_string()).unwrap(),
            LlmMessage::tool("call_a".to_string(), "result a".to_string()).unwrap(),
            LlmMessage::tool("call_b".to_string(), "result b".to_string()).unwrap(),
        ];
        let request = LlmRequest::new(messages, config, false).unwrap();
        // Parallel tool results stay separate: user + tool + tool = 3.
        assert_eq!(request.message_count(), 3);
    }

    // ── System messages survive coalescing (prompt caching) ──────────────
    //
    // `history_compaction` deliberately emits the volatile conversation summary
    // as a System message of its own so it lands OUTSIDE the Anthropic cache
    // breakpoint, which sits on the first system block. Coalescing the two back
    // together moved the cached prefix on every turn.

    #[test]
    fn consecutive_system_messages_are_not_coalesced() {
        let msgs = vec![
            LlmMessage::user("hola".to_string()).unwrap(),
            LlmMessage::system("STABLE AGENT PROMPT".to_string()).unwrap(),
            LlmMessage::system("## Conversation summary (older turns)".to_string()).unwrap(),
            LlmMessage::user("y ahora?".to_string()).unwrap(),
        ];
        let out = coalesce_consecutive_same_role(msgs);

        assert_eq!(out.len(), 4, "no message may be merged away");
        assert_eq!(out[1].content(), "STABLE AGENT PROMPT");
        assert_eq!(out[2].content(), "## Conversation summary (older turns)");
    }

    #[test]
    fn a_request_with_two_system_messages_is_valid() {
        let request = LlmRequest::new(
            vec![
                LlmMessage::user("hola".to_string()).unwrap(),
                LlmMessage::system("STABLE AGENT PROMPT".to_string()).unwrap(),
                LlmMessage::system("## Conversation summary (older turns)".to_string()).unwrap(),
                LlmMessage::user("y ahora?".to_string()).unwrap(),
            ],
            create_test_config(),
            false,
        )
        .expect("consecutive System messages must not be rejected");

        let systems = request
            .messages()
            .iter()
            .filter(|m| m.role() == &MessageRole::System)
            .count();
        assert_eq!(systems, 2, "both System messages must reach the adapter");
    }

    #[test]
    fn consecutive_user_messages_still_coalesce() {
        // Regression guard: the System exemption must not weaken the
        // self-healing merge that alternating-role providers depend on.
        let out = coalesce_consecutive_same_role(vec![
            LlmMessage::user("dangling".to_string()).unwrap(),
            LlmMessage::user("nueva pregunta".to_string()).unwrap(),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content(), "dangling\n\nnueva pregunta");
    }
}
