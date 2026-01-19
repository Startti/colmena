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
}

pub trait ExecutionObserver: Send + Sync {
    fn on_event(&self, event: NodeEvent);
}
