//! Internal stub toolkit node used by runtime tests.
//!
//! Not registered in the default `HashMapNodeRegistry`. Construct directly in
//! tests, or register via `HashMapNodeRegistry::register_toolkit_node`.

use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::domain::toolkit_node::{
    SubToolDefinition, ToolkitNode, SUB_TOOL_INPUT_KEY,
};
use crate::llm::domain::tools::ParameterProperty;
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;

pub struct EchoToolkitNode;

#[async_trait::async_trait]
impl ExecutableNode for EchoToolkitNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let sub_tool = inputs
            .get(SUB_TOOL_INPUT_KEY)
            .and_then(|v| v.as_str())
            .ok_or("missing __sub_tool")?;
        match sub_tool {
            "echo" => {
                let msg = inputs
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Ok(json!({ "output": msg }))
            }
            "double" => {
                let n = inputs
                    .get("n")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                Ok(json!({ "output": n * 2.0 }))
            }
            other => Err(format!("unknown sub_tool: {other}").into()),
        }
    }

    fn schema(&self) -> Value {
        json!({ "inputs": {}, "outputs": { "output": "any" } })
    }

    fn description(&self) -> Option<&str> {
        Some("Echo toolkit stub — internal test use only.")
    }
}

impl ToolkitNode for EchoToolkitNode {
    fn sub_tool_catalog(&self, _config: &Value) -> Vec<SubToolDefinition> {
        let mut echo_props = HashMap::new();
        echo_props.insert(
            "message".to_string(),
            ParameterProperty {
                property_type: "string".to_string(),
                description: "Text to echo back".to_string(),
                enum_values: None,
                pattern: None,
            },
        );

        let mut double_props = HashMap::new();
        double_props.insert(
            "n".to_string(),
            ParameterProperty {
                property_type: "number".to_string(),
                description: "Number to double".to_string(),
                enum_values: None,
                pattern: None,
            },
        );

        vec![
            SubToolDefinition {
                name: Cow::Borrowed("echo"),
                description: "Return the input string unchanged.".to_string(),
                properties: echo_props,
                required: vec!["message".to_string()],
            },
            SubToolDefinition {
                name: Cow::Borrowed("double"),
                description: "Return twice the input number.".to_string(),
                properties: double_props,
                required: vec!["n".to_string()],
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dispatches_on_sub_tool_echo() {
        let node = EchoToolkitNode;
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("echo"));
        inputs.insert("message".into(), json!("hi"));
        let mut state = json!({});
        let out = node.execute(&inputs, &json!({}), &mut state, None).await.unwrap();
        assert_eq!(out.get("output").unwrap().as_str(), Some("hi"));
    }

    #[tokio::test]
    async fn dispatches_on_sub_tool_double() {
        let node = EchoToolkitNode;
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("double"));
        inputs.insert("n".into(), json!(4));
        let mut state = json!({});
        let out = node.execute(&inputs, &json!({}), &mut state, None).await.unwrap();
        assert_eq!(out.get("output").unwrap().as_f64(), Some(8.0));
    }

    #[tokio::test]
    async fn catalog_has_two_entries() {
        let node = EchoToolkitNode;
        let cat = node.sub_tool_catalog(&json!({}));
        assert_eq!(cat.len(), 2);
        assert!(cat.iter().any(|d| d.name == "echo"));
        assert!(cat.iter().any(|d| d.name == "double"));
    }
}
