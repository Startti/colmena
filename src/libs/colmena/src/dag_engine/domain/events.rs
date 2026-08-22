use crate::dag_engine::domain::observer::NodeEvent;
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
    /// Coarse batch progress emitted by `for_each` at start, per item, and end.
    #[serde(rename = "batch_progress")]
    BatchProgress {
        node_id: String,
        total: usize,
        completed: usize,
        ok: usize,
        err: usize,
        in_flight: usize,
    },
    /// Emitted by `for_each` the moment a single item finishes.
    #[serde(rename = "batch_item_finished")]
    BatchItemFinished {
        node_id: String,
        index: usize,
        key: String,
        status: String,
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
    ///
    /// `depth` is the nesting level of the wrapped event (1 = direct child
    /// subgraph, 2 = grandchild, …). The wrapper is kept **flat**: `inner` is
    /// always a base event, never another `SubgraphWrapped`; deeper levels are
    /// represented by incrementing `depth` instead of re-nesting (see
    /// `run_use_case.rs`). The SSE mapper still tolerates legacy nested wrappers
    /// by accumulating `depth` across layers. `path` is the human-readable
    /// lineage (`parent>…>node`) used to render the nested tree.
    #[serde(rename = "subgraph_wrapped")]
    SubgraphWrapped {
        inner: Box<DagExecutionEvent>,
        #[serde(default = "default_subgraph_depth")]
        depth: u32,
        #[serde(default)]
        path: String,
    },
}

/// Default nesting depth for a `SubgraphWrapped` event whose `depth` field is
/// absent (legacy serialized events). A wrapped event is at least one level
/// deep, so the default is `1` rather than `0`.
fn default_subgraph_depth() -> u32 {
    1
}

impl DagExecutionEvent {
    /// Lift a node-emitted [`NodeEvent`] into the stream event it corresponds to,
    /// stamping it with `node_id`.
    ///
    /// Nodes emit `NodeEvent`s that carry no node identity of their own; whoever
    /// is running the node supplies it. The graph execution loop does this
    /// inline (it also keeps per-node bookkeeping while matching, so it cannot
    /// simply call this). `DagToolExecutor` needs the same mapping for a node it
    /// dispatches as a tool, and hand-rolling a second copy there would let the
    /// two drift apart variant by variant.
    ///
    /// Returns `None` for [`NodeEvent::SubgraphChildEvent`], which is not a
    /// node's own event but an already-formed stream event from one level below.
    /// Callers decide how to re-parent it.
    pub fn from_node_event(event: NodeEvent, node_id: &str) -> Option<Self> {
        let nid = || node_id.to_string();
        Some(match event {
            NodeEvent::LlmToken { token } => Self::LlmToken {
                node_id: nid(),
                token,
            },
            NodeEvent::LlmToolCall {
                tool_id,
                tool_name,
                args_chunk,
            } => Self::LlmToolCall {
                node_id: nid(),
                tool_id,
                tool_name,
                args_chunk,
            },
            NodeEvent::LlmUsage {
                prompt_tokens,
                completion_tokens,
                thinking_tokens,
                cache_read_tokens,
                cache_write_tokens,
            } => Self::LlmUsage {
                node_id: nid(),
                prompt_tokens,
                completion_tokens,
                thinking_tokens,
                cache_read_tokens,
                cache_write_tokens,
            },
            NodeEvent::LlmToolCallStart {
                tool_id,
                tool_name,
                tool_args,
            } => Self::LlmToolCallStart {
                node_id: nid(),
                tool_id,
                tool_name,
                tool_args,
            },
            NodeEvent::LlmToolCallFinish {
                tool_id,
                success,
                output,
            } => Self::LlmToolCallFinish {
                node_id: nid(),
                tool_id,
                success,
                output,
            },
            NodeEvent::SkillLoaded {
                tool_id,
                skill_name,
                reference,
                source,
                size_bytes,
            } => Self::SkillLoaded {
                node_id: nid(),
                tool_id,
                skill_name,
                reference,
                source,
                size_bytes,
            },
            NodeEvent::ToolDescribed { tool_id, tool_name } => Self::ToolDescribed {
                node_id: nid(),
                tool_id,
                tool_name,
            },
            // `for_each` stamps its own node_id on these two.
            NodeEvent::BatchProgress {
                node_id,
                total,
                completed,
                ok,
                err,
                in_flight,
            } => Self::BatchProgress {
                node_id,
                total,
                completed,
                ok,
                err,
                in_flight,
            },
            NodeEvent::BatchItemFinished {
                node_id,
                index,
                key,
                status,
            } => Self::BatchItemFinished {
                node_id,
                index,
                key,
                status,
            },
            NodeEvent::LlmMessageStart => Self::LlmMessageStart { node_id: nid() },
            NodeEvent::LlmMessageFinish(usage) => Self::LlmMessageFinish {
                node_id: nid(),
                usage: usage.map(|u| serde_json::json!(u)),
            },
            // Carries the internal sub-node's own identity (planner, critic, …).
            NodeEvent::ThinkingToken {
                node_id,
                node_type,
                token,
            } => Self::ThinkingToken {
                node_id,
                node_type,
                token,
            },
            NodeEvent::ReasoningStart { id } => Self::ReasoningStart { node_id: nid(), id },
            NodeEvent::ReasoningDelta { id, token } => Self::ReasoningDelta {
                node_id: nid(),
                id,
                token,
            },
            NodeEvent::ReasoningEnd { id } => Self::ReasoningEnd { node_id: nid(), id },
            NodeEvent::SubgraphChildEvent(_) => return None,
        })
    }

    /// Re-parent an event coming from one level below under `node_id`.
    ///
    /// Keeps the wrapper **flat**: an already-wrapped event gets its `depth`
    /// bumped and `node_id` prepended to its lineage, rather than nesting a
    /// second `SubgraphWrapped` around it. Deeply nested wrappers used to be
    /// dropped outright by the SSE mapper.
    pub fn wrap_as_child_of(self, node_id: &str) -> Self {
        match self {
            Self::SubgraphWrapped { inner, depth, path } => Self::SubgraphWrapped {
                inner,
                depth: depth + 1,
                path: if path.is_empty() {
                    node_id.to_string()
                } else {
                    format!("{node_id}>{path}")
                },
            },
            base => {
                let path = match base.node_id() {
                    Some(child) => format!("{node_id}>{child}"),
                    None => node_id.to_string(),
                };
                Self::SubgraphWrapped {
                    inner: Box::new(base),
                    depth: 1,
                    path,
                }
            }
        }
    }

    /// The `node_id` this event is scoped to, when it has one. Events that are
    /// not tied to a single node (`TurnStart`, `GraphFinish`, `GraphUsageSummary`,
    /// `Error`, `Cancelled`, `SubgraphWrapped`) return `None`. Used to build the
    /// `path` lineage when wrapping child events and to identify the active node
    /// for the liveness heartbeat.
    pub fn node_id(&self) -> Option<&str> {
        match self {
            DagExecutionEvent::NodeStart { node_id, .. }
            | DagExecutionEvent::NodeFinish { node_id, .. }
            | DagExecutionEvent::LlmToken { node_id, .. }
            | DagExecutionEvent::LlmToolCall { node_id, .. }
            | DagExecutionEvent::LlmUsage { node_id, .. }
            | DagExecutionEvent::LlmToolCallStart { node_id, .. }
            | DagExecutionEvent::LlmToolCallFinish { node_id, .. }
            | DagExecutionEvent::LlmMessageStart { node_id }
            | DagExecutionEvent::LlmMessageFinish { node_id, .. }
            | DagExecutionEvent::ThinkingToken { node_id, .. }
            | DagExecutionEvent::ReasoningStart { node_id, .. }
            | DagExecutionEvent::ReasoningDelta { node_id, .. }
            | DagExecutionEvent::ReasoningEnd { node_id, .. }
            | DagExecutionEvent::SubgraphNodeFinish { node_id, .. }
            | DagExecutionEvent::SkillLoaded { node_id, .. }
            | DagExecutionEvent::ToolDescribed { node_id, .. }
            | DagExecutionEvent::BatchProgress { node_id, .. }
            | DagExecutionEvent::BatchItemFinished { node_id, .. }
            | DagExecutionEvent::Progress { node_id, .. } => Some(node_id),
            _ => None,
        }
    }

    /// Whether this event should advance the liveness **heartbeat** clock
    /// (`last_forwarded`). Content and progress events (tokens, tool calls,
    /// reasoning, node boundaries) do; pure turn-boundary / accounting markers
    /// (`LlmUsage`, `LlmMessageStart`, `LlmMessageFinish`, `TurnStart`) and the
    /// heartbeat `Progress` itself do NOT — a stream emitting only those is
    /// effectively silent to the user and must still heartbeat. The idle-abort
    /// clock (`last_any`) is separate and advances on *every* event. For wrapped
    /// child events the decision follows the base event.
    pub fn advances_heartbeat_clock(&self) -> bool {
        match self {
            DagExecutionEvent::LlmUsage { .. }
            | DagExecutionEvent::LlmMessageStart { .. }
            | DagExecutionEvent::LlmMessageFinish { .. }
            | DagExecutionEvent::TurnStart { .. }
            | DagExecutionEvent::Progress { .. }
            | DagExecutionEvent::GraphFinish { .. } => false,
            DagExecutionEvent::SubgraphWrapped { inner, .. } => inner.advances_heartbeat_clock(),
            _ => true,
        }
    }
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
    fn batch_progress_serializes_with_event_tag() {
        let ev = DagExecutionEvent::BatchProgress {
            node_id: "fe1".into(),
            total: 10,
            completed: 3,
            ok: 2,
            err: 1,
            in_flight: 2,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], "batch_progress");
        assert_eq!(v["data"]["completed"], 3);
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
    fn heartbeat_clock_classification() {
        // Content / progress → advances the heartbeat clock.
        assert!(DagExecutionEvent::LlmToken {
            node_id: "n".into(),
            token: "x".into()
        }
        .advances_heartbeat_clock());
        assert!(DagExecutionEvent::NodeStart {
            node_id: "n".into(),
            node_type: "llm_call".into(),
            inputs: Value::Null,
            config: Value::Null
        }
        .advances_heartbeat_clock());

        // Turn boundaries / accounting → do NOT advance (heartbeat must still fire).
        assert!(!DagExecutionEvent::LlmMessageStart {
            node_id: "n".into()
        }
        .advances_heartbeat_clock());
        assert!(!DagExecutionEvent::LlmMessageFinish {
            node_id: "n".into(),
            usage: None
        }
        .advances_heartbeat_clock());
        assert!(!DagExecutionEvent::LlmUsage {
            node_id: "n".into(),
            prompt_tokens: 1,
            completion_tokens: 1,
            thinking_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None
        }
        .advances_heartbeat_clock());
        assert!(!DagExecutionEvent::TurnStart { turn: 1 }.advances_heartbeat_clock());

        // Wrapped events follow their base event.
        let wrapped_boundary = DagExecutionEvent::SubgraphWrapped {
            inner: Box::new(DagExecutionEvent::LlmMessageStart {
                node_id: "sub".into(),
            }),
            depth: 2,
            path: "a>b>sub".into(),
        };
        assert!(!wrapped_boundary.advances_heartbeat_clock());
        let wrapped_token = DagExecutionEvent::SubgraphWrapped {
            inner: Box::new(DagExecutionEvent::LlmToken {
                node_id: "sub".into(),
                token: "x".into(),
            }),
            depth: 2,
            path: "a>b>sub".into(),
        };
        assert!(wrapped_token.advances_heartbeat_clock());
    }

    #[test]
    fn subgraph_wrapped_carries_depth_and_path() {
        let ev = DagExecutionEvent::SubgraphWrapped {
            inner: Box::new(DagExecutionEvent::LlmToken {
                node_id: "calc_a".to_string(),
                token: "96".to_string(),
            }),
            depth: 3,
            path: "top>Builder>orch>calc_a".to_string(),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["event"], "subgraph_wrapped");
        assert_eq!(json["data"]["depth"], 3);
        assert_eq!(json["data"]["path"], "top>Builder>orch>calc_a");

        let back: DagExecutionEvent = serde_json::from_value(json).unwrap();
        match back {
            DagExecutionEvent::SubgraphWrapped { inner, depth, path } => {
                assert_eq!(depth, 3);
                assert_eq!(path, "top>Builder>orch>calc_a");
                assert!(matches!(*inner, DagExecutionEvent::LlmToken { .. }));
            }
            other => panic!("expected SubgraphWrapped, got {:?}", other),
        }
    }

    #[test]
    fn subgraph_wrapped_defaults_depth_to_one_when_absent() {
        // Legacy serialized form without depth/path must deserialize with
        // depth = 1 and an empty path (backward-compat).
        let json = serde_json::json!({
            "event": "subgraph_wrapped",
            "data": { "inner": { "event": "llm_token", "data": { "node_id": "n", "token": "x" } } }
        });
        let back: DagExecutionEvent = serde_json::from_value(json).unwrap();
        match back {
            DagExecutionEvent::SubgraphWrapped { depth, path, .. } => {
                assert_eq!(depth, 1, "absent depth must default to 1");
                assert_eq!(path, "", "absent path must default to empty");
            }
            other => panic!("expected SubgraphWrapped, got {:?}", other),
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

    // ── from_node_event / wrap_as_child_of ──────────────────────────────────

    #[test]
    fn from_node_event_stamps_the_supplied_node_id() {
        let ev = DagExecutionEvent::from_node_event(
            NodeEvent::LlmToken { token: "hi".into() },
            "agent_a",
        )
        .expect("LlmToken lifts");
        match ev {
            DagExecutionEvent::LlmToken { node_id, token } => {
                assert_eq!(node_id, "agent_a");
                assert_eq!(token, "hi");
            }
            other => panic!("expected LlmToken, got {other:?}"),
        }
    }

    /// ThinkingToken already knows which internal sub-node produced it; the
    /// caller's node id must not overwrite that.
    #[test]
    fn from_node_event_preserves_thinking_token_identity() {
        let ev = DagExecutionEvent::from_node_event(
            NodeEvent::ThinkingToken {
                node_id: "planner".into(),
                node_type: "planner".into(),
                token: "t".into(),
            },
            "orchestrator",
        )
        .expect("ThinkingToken lifts");
        match ev {
            DagExecutionEvent::ThinkingToken { node_id, .. } => assert_eq!(node_id, "planner"),
            other => panic!("expected ThinkingToken, got {other:?}"),
        }
    }

    #[test]
    fn from_node_event_returns_none_for_child_events() {
        assert!(DagExecutionEvent::from_node_event(
            NodeEvent::SubgraphChildEvent(serde_json::json!({})),
            "x"
        )
        .is_none());
    }

    #[test]
    fn wrap_as_child_of_wraps_a_base_event_at_depth_one() {
        let base = DagExecutionEvent::LlmToken {
            node_id: "child".into(),
            token: "t".into(),
        };
        match base.wrap_as_child_of("parent") {
            DagExecutionEvent::SubgraphWrapped { depth, path, .. } => {
                assert_eq!(depth, 1);
                assert_eq!(path, "parent>child");
            }
            other => panic!("expected SubgraphWrapped, got {other:?}"),
        }
    }

    /// Stays flat: an already-wrapped event bumps depth and extends the lineage
    /// instead of nesting a second wrapper, which the SSE mapper used to drop.
    #[test]
    fn wrap_as_child_of_flattens_an_already_wrapped_event() {
        let wrapped = DagExecutionEvent::SubgraphWrapped {
            inner: Box::new(DagExecutionEvent::LlmToken {
                node_id: "leaf".into(),
                token: "t".into(),
            }),
            depth: 2,
            path: "mid>leaf".into(),
        };
        match wrapped.wrap_as_child_of("top") {
            DagExecutionEvent::SubgraphWrapped { inner, depth, path } => {
                assert_eq!(depth, 3);
                assert_eq!(path, "top>mid>leaf");
                assert!(
                    !matches!(*inner, DagExecutionEvent::SubgraphWrapped { .. }),
                    "wrapper must stay flat"
                );
            }
            other => panic!("expected SubgraphWrapped, got {other:?}"),
        }
    }
}
