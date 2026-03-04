use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

pub struct LoopControllerNode;

impl LoopControllerNode {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl ExecutableNode for LoopControllerNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // Evaluate loop status
        let mut loop_status = inputs
            .get("loop_status")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("loop_status").and_then(|v| v.as_str()))
            .unwrap_or("FINISHED")
            .to_string();

        // Check for suspend flag
        let suspend_flag = inputs
            .get("suspend_flag")
            .and_then(|v| v.as_bool())
            .or_else(|| config.get("suspend_flag").and_then(|v| v.as_bool()))
            .unwrap_or(false);

        if suspend_flag {
             loop_status = "SUSPENDED".to_string();
        }

        let mut output_payload = json!({
            "__colmena_loop_status": loop_status
        });

        // Add additional context based on status
        if loop_status == "SUSPENDED" {
            if let Some(question) = inputs.get("question").or_else(|| config.get("question")) {
                 output_payload.as_object_mut().unwrap().insert("question".to_string(), question.clone());
            }
        } else if loop_status == "FINISHED" {
            if let Some(final_result) = inputs.get("all_tasks").or_else(|| config.get("all_tasks")) {
                 output_payload.as_object_mut().unwrap().insert("final_result".to_string(), final_result.clone());
            }
        }

        Ok(json!({
            "output": output_payload
        }))
    }

    fn description(&self) -> Option<&str> {
        Some("Aggregates state and determines whether to continue the loop (NEXT_TURN), suspend (SUSPENDED), or break it (FINISHED).")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "loop_controller",
            "inputs": {
                "loop_status": "string (NEXT_TURN, FINISHED, defaults to FINISHED)",
                "suspend_flag": "boolean (If true, overrides loop_status to SUSPENDED)",
                "question": "string (Question to prompt the user if suspended)",
                "all_tasks": "array/object (The final payload to output if FINISHED)"
            },
            "outputs": {
                "__colmena_loop_status": "string (NEXT_TURN, FINISHED, or SUSPENDED)",
                "question": "string (optional)",
                "final_result": "any (optional)"
            }
        })
    }
}
