use crate::dag_engine::domain::error::DagError;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct OutputNode;

#[async_trait]
impl ExecutableNode for OutputNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let input_val = if let Some(val) = inputs.get("input") {
            val.clone()
        } else {
            Value::Null
        };

        Ok(serde_json::json!({
            "output": {
                "result": input_val,
                "extra_info": {
                    "__colmena_is_output_node": true
                }
            }
        }))
    }

    fn schema(&self) -> Value {
        serde_json::json!({})
    }
}
