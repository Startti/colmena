use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use crate::dag_engine::domain::observer::ExecutionObserver;
use std::error::Error;

/// A mock node specifically designed to test the Suspend/Resume functionality 
/// of the DagRunUseCase.
pub struct SuspendNode;

#[async_trait]
impl ExecutableNode for SuspendNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _global_state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        // If the human answer is already provided in the inputs (injected by DagRunUseCase during resume)
        if let Some(answer) = inputs.get("__colmena_resume_answer") {
            return Ok(serde_json::json!({
                "status": "resumed",
                "answer_received": answer
            }));
        }

        // If not, we suspend and ask the question configured in the node (or default)
        let question = config
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("What is your input?");

        Ok(serde_json::json!({
            "__colmena_status": "SUSPENDED",
            "question": question
        }))
    }

    fn schema(&self) -> Value {
        serde_json::json!({})
    }
}
