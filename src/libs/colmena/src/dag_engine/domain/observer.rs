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
    /// Emitted when the synthetic `describe_tool` successfully reveals a tool's schema.
    /// Fires alongside LlmToolCallStart/Finish so frontends can render a discovery-specific UI.
    ToolDescribed {
        tool_id: String,
        tool_name: String,
    },
    /// Coarse batch progress emitted by `for_each` at start, per item, and end.
    BatchProgress {
        node_id: String,
        total: usize,
        completed: usize,
        ok: usize,
        err: usize,
        in_flight: usize,
    },
    /// Emitted by `for_each` the moment a single item finishes.
    BatchItemFinished {
        node_id: String,
        index: usize,
        key: String,
        status: String, // "ok" | "err"
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

/// Re-parents everything a node emits one nesting level below, under `node_id`.
///
/// A node that runs its own inner execution (an `llm_call` dispatched as a tool,
/// a `subgraph` reached through a synthetic boundary) shares its caller's
/// observer. The graph loop then stamps the CALLER's node id onto those events,
/// so the inner work surfaces at the caller's level — attributed to the caller,
/// or as a sibling of the very boundary frame that was supposed to contain it.
///
/// Wrapping the caller's observer in this adapter inserts the missing hop:
/// events come out one level deeper, with `node_id` in their lineage path.
///
/// Do NOT apply it where the enclosing loop already prepends the same id — the
/// segment would be duplicated and every run would gain a phantom level.
pub struct ChildScopeObserver {
    inner: std::sync::Arc<dyn ExecutionObserver>,
    node_id: String,
}

impl ChildScopeObserver {
    pub fn new(inner: std::sync::Arc<dyn ExecutionObserver>, node_id: impl Into<String>) -> Self {
        Self {
            inner,
            node_id: node_id.into(),
        }
    }

    /// Wrap `inner` and hand back a ready-to-pass observer handle.
    pub fn wrap(
        inner: std::sync::Arc<dyn ExecutionObserver>,
        node_id: impl Into<String>,
    ) -> std::sync::Arc<dyn ExecutionObserver> {
        std::sync::Arc::new(Self::new(inner, node_id)) as std::sync::Arc<dyn ExecutionObserver>
    }
}

impl ExecutionObserver for ChildScopeObserver {
    fn on_event(&self, event: NodeEvent) {
        use crate::dag_engine::domain::events::DagExecutionEvent;

        let lifted = match event {
            // Already a stream event from below this node. Re-parent it under us
            // so the lineage keeps every hop instead of skipping this one.
            NodeEvent::SubgraphChildEvent(raw) => serde_json::from_value::<DagExecutionEvent>(raw)
                .ok()
                .map(|ev| ev.wrap_as_child_of(&self.node_id)),
            other => DagExecutionEvent::from_node_event(other, &self.node_id).map(|ev| {
                // Most events carry no node identity of their own, so
                // `from_node_event` stamped THIS scope onto them. Left as base
                // events, the enclosing loop builds their path from that id and
                // the result is `parent>scope` — correct.
                //
                // But a few variants arrive with their own identity already set
                // (`BatchProgress`, `BatchItemFinished`, `ThinkingToken`). For
                // those the stamp is a no-op, so leaving them as base events made
                // the loop build `parent>ownId` and this scope's segment vanished
                // — a `for_each` dispatched as a tool reported its batch progress
                // outside the very tool node its rows were nested under. Wrap
                // those explicitly so the hop survives as `parent>scope>ownId`.
                if ev.node_id() == Some(self.node_id.as_str()) {
                    ev
                } else {
                    ev.wrap_as_child_of(&self.node_id)
                }
            }),
        };

        if let Some(ev) = lifted {
            if let Ok(raw) = serde_json::to_value(&ev) {
                self.inner.on_event(NodeEvent::SubgraphChildEvent(raw));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_described_variant_constructible() {
        let ev = NodeEvent::ToolDescribed {
            tool_id: "call_1".to_string(),
            tool_name: "search_orders".to_string(),
        };
        match ev {
            NodeEvent::ToolDescribed { tool_name, .. } => {
                assert_eq!(tool_name, "search_orders");
            }
            _ => panic!("expected ToolDescribed"),
        }
    }

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

    // ── ChildScopeObserver ──────────────────────────────────────────────────

    use crate::dag_engine::domain::events::DagExecutionEvent;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Capturing {
        events: Mutex<Vec<NodeEvent>>,
    }
    impl ExecutionObserver for Capturing {
        fn on_event(&self, event: NodeEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    fn only_captured(sink: &Arc<Capturing>) -> DagExecutionEvent {
        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1, "expected exactly one forwarded event");
        match &events[0] {
            NodeEvent::SubgraphChildEvent(raw) => serde_json::from_value(raw.clone())
                .expect("forwarded payload must be a DagExecutionEvent"),
            other => panic!("expected SubgraphChildEvent, got {other:?}"),
        }
    }

    #[test]
    fn scopes_a_plain_node_event_one_level_below() {
        let sink = Arc::new(Capturing::default());
        let scoped = ChildScopeObserver::new(sink.clone(), "nested_agent");

        scoped.on_event(NodeEvent::LlmToken {
            token: "hello".into(),
        });

        match only_captured(&sink) {
            DagExecutionEvent::LlmToken { node_id, token } => {
                assert_eq!(
                    node_id, "nested_agent",
                    "a nested agent's tokens must be attributed to it, not to its caller"
                );
                assert_eq!(token, "hello");
            }
            other => panic!("expected LlmToken, got {other:?}"),
        }
    }

    #[test]
    fn re_parents_an_event_that_already_came_from_below() {
        let sink = Arc::new(Capturing::default());
        let scoped = ChildScopeObserver::new(sink.clone(), "nested_agent");

        let from_below = DagExecutionEvent::SubgraphWrapped {
            inner: Box::new(DagExecutionEvent::LlmToken {
                node_id: "leaf".into(),
                token: "t".into(),
            }),
            depth: 1,
            path: "leaf".into(),
        };
        scoped.on_event(NodeEvent::SubgraphChildEvent(
            serde_json::to_value(&from_below).unwrap(),
        ));

        match only_captured(&sink) {
            DagExecutionEvent::SubgraphWrapped { depth, path, .. } => {
                assert_eq!(depth, 2, "the hop through this node must count");
                assert_eq!(path, "nested_agent>leaf");
            }
            other => panic!("expected SubgraphWrapped, got {other:?}"),
        }
    }

    /// `BatchProgress` arrives with its own `node_id`, so the scope stamp is a
    /// no-op on it. Without an explicit wrap the enclosing loop would rebuild the
    /// path from that intrinsic id and drop this scope's segment entirely.
    #[test]
    fn events_with_their_own_identity_still_keep_the_scope_segment() {
        let sink = Arc::new(Capturing::default());
        let scoped = ChildScopeObserver::new(sink.clone(), "abanico");

        scoped.on_event(NodeEvent::BatchProgress {
            node_id: "for_each".into(),
            total: 2,
            completed: 0,
            ok: 0,
            err: 0,
            in_flight: 0,
        });

        match only_captured(&sink) {
            DagExecutionEvent::SubgraphWrapped { depth, path, .. } => {
                assert_eq!(depth, 1);
                assert_eq!(
                    path, "abanico>for_each",
                    "the scope must survive for events that carry their own node id"
                );
            }
            other => panic!("expected the event to be wrapped, got {other:?}"),
        }
    }

    #[test]
    fn drops_undeserializable_child_payloads_instead_of_forwarding_garbage() {
        let sink = Arc::new(Capturing::default());
        let scoped = ChildScopeObserver::new(sink.clone(), "nested_agent");
        scoped.on_event(NodeEvent::SubgraphChildEvent(serde_json::json!({
            "event": "definitely_not_a_variant"
        })));
        assert!(sink.events.lock().unwrap().is_empty());
    }
}
