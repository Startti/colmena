//! `current_time` node — returns the current UTC timestamp as ISO-8601.
//!
//! Real, side-effect-free node intended for use as an LLM tool when the model
//! needs the wall-clock time.

use crate::dag_engine::domain::lint::NodeCatalogEntry;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use chrono::Utc;
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

pub struct CurrentTimeNode;

#[async_trait::async_trait]
impl ExecutableNode for CurrentTimeNode {
    async fn execute(
        &self,
        _inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let now = Utc::now().to_rfc3339();
        Ok(json!({ "output": now }))
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "current_time",
            "inputs": {},
            "outputs": { "output": "string" }
        })
    }

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        // No config; emits the current UTC time.
        Some(NodeCatalogEntry::no_config())
    }

    fn description(&self) -> Option<&str> {
        Some("Return the current UTC timestamp as an ISO-8601 string. Takes no parameters.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn returns_iso8601_utc_timestamp() {
        let node = CurrentTimeNode;
        let inputs: NodeInputs = HashMap::new();
        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        let s = out.get("output").and_then(|v| v.as_str()).unwrap();
        assert!(s.contains('T'), "expected ISO-8601, got {s}");
        assert!(
            s.ends_with('Z') || s.contains('+') || s.contains('-'),
            "expected timezone suffix, got {s}"
        );
    }
}
