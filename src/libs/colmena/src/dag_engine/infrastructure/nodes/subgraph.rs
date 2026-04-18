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
}

#[async_trait::async_trait]
impl ExecutableNode for SubGraphNode {
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
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
            .unwrap_or("unknown_parent");

        let node_id = inputs
            .get("__node_id")
            .and_then(|v| v.as_str())
            .unwrap_or("subgraph");

        let child_session_id = format!("{}_sub_{}", parent_session_id, node_id);

        // --- 1. RESUME PROPAGATION ---
        // If the parent was suspended in this node, it receives __colmena_resume_answer
        if let Some(resume_answer) = inputs
            .get("__colmena_resume_answer")
            .and_then(|v| v.as_str())
        {
            colmena_log!(
                "▶️ [SubGraphNode] Resuming child graph {} with answer...",
                child_session_id
            );
            let result = self
                .executor
                .get()
                .ok_or("SubGraphExecutorPort not initialized in SubGraphNode")?
                .resume_subgraph(
                    &child_session_id,
                    resume_answer.to_string(),
                    _observer.clone(),
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
        let graph_json = if let Some(inline) = config.get("child_graph_inline") {
            inline.clone()
        } else if let Some(path_val) = config.get("child_graph_path").and_then(|v| v.as_str()) {
            let path = std::path::Path::new(path_val);
            if !path.exists() {
                return Err(format!("child_graph_path not found: {}", path_val).into());
            }
            let contents = fs::read_to_string(path).await?;
            serde_json::from_str(&contents)?
        } else {
            return Err(
                "SubGraphNode requires 'child_graph_inline' or 'child_graph_path' in config".into(),
            );
        };

        // Agent name is injected by OrchestratorNode when this subgraph is called as an agent.
        // When present, we emit boundary node-start / node-end events so the parent stream
        // shows a clear subgraph boundary around the child events.
        let agent_name = config
            .get("__agent_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // --- 3. STATE MAPPING (IN) ---
        // We pass the resolved `inputs` to the child graph as its initial global_state
        let mut child_state_obj = serde_json::Map::new();
        for (k, v) in inputs {
            if !k.starts_with("__colmena_") && k != "__node_id" {
                child_state_obj.insert(k.clone(), v.clone());
            }
        }
        let child_state = Value::Object(child_state_obj);

        // Emit subgraph node-start boundary event (only when called by orchestrator)
        if let (Some(ref name), Some(ref obs)) = (&agent_name, &_observer) {
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

        colmena_log!(
            "🔄 [SubGraphNode] Running SubGraph in isolated session: {}",
            child_session_id
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
        if let (Some(ref name), Some(ref obs)) = (&agent_name, &_observer) {
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
