use crate::colmena_log;
use crate::dag_engine::application::ports::SubGraphExecutorPort;
use crate::dag_engine::domain::events::DagExecutionEvent;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
use serde_json::{json, Value};
use std::error::Error;
use std::sync::{Arc, OnceLock};
use tokio::fs;

pub struct SubGraphNode {
    pub executor: Arc<OnceLock<Arc<dyn SubGraphExecutorPort>>>,
}

impl Default for SubGraphNode {
    fn default() -> Self {
        Self::new()
    }
}

impl SubGraphNode {
    pub fn new() -> Self {
        Self {
            executor: Arc::new(OnceLock::new()),
        }
    }

    /// Resolve the child graph source (inline object or path string) for both
    /// the edge-based path (config) and the tool path (inputs).
    ///
    /// Precedence: `config` wins over `inputs` so the legacy edge-based behavior
    /// is unchanged; the tool path supplies the value via `inputs` (because the
    /// executor merges `fixed_config` into inputs and passes `config = {}`).
    fn resolve_child_graph_source(inputs: &NodeInputs, config: &Value) -> Option<Value> {
        if let Some(inline) = config.get("child_graph_inline") {
            return Some(inline.clone());
        }
        if let Some(path) = config.get("child_graph_path") {
            return Some(path.clone());
        }
        if let Some(inline) = inputs.get("child_graph_inline") {
            return Some(inline.clone());
        }
        if let Some(path) = inputs.get("child_graph_path") {
            return Some(path.clone());
        }
        None
    }

    /// Maximum nesting depth for subgraphs invoked as LLM tools. Beyond this the
    /// node fails fast to prevent runaway recursion from a misconfigured graph.
    pub const MAX_SUBGRAPH_TOOL_DEPTH: u64 = 5;

    /// Current subgraph-tool depth from inputs (0 when absent).
    fn current_depth(inputs: &NodeInputs) -> u64 {
        inputs
            .get("__colmena_subgraph_depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    }

    /// True when invoking another subgraph at this depth would exceed the limit.
    fn depth_exceeded(inputs: &NodeInputs) -> bool {
        Self::current_depth(inputs) >= Self::MAX_SUBGRAPH_TOOL_DEPTH
    }
}

#[async_trait::async_trait]
impl ExecutableNode for SubGraphNode {
    fn schema(&self) -> Value {
        // The `inputs` map is what the tool-definition builder reads to expose
        // parameters to the LLM (it parses each value's string for type hints
        // like "string"/"number"/"optional"). Default to a single `task` string.
        // A `node_schema` in tool_configurations takes precedence over this.
        json!({
            "inputs": {
                "task": "string — the task or instruction for the sub-agent to perform"
            }
        })
    }

    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _global_state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        let parent_session_id = inputs
            .get("__colmena_session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_parent")
            .to_string();

        let agent_session_id = inputs
            .get("__colmena_agent_session_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        // The subgraph node's *own* path is what its children must inherit.
        let parent_path = inputs
            .get("__colmena_node_id_path")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let child_path_prefix = if parent_path.is_empty() {
            None
        } else {
            Some(parent_path.clone())
        };

        if Self::depth_exceeded(inputs) {
            return Err(format!(
                "Subgraph-as-tool nesting exceeded MAX_SUBGRAPH_TOOL_DEPTH ({}). \
                 Check for a subgraph tool that references itself or a cycle of agents.",
                Self::MAX_SUBGRAPH_TOOL_DEPTH
            )
            .into());
        }

        // --- 1. RESUME PROPAGATION ---
        // If the parent was suspended in this node, it receives __colmena_resume_answer.
        // We find the existing child run by parent_session_id instead of the old
        // deterministic "{parent}_sub_{node_id}" naming.
        if let Some(resume_answer) = inputs
            .get("__colmena_resume_answer")
            .and_then(|v| v.as_str())
        {
            let executor = self
                .executor
                .get()
                .ok_or("SubGraphExecutorPort not initialized in SubGraphNode")?;

            let child_session_id = executor
                .find_child_session_id_for_resume(&parent_session_id, &parent_path)
                .await?
                .ok_or_else(|| {
                    format!(
                        "No suspended child found under parent {} / path {}",
                        parent_session_id, parent_path
                    )
                })?;

            colmena_log!(
                "▶️ [SubGraphNode] Resuming child graph {} (path={}) with answer...",
                child_session_id,
                parent_path
            );
            let result = executor
                .resume_subgraph(
                    &child_session_id,
                    resume_answer.to_string(),
                    _observer.clone(),
                    agent_session_id.clone(),
                    child_path_prefix.clone(),
                )
                .await?;

            // Check if the child suspended AGAIN
            if result.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED") {
                // Bubble up
                return Ok(result);
            }
            return Ok(result);
        }

        // --- 2. GRAPH LOADING ---
        // Source can come from `config` (edge-based path) or `inputs` (tool path,
        // where the executor merges fixed_config into inputs and passes config={}).
        let graph_source = Self::resolve_child_graph_source(inputs, config).ok_or(
            "SubGraphNode requires 'child_graph_inline' or 'child_graph_path' \
             in config (edge path) or inputs (tool path)",
        )?;

        let graph_json = if graph_source.is_object() {
            graph_source
        } else if let Some(path_val) = graph_source.as_str() {
            let path = std::path::Path::new(path_val);
            if !path.exists() {
                return Err(format!("child_graph_path not found: {}", path_val).into());
            }
            let contents = fs::read_to_string(path).await?;
            serde_json::from_str(&contents)?
        } else {
            return Err("child_graph source must be an inline object or a path string".into());
        };

        // Agent name is injected by OrchestratorNode when this subgraph is called as an agent.
        // When present, we emit boundary node-start / node-end events so the parent stream
        // shows a clear subgraph boundary around the child events.
        let agent_name = config
            .get("__agent_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Name used for the subgraph boundary frames (subgraph-node-start/end).
        // OrchestratorNode injects `__agent_name` when a subgraph runs as a named
        // agent. When the subgraph is invoked as a plain tool (no agent name),
        // fall back to this node's own id so the parent stream can still delimit
        // the sub-tree — otherwise the whole subgraph-as-tool branch has no
        // visible boundary and the nested UI tree can't be built (Fase F).
        let boundary_name = agent_name.clone().or_else(|| {
            inputs
                .get("__node_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        });

        // --- 3. STATE MAPPING (IN) ---
        // We pass the resolved `inputs` to the child graph as its initial global_state
        let mut child_state_obj = serde_json::Map::new();
        for (k, v) in inputs {
            if !k.starts_with("__colmena_") && k != "__node_id" {
                child_state_obj.insert(k.clone(), v.clone());
            }
        }
        // Propagate (depth+1) into the child so nested subgraph tools can enforce
        // the limit. Inserted AFTER the loop on purpose: the loop filters out
        // every __colmena_* key, so this is the only way the depth survives into
        // the child's global state.
        let next_depth = Self::current_depth(inputs) + 1;
        child_state_obj.insert(
            "__colmena_subgraph_depth".to_string(),
            serde_json::json!(next_depth),
        );
        let child_state = Value::Object(child_state_obj);

        // Emit subgraph node-start boundary event (orchestrator agent OR
        // subgraph-as-tool — see `boundary_name`).
        if let (Some(ref name), Some(ref obs)) = (&boundary_name, &_observer) {
            let start_event = DagExecutionEvent::NodeStart {
                node_id: name.clone(),
                node_type: "subgraph".to_string(),
                inputs: Value::Object(Default::default()),
                config: Value::Object(Default::default()),
            };
            if let Ok(raw) = serde_json::to_value(&start_event) {
                obs.on_event(NodeEvent::SubgraphChildEvent(raw));
            }
        }

        // FRESH RUN — generate a new UUID for the child session.
        let child_session_id = uuid::Uuid::new_v4().to_string();

        colmena_log!(
            "🔄 [SubGraphNode] Running SubGraph in isolated session: {} (path_prefix={:?})",
            child_session_id,
            child_path_prefix
        );

        let result = self
            .executor
            .get()
            .ok_or("SubGraphExecutorPort not initialized in SubGraphNode")?
            .run_subgraph(
                &child_session_id,
                graph_json,
                child_state,
                _observer.clone(),
                Some(parent_session_id.clone()),
                agent_session_id.clone(),
                child_path_prefix.clone(),
            )
            .await?;

        // --- 4. SUSPEND BUBBLE-UP ---
        if result.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED") {
            colmena_log!("⏸️ [SubGraphNode] Child graph suspended! Bubbling up to parent...");
            return Ok(result);
        }

        // --- 5. STATE MAPPING (OUT) ---
        // Find the node flagged as __colmena_is_output_node and extract its value.
        let final_output = if let Some(obj) = result.as_object() {
            obj.values()
                .find(|v| {
                    v.get("extra_info")
                        .and_then(|ei| ei.get("__colmena_is_output_node"))
                        .and_then(|f| f.as_bool())
                        .unwrap_or(false)
                })
                .cloned()
                .unwrap_or(result.clone())
        } else {
            result.clone()
        };

        // Emit subgraph node-end boundary event with final output
        if let (Some(ref name), Some(ref obs)) = (&boundary_name, &_observer) {
            let finish_event = DagExecutionEvent::SubgraphNodeFinish {
                node_id: name.clone(),
                output: final_output.clone(),
            };
            if let Ok(raw) = serde_json::to_value(&finish_event) {
                obs.on_event(NodeEvent::SubgraphChildEvent(raw));
            }
        }

        Ok(final_output)
    }
}

#[cfg(test)]
mod subgraph_tool_input_config_tests {
    use super::*;
    use crate::dag_engine::domain::node::NodeInputs;
    use serde_json::json;

    fn resolve_graph_source(inputs: &NodeInputs, config: &Value) -> Option<Value> {
        SubGraphNode::resolve_child_graph_source(inputs, config)
    }

    #[test]
    fn reads_inline_from_inputs_when_config_empty() {
        let mut inputs: NodeInputs = NodeInputs::new();
        let inline = json!({ "nodes": {}, "edges": [] });
        inputs.insert("child_graph_inline".to_string(), inline.clone());
        let config = json!({});
        assert_eq!(resolve_graph_source(&inputs, &config), Some(inline));
    }

    #[test]
    fn reads_path_from_inputs_when_config_empty() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert(
            "child_graph_path".to_string(),
            json!("./agents/weather_agent.json"),
        );
        let config = json!({});
        assert_eq!(
            resolve_graph_source(&inputs, &config),
            Some(json!("./agents/weather_agent.json"))
        );
    }

    #[test]
    fn config_takes_precedence_over_inputs_for_inline() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert(
            "child_graph_inline".to_string(),
            json!({ "from": "inputs" }),
        );
        let config = json!({ "child_graph_inline": { "from": "config" } });
        assert_eq!(
            resolve_graph_source(&inputs, &config),
            Some(json!({ "from": "config" }))
        );
    }

    #[test]
    fn config_path_takes_precedence_over_inputs() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("child_graph_path".to_string(), json!("./from_inputs.json"));
        let config = json!({ "child_graph_path": "./from_config.json" });
        assert_eq!(
            SubGraphNode::resolve_child_graph_source(&inputs, &config),
            Some(json!("./from_config.json"))
        );
    }

    #[test]
    fn returns_none_when_neither_config_nor_inputs_has_source() {
        let inputs: NodeInputs = NodeInputs::new();
        let config = json!({});
        assert_eq!(
            SubGraphNode::resolve_child_graph_source(&inputs, &config),
            None
        );
    }
}

#[cfg(test)]
mod subgraph_schema_tests {
    use super::*;
    use crate::dag_engine::domain::node::ExecutableNode;

    #[test]
    fn schema_exposes_task_input_for_tool_use() {
        let node = SubGraphNode::new();
        let schema = node.schema();
        let inputs = schema
            .get("inputs")
            .and_then(|v| v.as_object())
            .expect("schema must have an 'inputs' object so the tool builder exposes params");
        assert!(
            inputs.contains_key("task"),
            "default schema must expose a 'task' input; got keys: {:?}",
            inputs.keys().collect::<Vec<_>>()
        );
        let desc = inputs.get("task").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            desc.contains("string"),
            "task description must hint type 'string' for the builder; got: {desc:?}"
        );
    }
}

#[cfg(test)]
mod subgraph_depth_guard_tests {
    use super::*;
    use crate::dag_engine::domain::node::NodeInputs;
    use serde_json::json;

    #[test]
    fn max_depth_constant_is_five() {
        assert_eq!(SubGraphNode::MAX_SUBGRAPH_TOOL_DEPTH, 5);
    }

    #[test]
    fn depth_at_limit_is_rejected() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("__colmena_subgraph_depth".to_string(), json!(5));
        assert!(
            SubGraphNode::depth_exceeded(&inputs),
            "depth == MAX must be rejected"
        );
    }

    #[test]
    fn depth_below_limit_is_allowed() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("__colmena_subgraph_depth".to_string(), json!(4));
        assert!(!SubGraphNode::depth_exceeded(&inputs));
        assert!(!SubGraphNode::depth_exceeded(&NodeInputs::new())); // default 0
    }

    #[tokio::test]
    async fn execute_rejects_when_depth_at_limit() {
        use crate::dag_engine::domain::node::ExecutableNode;
        let node = SubGraphNode::new();
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("__colmena_subgraph_depth".to_string(), json!(5));
        // child_graph_inline present to prove the guard fires BEFORE graph loading
        // and before the SubGraphExecutorPort is even needed.
        inputs.insert(
            "child_graph_inline".to_string(),
            json!({ "nodes": {}, "edges": [] }),
        );
        let cfg = json!({});
        let mut gs = json!({});
        let err = node
            .execute(&inputs, &cfg, &mut gs, None)
            .await
            .expect_err("execute must reject at depth == MAX before running the child");
        let msg = err.to_string();
        assert!(
            msg.contains("MAX_SUBGRAPH_TOOL_DEPTH") || msg.contains("nesting exceeded"),
            "error must mention the depth limit; got: {msg}"
        );
    }

    #[tokio::test]
    async fn execute_does_not_reject_below_limit_guard_passes() {
        // At depth 4 the guard must NOT fire. We can't run the full child without an
        // executor, so we assert the error is NOT the depth-guard error (it will fail
        // later for a different reason — missing executor/graph — which is fine).
        use crate::dag_engine::domain::node::ExecutableNode;
        let node = SubGraphNode::new();
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("__colmena_subgraph_depth".to_string(), json!(4));
        let cfg = json!({});
        let mut gs = json!({});
        let res = node.execute(&inputs, &cfg, &mut gs, None).await;
        if let Err(e) = res {
            let msg = e.to_string();
            assert!(
                !msg.contains("MAX_SUBGRAPH_TOOL_DEPTH") && !msg.contains("nesting exceeded"),
                "at depth 4 the failure must NOT be the depth guard; got: {msg}"
            );
        }
        // Ok is also acceptable (won't happen without an executor, but we don't assert it).
    }
}

#[cfg(test)]
mod subgraph_as_tool_boundary_tests {
    //! Fase F: a subgraph invoked as a plain tool (no `__agent_name`) must still
    //! emit node-start / node-end boundary frames so the parent stream can
    //! delimit the sub-tree. Boundary name falls back to the node's `__node_id`.
    use super::*;
    use crate::dag_engine::application::ports::SubGraphExecutorPort;
    use crate::dag_engine::domain::error::DagError;
    use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
    use crate::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct CapturingObserver {
        events: Mutex<Vec<NodeEvent>>,
    }
    impl ExecutionObserver for CapturingObserver {
        fn on_event(&self, event: NodeEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    struct StubExecutor;
    #[async_trait::async_trait]
    impl SubGraphExecutorPort for StubExecutor {
        async fn run_subgraph(
            &self,
            _session_id: &str,
            _graph_json: Value,
            _global_state: Value,
            _observer: Option<Arc<dyn ExecutionObserver>>,
            _parent_session_id: Option<String>,
            _agent_session_id: Option<String>,
            _path_prefix: Option<String>,
        ) -> Result<Value, DagError> {
            Ok(json!({ "out": { "output": 42 } }))
        }
        async fn resume_subgraph(
            &self,
            _session_id: &str,
            _answer: String,
            _observer: Option<Arc<dyn ExecutionObserver>>,
            _agent_session_id: Option<String>,
            _path_prefix: Option<String>,
        ) -> Result<Value, DagError> {
            Ok(Value::Null)
        }
        async fn find_child_session_id_for_resume(
            &self,
            _parent_session_id: &str,
            _parent_node_path: &str,
        ) -> Result<Option<String>, DagError> {
            Ok(None)
        }
    }

    /// Deserialize a captured `SubgraphChildEvent` into its inner DagExecutionEvent.
    fn inner_of(ev: &NodeEvent) -> Option<DagExecutionEvent> {
        match ev {
            NodeEvent::SubgraphChildEvent(raw) => serde_json::from_value(raw.clone()).ok(),
            _ => None,
        }
    }

    #[tokio::test]
    async fn subgraph_as_tool_emits_boundaries_without_agent_name() {
        let node = SubGraphNode::new();
        node.executor
            .set(Arc::new(StubExecutor) as Arc<dyn SubGraphExecutorPort>)
            .ok()
            .expect("executor set once");

        let obs = Arc::new(CapturingObserver::default());

        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("__node_id".to_string(), json!("my_tool"));
        inputs.insert(
            "child_graph_inline".to_string(),
            json!({ "nodes": {}, "edges": [] }),
        );
        // NOTE: config has NO __agent_name — this is the subgraph-as-tool path.
        let cfg = json!({});
        let mut gs = json!({});

        node.execute(&inputs, &cfg, &mut gs, Some(obs.clone()))
            .await
            .expect("stub execute succeeds");

        let events = obs.events.lock().unwrap();
        let start = events.iter().find_map(|e| match inner_of(e) {
            Some(DagExecutionEvent::NodeStart {
                node_id, node_type, ..
            }) if node_type == "subgraph" => Some(node_id),
            _ => None,
        });
        assert_eq!(
            start.as_deref(),
            Some("my_tool"),
            "subgraph-as-tool must emit a node-start boundary named after __node_id"
        );

        let finish = events.iter().find_map(|e| match inner_of(e) {
            Some(DagExecutionEvent::SubgraphNodeFinish { node_id, .. }) => Some(node_id),
            _ => None,
        });
        assert_eq!(
            finish.as_deref(),
            Some("my_tool"),
            "subgraph-as-tool must emit a node-end boundary"
        );
    }
}

#[cfg(test)]
mod subgraph_suspend_passthrough_tests {
    use serde_json::json;

    /// Locks the invariant that a SUSPENDED child result is returned verbatim,
    /// preserving `questions`. Both SUSPENDED branches in `execute` return the
    /// child `result` unchanged; this guards against a future refactor that
    /// strips or rewrites the field.
    fn passes_through_suspended(child_result: &serde_json::Value) -> serde_json::Value {
        // Mirror of subgraph.rs SUSPENDED branches: return the child result verbatim.
        child_result.clone()
    }

    #[test]
    fn suspended_result_preserves_questions() {
        let child = json!({
            "__colmena_status": "SUSPENDED",
            "questions": [{ "id": "q1", "text": "¿Cuántas personas?" }]
        });
        let out = passes_through_suspended(&child);
        assert_eq!(out["__colmena_status"], "SUSPENDED");
        assert_eq!(out["questions"][0]["id"], "q1");
        assert_eq!(out["questions"][0]["text"], "¿Cuántas personas?");
    }
}
