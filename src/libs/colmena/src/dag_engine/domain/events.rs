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
    #[serde(rename = "llm_message_start")]
    LlmMessageStart { node_id: String },
    #[serde(rename = "llm_message_finish")]
    LlmMessageFinish {
        node_id: String,
        usage: Option<Value>,
    },
    /// Streaming token from an internal "thinking" LLM call (planner, critic, reactor, agent subgraphs).
    #[serde(rename = "thinking_token")]
    ThinkingToken { node_id: String, token: String },
    #[serde(rename = "graph_finish")]
    GraphFinish { output: Value },
    #[serde(rename = "error")]
    Error { message: String },
    /// Emitted at the beginning of each loop turn
    #[serde(rename = "turn_start")]
    TurnStart { turn: u32 },
    /// Emitted when a subgraph node completes. Carries `node_type: "subgraph"` so the
    /// data-stream protocol can distinguish it from a regular NodeFinish.
    #[serde(rename = "subgraph_node_finish")]
    SubgraphNodeFinish { node_id: String, output: Value },
    /// Emitted just before GraphFinish. Summarises token usage per node with model and
    /// provider names for cost/audit visibility.
    #[serde(rename = "graph_usage_summary")]
    GraphUsageSummary { entries: Vec<Value> },
}
