use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

pub struct InputNode;

#[async_trait::async_trait]
impl ExecutableNode for InputNode {
    /// The Input node ignores upstream inputs and payload, and just outpus the static `data` defined in its config.
    async fn execute(
        &self,
        _inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // If a state payload was injected (e.g. from a loop), yield it directly 
        // to prevent double-nesting the graph output state.
        if let Some(p) = config.get("__payload__") {
            return Ok(p.clone());
        }

        // Check for static "data" key first
        if let Some(data) = config.get("data") {
            return Ok(data.clone());
        }

        // Fallback: return the whole configuration directly
        // This allows users to define inputs at the top level of the config.
        Ok(config.clone())
    }

    fn description(&self) -> Option<&str> {
        Some("Input node that outputs hardcoded data from its configuration.")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "input",
            "config": {
                "data": "any (the static data to output)"
            },
            "outputs": {
                "output": "any"
            }
        })
    }
}
