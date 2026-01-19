use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum DagExecutionEvent {
    #[serde(rename = "node_start")]
    NodeStart {
        node_id: String,
        node_type: String,
        inputs: Value,
        config: Value,
    },
    #[serde(rename = "node_finish")]
    NodeFinish { node_id: String, output: Value },
    #[serde(rename = "llm_token")]
    LlmToken { node_id: String, token: String },
    #[serde(rename = "llm_tool_call")]
    LlmToolCall {
        node_id: String,
        tool_id: String,
        tool_name: String,
        args_chunk: String,
    },
    #[serde(rename = "llm_usage")]
    LlmUsage {
        node_id: String,
        prompt_tokens: u32,
        completion_tokens: u32,
    },
    #[serde(rename = "llm_tool_call_start")]
    LlmToolCallStart {
        node_id: String,
        tool_id: String,
        tool_name: String,
        tool_args: String,
    },
    #[serde(rename = "llm_tool_call_finish")]
    LlmToolCallFinish {
        node_id: String,
        tool_id: String,
        success: bool,
        output: String,
    },
    #[serde(rename = "graph_finish")]
    GraphFinish { output: Value },
    #[serde(rename = "error")]
    Error { message: String },
}
