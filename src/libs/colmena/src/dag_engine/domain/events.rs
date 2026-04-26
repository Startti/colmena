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
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking_tokens: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_read_tokens: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_write_tokens: Option<u32>,
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
    ThinkingToken { node_id: String, node_type: String, token: String },
    /// A provider reasoning block has opened for `node_id`.
    #[serde(rename = "reasoning_start")]
    ReasoningStart { node_id: String, id: String },
    /// An incremental reasoning token for `node_id`.
    #[serde(rename = "reasoning_delta")]
    ReasoningDelta { node_id: String, id: String, token: String },
    /// The reasoning block for `node_id` has closed.
    #[serde(rename = "reasoning_end")]
    ReasoningEnd { node_id: String, id: String },
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
    /// Emitted when the load_skill synthetic tool successfully loads a skill or reference.
    /// Fires alongside llm_tool_call_start/finish so frontends can render a skill-specific UI.
    #[serde(rename = "skill_loaded")]
    SkillLoaded {
        node_id: String,
        tool_id: String,
        skill_name: String,
        reference: Option<String>,
        source: String,
        size_bytes: usize,
    },
    /// Wraps a DagExecutionEvent emitted from inside a subgraph execution.
    /// The frontend receives these with a "subgraph-" prefix on the event type.
    #[serde(rename = "subgraph_wrapped")]
    SubgraphWrapped { inner: Box<DagExecutionEvent> },
}
