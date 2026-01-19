use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

pub struct TriggerWebhookNode;

#[async_trait::async_trait]
impl ExecutableNode for TriggerWebhookNode {
    /// The Trigger's execution logic is simple:
    /// It takes the `inputs` (which we will inject from the HTTP request body)
    /// and passes them to the output.
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>, // Added observer parameter
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // Changed `StdError` to `Error`
        // For a trigger, the "inputs" ARE the payload from the outside world.
        // We wrap them in "output" to match our convention.

        // Priority order:
        // 1. __payload__ (injected by serve command)
        // 2. test_payload (defined by user for local testing with run command)
        // 3. inputs (fallback)
        let payload = if let Some(p) = config.get("__payload__") {
            p.clone()
        } else if let Some(p) = config.get("test_payload") {
            p.clone()
        } else {
            // Fallback: use inputs if available (e.g. if not running in serve mode or testing)
            serde_json::to_value(inputs)?
        };

        // We'll return the whole input map as the output object
        // so downstream nodes can access fields like `trigger.message`.
        Ok(json!({ "output": payload }))
    }

    fn description(&self) -> Option<&str> {
        Some("Trigger execution with webhook payloads. Acts as an entry point for external events.")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "trigger_webhook",
            "config": {
                "path": "string", // e.g., "/webhook/test"
                "method": "string", // e.g., "POST"
                "test_payload": "any (optional, for local testing with 'run' command)"
            },
            "outputs": {
                "output": "any"
            }
        })
    }
}
