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
    ThinkingToken {
        node_id: String,
        node_type: String,
        token: String,
    },
    /// A provider reasoning block has opened for `node_id`.
    #[serde(rename = "reasoning_start")]
    ReasoningStart { node_id: String, id: String },
    /// An incremental reasoning token for `node_id`.
    #[serde(rename = "reasoning_delta")]
    ReasoningDelta {
        node_id: String,
        id: String,
        token: String,
    },
    /// The reasoning block for `node_id` has closed.
    #[serde(rename = "reasoning_end")]
    ReasoningEnd { node_id: String, id: String },
    #[serde(rename = "graph_finish")]
    GraphFinish { output: Value },
    #[serde(rename = "error")]
    Error { message: String },
    /// Terminal event emitted when a run is hard-stopped (cooperative cancellation
    /// between nodes). `partial_output` carries whatever the last completed node
    /// produced (or `null`). Consumers should treat this as a stream terminator,
    /// distinct from `error` (it was intentional, not a failure).
    #[serde(rename = "cancelled")]
    Cancelled {
        reason: Option<String>,
        partial_output: Value,
    },
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
    /// Emitted when the synthetic describe_tool reveals a tool's schema.
    /// Fires alongside llm_tool_call_start/finish so frontends can render a discovery-specific UI.
    #[serde(rename = "tool_described")]
    ToolDescribed {
        node_id: String,
        tool_id: String,
        tool_name: String,
    },
    /// Liveness heartbeat emitted by the execution loop when the in-flight
    /// node has produced no real events for the configured heartbeat
    /// interval. `idle_secs` is the silence elapsed so far. Emitted directly
    /// by the loop (never via the observer channel), so it can never reset
    /// the idle watchdog that guards against hung nodes.
    #[serde(rename = "progress")]
    Progress { node_id: String, idle_secs: u64 },
    /// Wraps a DagExecutionEvent emitted from inside a subgraph execution.
    /// The frontend receives these with a "subgraph-" prefix on the event type.
    #[serde(rename = "subgraph_wrapped")]
    SubgraphWrapped { inner: Box<DagExecutionEvent> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_described_serializes_with_event_tag() {
        let ev = DagExecutionEvent::ToolDescribed {
            node_id: "n1".to_string(),
            tool_id: "call_1".to_string(),
            tool_name: "search_orders".to_string(),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["event"], "tool_described");
        assert_eq!(json["data"]["tool_name"], "search_orders");
    }

    #[test]
    fn cancelled_serializes_with_event_tag() {
        let ev = DagExecutionEvent::Cancelled {
            reason: Some("user requested stop".to_string()),
            partial_output: serde_json::json!({ "n1": { "output": 42 } }),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["event"], "cancelled");
        assert_eq!(json["data"]["reason"], "user requested stop");
        assert_eq!(json["data"]["partial_output"]["n1"]["output"], 42);
    }

    #[test]
    fn cancelled_roundtrips() {
        let ev = DagExecutionEvent::Cancelled {
            reason: None,
            partial_output: serde_json::Value::Null,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: DagExecutionEvent = serde_json::from_str(&json).unwrap();
        match back {
            DagExecutionEvent::Cancelled {
                reason,
                partial_output,
            } => {
                assert!(reason.is_none());
                assert!(partial_output.is_null());
            }
            other => panic!("expected Cancelled, got {:?}", other),
        }
    }

    #[test]
    fn progress_serializes_and_roundtrips() {
        let ev = DagExecutionEvent::Progress {
            node_id: "n1".to_string(),
            idle_secs: 42,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["event"], "progress");
        assert_eq!(json["data"]["node_id"], "n1");
        assert_eq!(json["data"]["idle_secs"], 42);

        let back: DagExecutionEvent = serde_json::from_value(json).unwrap();
        match back {
            DagExecutionEvent::Progress { node_id, idle_secs } => {
                assert_eq!(node_id, "n1");
                assert_eq!(idle_secs, 42);
            }
            other => panic!("expected Progress, got {:?}", other),
        }
    }
}
