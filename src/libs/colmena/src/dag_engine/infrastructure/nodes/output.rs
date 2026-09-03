use crate::dag_engine::domain::lint::NodeCatalogEntry;
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
            "result": input_val,
            "extra_info": {
                "__colmena_is_output_node": true
            }
        }))
    }

    fn default_input(&self) -> Option<&str> {
        Some("input")
    }

    fn default_output(&self) -> Option<&str> {
        Some("result")
    }

    fn schema(&self) -> Value {
        serde_json::json!({})
    }

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        // `execute` takes `_config`; it wraps the `input` edge.
        Some(NodeCatalogEntry::no_config())
    }
}
