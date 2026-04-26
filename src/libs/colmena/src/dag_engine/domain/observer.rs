use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeEvent {
    LlmToken {
        token: String,
    },
    LlmToolCall {
        tool_id: String,
        tool_name: String,
        args_chunk: String,
    },
    LlmUsage {
        prompt_tokens: u32,
        completion_tokens: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking_tokens: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_read_tokens: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_write_tokens: Option<u32>,
    },
    LlmToolCallStart {
        tool_id: String,
        tool_name: String,
        tool_args: String,
    },
    LlmToolCallFinish {
        tool_id: String,
        success: bool,
        output: String,
    },
    /// Emitted when the load_skill synthetic tool successfully loads a skill or reference.
    /// Fires in addition to LlmToolCallStart/Finish so frontends can render a skill-specific UI.
    SkillLoaded {
        tool_id: String,
        skill_name: String,
        reference: Option<String>,
        source: String, // "builtin" | "path"
        size_bytes: usize,
    },
    LlmMessageStart,
    LlmMessageFinish(Option<crate::llm::domain::LlmUsage>),
    /// Streaming token from an internal "thinking" LLM call (planner, critic, reactor, agent subgraphs).
    /// Distinct from LlmToken so the frontend can separate thinking activity from the final response.
    ThinkingToken {
        node_id: String,
        node_type: String,
        token: String,
    },
    /// A provider reasoning block has opened (extended thinking / thinking models).
    ReasoningStart {
        id: String,
    },
    /// An incremental token within the current reasoning block.
    ReasoningDelta {
        id: String,
        token: String,
    },
    /// The current reasoning block has closed.
    ReasoningEnd {
        id: String,
    },
    /// Raw DagExecutionEvent from a child subgraph execution.
    /// Child node IDs are preserved so the parent stream can re-yield them as-is.
    SubgraphChildEvent(serde_json::Value),
}

pub trait ExecutionObserver: Send + Sync {
    fn on_event(&self, event: NodeEvent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_loaded_variant_constructible() {
        let ev = NodeEvent::SkillLoaded {
            tool_id: "call_1".to_string(),
            skill_name: "python-expert".to_string(),
            reference: None,
            source: "builtin".to_string(),
            size_bytes: 1024,
        };
        match ev {
            NodeEvent::SkillLoaded { skill_name, .. } => {
                assert_eq!(skill_name, "python-expert");
            }
            _ => panic!("expected SkillLoaded"),
        }
    }
}
